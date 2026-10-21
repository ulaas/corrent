//! The native player: libmpv rendering into an OpenGL texture inside the egui window (software
//! rendering as fallback), with egui controls on top. Music plays without video in a mini
//! player that keeps going while browsing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use corrent_mpv::render::RenderContext;
use corrent_mpv::{EndReason, Event, Mpv, Node};
use eframe::glow::{self, HasContext};
use egui::{Color32, CornerRadius, Rect, Sense, Ui, pos2, vec2};

use corrent_proto::library::{CastCommand, RoomCommand};

use super::client::MediaClient;
use crate::ui::theme::Palette;

/// What is playing.
#[derive(Clone, Debug)]
pub struct QueueEntry {
    pub item_id: i64,
    pub title: String,
    pub subtitle: String,
    pub resume_ms: i64,
    pub poster: Option<String>,
    /// An offline copy to play instead of asking the server.
    pub local: Option<std::path::PathBuf>,
    /// A live stream (internet radio): played as it is, no progress kept.
    pub stream: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub selected: bool,
}

#[derive(Default)]
struct Status {
    pos: f64,
    duration: f64,
    paused: bool,
    volume: f64,
    muted: bool,
    tracks: Vec<Track>,
    /// (start in seconds, title)
    chapters: Vec<(f64, Option<String>)>,
    /// Skippable parts found by analysis (from the playback info).
    segments: Vec<corrent_proto::library::MediaSegment>,
    ended: bool,
    buffering: bool,
    error: Option<String>,
    hwdec: String,
    aspect: f64,
    speed: f64,
    sub_delay: f64,
    audio_delay: f64,
    sub_scale: f64,
    stats: Stats,
}

/// "Stats for nerds".
#[derive(Clone, Debug, Default)]
struct Stats {
    video_codec: String,
    audio_codec: String,
    width: i64,
    height: i64,
    fps: f64,
    dropped: i64,
    cache_secs: f64,
}

struct GlVideo {
    rc: RenderContext,
    gl: Arc<glow::Context>,
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    size: [i32; 2],
}

impl GlVideo {
    fn paint(&mut self, want: [i32; 2]) {
        let gl = self.gl.clone();
        let resized = want != self.size;
        // SAFETY: called inside egui_glow's paint callback, with eframe's GL context current.
        unsafe {
            if resized {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.tex));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    want[0],
                    want[1],
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                self.size = want;
            }
            if !self.rc.update() && !resized {
                return;
            }
            // egui_glow restores its state after a callback, but not the framebuffer binding.
            let prev = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            if let Err(e) = self.rc.render_gl(self.fbo.0.get() as i32, want[0], want[1], false) {
                tracing::debug!("video render: {e}");
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, std::num::NonZeroU32::new(prev as u32).map(glow::NativeFramebuffer));
        }
    }
}

impl Drop for GlVideo {
    fn drop(&mut self) {
        // SAFETY: dropped on the UI thread (the app owns the player), GL context current.
        unsafe {
            self.gl.delete_framebuffer(self.fbo);
            self.gl.delete_texture(self.tex);
        }
    }
}

enum Video {
    None,
    Gl(Arc<Mutex<GlVideo>>, Option<egui::TextureId>),
    Sw { rc: RenderContext, buf: Vec<u8>, tex: Option<egui::TextureHandle>, size: [usize; 2] },
}

/// GL access for the player (from eframe's creation context).
#[derive(Clone)]
pub struct GlAccess {
    pub gl: Arc<glow::Context>,
    pub proc: Arc<dyn Fn(&std::ffi::CStr) -> *const std::ffi::c_void + Send + Sync>,
}

pub struct Player {
    mpv: Arc<Mpv>,
    video: Video,
    status: Arc<Mutex<Status>>,
    pub queue: Vec<QueueEntry>,
    pub index: usize,
    pub audio_only: bool,
    pub fullscreen: bool,
    client: MediaClient,
    rt: tokio::runtime::Handle,
    last_report: Instant,
    controls_until: Instant,
    stop_events: Arc<AtomicBool>,
    /// The texture created this frame, to register with eframe.
    pub needs_registration: Option<glow::Texture>,
    seek_drag: Option<f32>,
    /// Seek-bar thumbnails of the current item (empty until loaded, or when there are none).
    trick: Arc<Mutex<Vec<Cue>>>,
    sheets: std::collections::HashMap<String, Sheet>,
    /// Watching together, and the code typed to join a room.
    pub room: Option<super::syncplay::Room>,
    room_code: String,
    /// Showing "Stats for nerds".
    stats: bool,
    /// Picture in picture: the window small and on top (the size to go back to).
    pip: Option<egui::Vec2>,
    /// The end-of-episode countdown was dismissed for this item.
    no_countdown: Option<i64>,
    cast: super::cast::CastCtl,
    /// Where progress goes while the server can't be reached (remote servers).
    pub offline: Option<(Arc<super::offline::Store>, String)>,
    /// Casting to a device: the controls drive it and mpv waits, paused.
    casting: Option<super::cast::Casting>,
    /// The current track's lyrics (item id, answer).
    lyrics: Option<(i64, super::client::Slot<corrent_proto::library::Lyrics>)>,
    ctx: egui::Context,
    /// "Use this frame as the thumbnail" on its way to the server.
    thumb_job: Option<super::client::Slot<serde_json::Value>>,
    /// A message for the app's toasts (taken by the media view).
    pub notice: Option<(crate::engine::ToastKind, String)>,
}

fn track_list(n: &Node) -> Vec<Track> {
    n.as_array()
        .iter()
        .filter_map(|t| {
            let kind = t.get("type")?.as_str()?.to_string();
            let id = t.get("id")?.as_i64()?;
            let lang = t.get("lang").and_then(Node::as_str).unwrap_or("");
            let title = t.get("title").and_then(Node::as_str).unwrap_or("");
            let codec = t.get("codec").and_then(Node::as_str).unwrap_or("");
            let label = [title, lang, codec].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
            Some(Track {
                id,
                kind,
                title: if label.is_empty() { format!("#{id}") } else { label },
                selected: t.get("selected").and_then(Node::as_bool) == Some(true),
            })
        })
        .collect()
}

/// One seek-bar thumbnail: a rectangle of a sprite sheet for a time range.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    /// API path of the sheet image.
    pub sheet: String,
    pub rect: [u32; 4],
}

fn vtt_secs(ts: &str) -> f64 {
    ts.trim().split(':').fold(0.0, |a, v| a * 60.0 + v.trim().parse::<f64>().unwrap_or(0.0))
}

/// Parses a trickplay index (WebVTT whose cues are `sheet-0.jpg#xywh=x,y,w,h`), resolving sheet
/// names against the index's own path.
pub fn parse_trickplay(vtt: &str, index_path: &str) -> Vec<Cue> {
    let dir = index_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let mut out = Vec::new();
    for block in vtt.replace('\r', "").split("\n\n") {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        let Some(ti) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let Some(target) = lines.get(ti + 1) else { continue };
        let Some((a, z)) = lines.get(ti).and_then(|l| l.split_once("-->")) else { continue };
        let Some((file, xywh)) = target.trim().split_once("#xywh=") else { continue };
        let n: Vec<u32> = xywh.split(',').filter_map(|v| v.trim().parse().ok()).collect();
        let [x, y, w, h] = n[..] else { continue };
        let sheet = if file.starts_with('/') { file.to_string() } else { format!("{dir}/{file}") };
        out.push(Cue {
            start: vtt_secs(a),
            end: vtt_secs(z.split_whitespace().next().unwrap_or("")),
            sheet,
            rect: [x, y, w, h],
        });
    }
    out
}

fn cue_at(cues: &[Cue], t: f64) -> Option<&Cue> {
    let i = cues.partition_point(|c| c.end <= t);
    cues.get(i).filter(|c| c.start <= t)
}

enum Sheet {
    Loading(super::client::Slot<egui::ColorImage>),
    Ready(egui::TextureHandle),
    Failed,
}

/// Below this width the player's controls drop the volume slider (the time moves above the bar).
const NARROW: f32 = 760.0;
/// Below this width they also drop previous/next, watch together and cast.
const TINY: f32 = 560.0;

pub fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

impl Player {
    /// Starts mpv for `queue[0]`. `gl`: render into the window; `None` with `audio_only`.
    pub fn new(
        client: MediaClient,
        rt: tokio::runtime::Handle,
        ctx: &egui::Context,
        gl: Option<&GlAccess>,
        queue: Vec<QueueEntry>,
        audio_only: bool,
        audio: &corrent_core::config::Playback,
    ) -> Result<Player, String> {
        let lib = corrent_mpv::Library::get().map_err(|e| e.to_string())?;
        let header = client.auth_header();
        let ca = client.ca_file().map(|p| p.display().to_string());
        let mut opts: Vec<(&str, &str)> = corrent_mpv::HEADLESS.to_vec();
        opts.retain(|(k, _)| *k != "idle");
        opts.extend([
            ("idle", "yes"),
            ("keep-open", "yes"),
            ("hwdec", "auto-safe"),
            ("http-header-fields", header.as_str()),
            ("audio-display", "no"),
        ]);
        let audio_opts = audio.mpv_options();
        opts.extend(audio_opts.iter().map(|(k, v)| (*k, v.as_str())));
        opts.push(if audio_only { ("vid", "no") } else { ("vo", "libmpv") });
        // mpv sends the token: always verify HTTPS (mpv doesn't by default), against the
        // server's own CA when it has one.
        opts.push(("tls-verify", "yes"));
        if let Some(ca) = &ca {
            opts.push(("tls-ca-file", ca.as_str()));
        }
        let mpv = Arc::new(Mpv::new(lib, &opts).map_err(|e| e.to_string())?);
        let force_sw = std::env::var_os("CORRENT_SW_VIDEO").is_some();
        let video = if audio_only {
            Video::None
        } else {
            match gl.filter(|_| !force_sw) {
                Some(g) => {
                    let proc = g.proc.clone();
                    match RenderContext::new_gl(mpv.clone(), move |n| proc(n), false) {
                        Ok(rc) => {
                            let (fbo, tex) = create_target(&g.gl)?;
                            Video::Gl(
                                Arc::new(Mutex::new(GlVideo { rc, gl: g.gl.clone(), fbo, tex, size: [16, 16] })),
                                None,
                            )
                        }
                        Err(e) => {
                            tracing::warn!("OpenGL video unavailable ({e}); using software rendering");
                            Video::Sw {
                                rc: RenderContext::new_sw(mpv.clone()).map_err(|e| e.to_string())?,
                                buf: Vec::new(),
                                tex: None,
                                size: [0, 0],
                            }
                        }
                    }
                }
                None => Video::Sw {
                    rc: RenderContext::new_sw(mpv.clone()).map_err(|e| e.to_string())?,
                    buf: Vec::new(),
                    tex: None,
                    size: [0, 0],
                },
            }
        };
        let repaint = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        let mut needs_registration = None;
        let video = match video {
            Video::Gl(v, id) => {
                {
                    let mut g = v.lock().unwrap_or_else(|e| e.into_inner());
                    g.rc.set_update_callback(repaint);
                    needs_registration = Some(g.tex);
                }
                Video::Gl(v, id)
            }
            Video::Sw { mut rc, buf, tex, size } => {
                rc.set_update_callback(repaint);
                Video::Sw { rc, buf, tex, size }
            }
            Video::None => Video::None,
        };
        let status = Arc::new(Mutex::new(Status { volume: 100.0, speed: 1.0, sub_scale: 1.0, ..Default::default() }));
        for (i, p) in [
            "time-pos",
            "duration",
            "pause",
            "volume",
            "mute",
            "track-list",
            "chapter-list",
            "paused-for-cache",
            "hwdec-current",
            "video-params/aspect",
            "speed",
            "sub-delay",
            "audio-delay",
            "sub-scale",
            "video-codec",
            "video-params/w",
            "video-params/h",
            "estimated-vf-fps",
            "frame-drop-count",
            "demuxer-cache-duration",
            "audio-codec-name",
        ]
        .iter()
        .enumerate()
        {
            mpv.observe(i as u64, p).map_err(|e| e.to_string())?;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (m, st, c2, stop2) = (mpv.clone(), status.clone(), ctx.clone(), stop.clone());
        std::thread::Builder::new()
            .name("mpv-events".into())
            .spawn(move || {
                while !stop2.load(Ordering::Relaxed) {
                    let Some(ev) = m.wait_event(0.5) else { continue };
                    let mut s = st.lock().unwrap_or_else(|e| e.into_inner());
                    match ev {
                        Event::Shutdown => break,
                        Event::PropertyChange { name, value, .. } => match name.as_str() {
                            "time-pos" => s.pos = value.as_f64().unwrap_or(s.pos),
                            "duration" => s.duration = value.as_f64().unwrap_or(0.0),
                            "pause" => s.paused = value.as_bool().unwrap_or(false),
                            "volume" => s.volume = value.as_f64().unwrap_or(100.0),
                            "mute" => s.muted = value.as_bool().unwrap_or(false),
                            "track-list" => s.tracks = track_list(&value),
                            "chapter-list" => {
                                s.chapters = value
                                    .as_array()
                                    .iter()
                                    .filter_map(|c| {
                                        let t = c.get("time").and_then(Node::as_f64)?;
                                        Some((t, c.get("title").and_then(Node::as_str).map(str::to_string)))
                                    })
                                    .collect()
                            }
                            "paused-for-cache" => s.buffering = value.as_bool().unwrap_or(false),
                            "hwdec-current" => s.hwdec = value.as_str().unwrap_or("no").to_string(),
                            "video-params/aspect" => s.aspect = value.as_f64().unwrap_or(0.0),
                            "speed" => s.speed = value.as_f64().unwrap_or(1.0),
                            "sub-delay" => s.sub_delay = value.as_f64().unwrap_or(0.0),
                            "audio-delay" => s.audio_delay = value.as_f64().unwrap_or(0.0),
                            "sub-scale" => s.sub_scale = value.as_f64().unwrap_or(1.0),
                            "video-codec" => s.stats.video_codec = value.as_str().unwrap_or("").to_string(),
                            "audio-codec-name" => s.stats.audio_codec = value.as_str().unwrap_or("").to_string(),
                            "video-params/w" => s.stats.width = value.as_i64().unwrap_or(0),
                            "video-params/h" => s.stats.height = value.as_i64().unwrap_or(0),
                            "estimated-vf-fps" => s.stats.fps = value.as_f64().unwrap_or(0.0),
                            "frame-drop-count" => s.stats.dropped = value.as_i64().unwrap_or(0),
                            "demuxer-cache-duration" => s.stats.cache_secs = value.as_f64().unwrap_or(0.0),
                            _ => {}
                        },
                        Event::FileLoaded => {
                            s.ended = false;
                            s.error = None;
                        }
                        Event::EndFile(EndReason::Eof) => s.ended = true,
                        Event::EndFile(EndReason::Error(code)) => s.error = Some(format!("playback failed ({code})")),
                        _ => {}
                    }
                    drop(s);
                    c2.request_repaint();
                }
            })
            .map_err(|e| e.to_string())?;
        let cast = super::cast::CastCtl::new(client.clone(), rt.clone());
        let mut p = Player {
            mpv,
            video,
            status,
            queue,
            index: 0,
            audio_only,
            fullscreen: false,
            client,
            rt,
            last_report: Instant::now(),
            controls_until: Instant::now() + Duration::from_secs(3),
            stop_events: stop,
            needs_registration,
            seek_drag: None,
            trick: Arc::default(),
            sheets: Default::default(),
            room: None,
            room_code: String::new(),
            stats: false,
            pip: None,
            no_countdown: None,
            cast,
            casting: None,
            lyrics: None,
            thumb_job: None,
            notice: None,
            ctx: ctx.clone(),
            offline: None,
        };
        p.load_current();
        Ok(p)
    }

    pub fn current(&self) -> Option<&QueueEntry> {
        self.queue.get(self.index)
    }

    fn load_current(&mut self) {
        let Some(e) = self.current().cloned() else { return };
        if let Some(url) = &e.stream {
            let title = format!("force-media-title=%{}%{}", e.title.len(), e.title);
            let _ = self.mpv.command_async(0, &["loadfile", url, "replace", "-1", &title]);
            return;
        }
        if let Some(path) = &e.local {
            {
                let mut t = crate::util::lock(&self.trick);
                t.clear();
            }
            self.sheets.clear();
            let start = format!("start={}", e.resume_ms as f64 / 1000.0);
            let title = format!("force-media-title=%{}%{}", e.title.len(), e.title);
            let _ = self
                .mpv
                .command_async(0, &["loadfile", &path.to_string_lossy(), "replace", "-1", &format!("{start},{title}")]);
            {
                let mut s = crate::util::lock(&self.status);
                s.pos = e.resume_ms as f64 / 1000.0;
                s.ended = false;
            }
            return;
        }
        // The server decides; the native client plays everything directly.
        let client = self.client.clone();
        let mpv = self.mpv.clone();
        let trick = self.trick.clone();
        {
            let mut t = crate::util::lock(&trick);
            t.clear();
        }
        let status = self.status.clone();
        {
            let mut s = crate::util::lock(&status);
            s.segments.clear();
        }
        self.sheets.clear();
        self.rt.spawn(async move {
            let info: Result<corrent_proto::library::PlaybackInfo, String> = client
                .send(
                    reqwest::Method::POST,
                    &format!("/api/v1/playback/{}", e.item_id),
                    &serde_json::json!({ "profile": "native" }),
                )
                .await;
            match info {
                Ok(info) => {
                    {
                        let mut s = crate::util::lock(&status);
                        s.segments = info.segments.clone();
                    }
                    if let Some(tp) = info.trickplay.clone() {
                        let (client, trick) = (client.clone(), trick.clone());
                        tokio::spawn(async move {
                            if let Ok(vtt) = client.text(&tp).await {
                                *crate::util::lock(&trick) = parse_trickplay(&vtt, &tp);
                            }
                        });
                    }
                    let url = client.url(&info.url);
                    let start = format!("start={}", e.resume_ms as f64 / 1000.0);
                    let title = format!("force-media-title=%{}%{}", e.title.len(), e.title);
                    let _ = mpv.command(&["loadfile", &url, "replace", "-1", &format!("{start},{title}")]);
                    for s in info.subtitles.iter().filter_map(|s| s.url.as_ref().map(|u| (u, s))) {
                        let _ = mpv.command(&[
                            "sub-add",
                            &client.url(s.0),
                            "auto",
                            s.1.title.as_deref().unwrap_or(""),
                            s.1.lang.as_deref().unwrap_or(""),
                        ]);
                    }
                }
                Err(e) => tracing::warn!("playback: {e}"),
            }
        });
        let mut s = crate::util::lock(&self.status);
        s.pos = e.resume_ms as f64 / 1000.0;
        s.ended = false;
    }

    /// A subtitle file from this computer, shown right away (for this playback only).
    pub fn add_subtitle(&self, path: &std::path::Path) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.cmd(&["sub-add", &path.to_string_lossy(), "select", &name]);
    }

    /// The folder of the video when it plays from this computer, to start the picker there.
    pub fn local_folder(&self) -> Option<std::path::PathBuf> {
        self.current()?.local.as_ref()?.parent().map(std::path::Path::to_path_buf)
    }

    /// Commands from the UI thread are asynchronous: the UI never waits for mpv's core.
    fn cmd(&self, args: &[&str]) {
        let _ = self.mpv.command_async(0, args);
    }

    /// New sound settings while playing.
    pub fn apply_audio(&self, audio: &corrent_core::config::Playback) {
        for (k, v) in audio.mpv_options() {
            self.cmd(&["set", k, &v]);
        }
    }

    pub fn toggle_pause(&mut self) {
        if let Some(c) = &self.casting {
            let playing = self.cast.status().is_some_and(|s| s.state == "playing" || s.state == "buffering");
            self.cast.command(c, if playing { CastCommand::Pause } else { CastCommand::Play });
            return;
        }
        self.cmd(&["cycle", "pause"]);
        if let Some(room) = &mut self.room {
            let (pos, _, paused) = crate::util::with_lock(&self.status, |s| (s.pos, s.duration, s.paused));
            let position_ms = (pos * 1000.0) as i64;
            room.send(if paused { RoomCommand::Play { position_ms } } else { RoomCommand::Pause { position_ms } });
        }
    }

    pub fn seek(&mut self, secs: f64, relative: bool) {
        if let Some(c) = &self.casting {
            let pos = self.cast.status().map(|s| s.position_ms as f64 / 1000.0).unwrap_or(0.0);
            let to = if relative { pos + secs } else { secs };
            self.cast.command(c, CastCommand::Seek { position_ms: (to.max(0.0) * 1000.0) as i64 });
            return;
        }
        self.cmd(&["seek", &format!("{secs}"), if relative { "relative" } else { "absolute" }]);
        if let Some(room) = &mut self.room {
            let pos = crate::util::with_lock(&self.status, |s| s.pos);
            let to = if relative { pos + secs } else { secs };
            room.send(RoomCommand::Seek { position_ms: (to.max(0.0) * 1000.0) as i64 });
        }
    }

    /// Follows the room (when watching together).
    fn sync_room(&mut self, ctx: &egui::Context) {
        let Some(item) = self.current().map(|e| e.item_id) else { return };
        let Some(room) = &mut self.room else { return };
        room.poll(ctx);
        let (pos, paused, buffering) = crate::util::with_lock(&self.status, |s| (s.pos, s.paused, s.buffering));
        let c = room.correction(item, pos, paused, buffering);
        if let Some(to) = c.seek_to {
            self.cmd(&["seek", &format!("{to}"), "absolute"]);
        }
        if let Some(p) = c.pause {
            self.cmd(&["set", "pause", if p { "yes" } else { "no" }]);
        }
    }

    pub fn next(&mut self) -> bool {
        if self.index + 1 < self.queue.len() {
            self.report(true);
            self.index += 1;
            self.load_current();
            true
        } else {
            false
        }
    }

    pub fn previous(&mut self) {
        if crate::util::with_lock(&self.status, |s| s.pos > 5.0) || self.index == 0 {
            self.seek(0.0, false);
        } else {
            self.report(true);
            self.index -= 1;
            self.load_current();
        }
    }

    /// Tells the server how far we got (`done`: the item ended or we moved on).
    pub fn report(&mut self, done: bool) {
        let Some(e) = self.current() else { return };
        if e.stream.is_some() {
            return;
        }
        let (pos, dur, ended) = crate::util::with_lock(&self.status, |s| (s.pos, s.duration, s.ended));
        let body = if ended || (done && dur > 0.0 && pos / dur > 0.9) {
            serde_json::json!({ "played": true })
        } else {
            serde_json::json!({ "position_ms": (pos * 1000.0) as i64 })
        };
        let (client, id, offline) = (self.client.clone(), e.item_id, self.offline.clone());
        self.rt.spawn(async move {
            let r: Result<serde_json::Value, String> =
                client.send(reqwest::Method::PUT, &format!("/api/v1/items/{id}/state"), &body).await;
            if r.is_err()
                && let Some((store, server)) = offline
            {
                store.queue_state(&server, id, body);
            }
        });
        self.last_report = Instant::now();
    }

    /// Per-frame housekeeping: progress reports and advancing the queue at the end.
    pub fn tick(&mut self) {
        if self.last_report.elapsed() > Duration::from_secs(10) {
            self.report(false);
        }
        let ended = crate::util::with_lock(&self.status, |s| s.ended);
        if ended {
            self.report(true);
            {
                let mut s = crate::util::lock(&self.status);
                s.ended = false;
            }
            // "Cancel" on the countdown: stay at the end.
            if self.no_countdown != self.current().map(|e| e.item_id) || self.audio_only {
                self.next();
            }
        }
    }

    pub fn position(&self) -> (f64, f64, bool) {
        if self.casting.is_some()
            && let Some(s) = self.cast.status()
        {
            let dur = s.duration_ms.map(|d| d as f64 / 1000.0);
            let local = crate::util::with_lock(&self.status, |s| s.duration);
            return (
                s.position_ms as f64 / 1000.0,
                dur.unwrap_or(local),
                !(s.state == "playing" || s.state == "buffering"),
            );
        }
        crate::util::with_lock(&self.status, |s| (s.pos, s.duration, s.paused))
    }

    /// The full-window player (video).
    pub fn show(&mut self, ui: &mut Ui, p: &Palette) -> PlayerAction {
        let mut action = PlayerAction::None;
        let ctx = ui.ctx().clone();
        // Space plays and pauses. Taken before the controls are drawn: a focused button would
        // also take it as a click (and undo the toggle). Not while typing (the room code).
        if !ctx.text_edit_focused() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
            self.toggle_pause();
            self.controls_until = Instant::now() + Duration::from_secs(3);
        }
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        // Never ask mpv synchronously from the render thread: properties arrive as events.
        let aspect = crate::util::with_lock(&self.status, |s| s.aspect);
        let aspect = if aspect > 0.0 { aspect as f32 } else { 16.0 / 9.0 };
        let mut vr = rect;
        if rect.width() / rect.height() > aspect {
            vr.set_width(rect.height() * aspect);
        } else {
            vr.set_height(rect.width() / aspect);
        }
        let vr = Rect::from_center_size(rect.center(), vr.size());
        let ppp = ctx.pixels_per_point();
        let px = [(vr.width() * ppp).round().max(1.0) as i32, (vr.height() * ppp).round().max(1.0) as i32];
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        match &mut self.video {
            Video::Gl(state, id) => {
                if let Some(id) = *id {
                    let st = state.clone();
                    let cb = eframe::egui_glow::CallbackFn::new(move |_info, _painter| {
                        st.lock().unwrap_or_else(|e| e.into_inner()).paint(px)
                    });
                    ui.painter().add(egui::PaintCallback { rect: vr, callback: Arc::new(cb) });
                    ui.painter().image(id, vr, uv, Color32::WHITE);
                }
            }
            Video::Sw { rc, buf, tex, size } => {
                let want = [px[0] as usize, px[1] as usize];
                if rc.update() || want != *size {
                    buf.resize(want[0] * want[1] * 4, 0);
                    if rc.render_sw(px[0], px[1], buf).is_ok() {
                        let img = egui::ColorImage::from_rgba_unmultiplied(want, buf);
                        match tex {
                            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                            None => *tex = Some(ctx.load_texture("video", img, egui::TextureOptions::LINEAR)),
                        }
                        *size = want;
                    }
                }
                if let Some(t) = tex {
                    ui.painter().image(t.id(), vr, uv, Color32::WHITE);
                }
            }
            Video::None => {}
        }

        // Controls appear on mouse movement and hide after 3 s of stillness (not while paused).
        // The window has no title bar while a video plays: dragging the picture moves it, as in
        // mpv's own window (the edges resize it, see `window_frame`).
        let resp = ui.interact(rect, ui.id().with("player-area"), Sense::click_and_drag());
        if resp.drag_started_by(egui::PointerButton::Primary) && !self.fullscreen {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO)
            || resp.hovered() && ctx.input(|i| i.pointer.any_pressed())
        {
            self.controls_until = Instant::now() + Duration::from_secs(3);
        }
        // A click alone does nothing: it may be the start of a resize at the window's edge, or
        // just bring the window to the front. Space plays and pauses.
        if resp.double_clicked() {
            action = PlayerAction::ToggleFullscreen;
        }
        self.sync_room(&ctx);
        if let Some(c) = &mut self.casting {
            self.cast.poll(c, &ctx);
        }
        let (pos, dur, paused) = self.position();
        self.thumb_tick();
        let show = paused || Instant::now() < self.controls_until;
        let k = crate::ui::motion::toggle(&ctx, ui.id().with("controls"), show, crate::ui::motion::PANEL);
        if !show {
            ctx.request_repaint_after(Duration::from_millis(500));
        } else {
            ctx.request_repaint_after(self.controls_until.saturating_duration_since(Instant::now()));
        }
        let buffering = crate::util::with_lock(&self.status, |s| s.buffering);
        if buffering {
            ui.put(
                Rect::from_center_size(rect.center(), vec2(48.0, 48.0)),
                egui::Spinner::new().size(40.0).color(Color32::WHITE),
            );
        }
        if let Some(err) = crate::util::with_lock(&self.status, |s| s.error.clone()) {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                err,
                egui::FontId::proportional(18.0),
                Color32::WHITE,
            );
        }
        if k > 0.0 {
            action = self.controls(ui, p, rect, k, pos, dur, paused).or(action);
        }
        action = self.skip_button(ui, p, rect, pos, dur, k).or(action);
        self.countdown(ui, p, rect, pos, dur);
        if self.stats {
            self.stats_overlay(ui, rect);
        }
        self.keys(&ctx, &mut action);
        self.extra_keys(&ctx);
        action
    }

    /// mpv's keys for the less common things: `[` `]` speed (Backspace resets), `z`/`x`
    /// subtitle delay, `,`/`.` frame step, `s` screenshot, `i` stats, `n`/`p` next/previous.
    fn extra_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let typed: Vec<String> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect()
        });
        let backspace = ctx.input(|i| i.key_pressed(egui::Key::Backspace));
        if backspace {
            self.cmd(&["set", "speed", "1"]);
        }
        for t in typed {
            match t.as_str() {
                "[" => self.cmd(&["multiply", "speed", "0.9"]),
                "]" => self.cmd(&["multiply", "speed", "1.1"]),
                "z" => self.cmd(&["add", "sub-delay", "-0.1"]),
                "x" => self.cmd(&["add", "sub-delay", "0.1"]),
                "," => self.cmd(&["frame-back-step"]),
                "." => self.cmd(&["frame-step"]),
                "s" => self.screenshot(),
                "i" => self.stats = !self.stats,
                "n" => {
                    self.next();
                }
                "p" => self.previous(),
                _ => {}
            }
        }
    }

    /// Whether the current item can take a thumbnail from its video (a library item played from
    /// the server: not an offline copy, a stream or music).
    fn can_set_thumbnail(&self) -> bool {
        !self.audio_only && self.current().is_some_and(|e| e.local.is_none() && e.stream.is_none() && e.item_id > 0)
    }

    /// The frame at `at` seconds becomes the item's thumbnail (taken on the server from the file,
    /// locked against providers).
    fn set_thumbnail(&mut self, at: f64) {
        let Some(item) = self.current().map(|e| e.item_id) else { return };
        if self.thumb_job.is_some() {
            return;
        }
        let c = self.client.clone();
        let body = serde_json::json!({ "at_ms": (at * 1000.0) as i64, "lock": true });
        self.thumb_job = Some(super::client::fetch(&self.rt, &self.ctx, async move {
            c.send(reqwest::Method::POST, &format!("/api/v1/items/{item}/thumbnail"), &body).await
        }));
    }

    fn thumb_tick(&mut self) {
        let Some(r) = self.thumb_job.as_ref().and_then(super::client::ready) else { return };
        self.thumb_job = None;
        self.notice = Some(match r {
            Ok(_) => (crate::engine::ToastKind::Success, t!("Thumbnail updated.").to_string()),
            Err(e) => (crate::engine::ToastKind::Error, t!("Couldn't set the thumbnail: {why}", why = e)),
        });
    }

    /// A screenshot into the Pictures folder (mpv names the file).
    fn screenshot(&self) {
        let dir = directories::UserDirs::new()
            .and_then(|u| u.picture_dir().map(|p| p.join("Corrent")))
            .unwrap_or_else(std::env::temp_dir);
        let mpv = self.mpv.clone();
        // The folder is made off the UI thread.
        self.rt.spawn_blocking(move || {
            let _ = std::fs::create_dir_all(&dir);
            let _ = mpv.command_async(0, &["set", "screenshot-directory", &dir.display().to_string()]);
            let _ = mpv.command_async(0, &["screenshot", "video"]);
        });
    }

    /// Picture in picture: a small window on top of the others, and back.
    fn toggle_pip(&mut self, ctx: &egui::Context) {
        match self.pip.take() {
            Some(size) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            }
            None => {
                let size = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size())).unwrap_or(vec2(1280.0, 720.0));
                self.pip = Some(size);
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(480.0, 270.0)));
            }
        }
    }

    fn keys(&mut self, ctx: &egui::Context, action: &mut PlayerAction) {
        // T: this frame becomes the thumbnail (not while typing).
        if !ctx.text_edit_focused() && ctx.input(|i| i.key_pressed(egui::Key::T)) && self.can_set_thumbnail() {
            let (pos, _, _) = self.position();
            self.set_thumbnail(pos);
        }
        let (left, right, up, down, f, m, esc) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::M),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if left {
            self.seek(-10.0, true);
        }
        if right {
            self.seek(10.0, true);
        }
        if up {
            self.cmd(&["add", "volume", "5"]);
        }
        if down {
            self.cmd(&["add", "volume", "-5"]);
        }
        if m {
            self.cmd(&["cycle", "mute"]);
        }
        if f {
            *action = PlayerAction::ToggleFullscreen;
        }
        if esc {
            *action = if self.fullscreen { PlayerAction::ToggleFullscreen } else { PlayerAction::Close };
        }
        if left || right || up || down || m {
            self.controls_until = Instant::now() + Duration::from_secs(3);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn controls(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        rect: Rect,
        k: f32,
        pos: f64,
        dur: f64,
        paused: bool,
    ) -> PlayerAction {
        if self.pip.is_some() {
            return self.pip_controls(ui, p, rect, k, pos, dur, paused);
        }
        let mut action = PlayerAction::None;
        let painter = ui.painter();
        let alpha = (k * 255.0) as u8;
        // Top and bottom scrims.
        let top = Rect::from_min_size(rect.min, vec2(rect.width(), 90.0));
        let bottom = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 130.0), rect.max);
        let fade = |r: Rect, down: bool| {
            let mut mesh = egui::Mesh::default();
            let (a, b) = (Color32::from_black_alpha((k * 200.0) as u8), Color32::TRANSPARENT);
            let (c1, c2) = if down { (a, b) } else { (b, a) };
            mesh.colored_vertex(r.left_top(), c1);
            mesh.colored_vertex(r.right_top(), c1);
            mesh.colored_vertex(r.left_bottom(), c2);
            mesh.colored_vertex(r.right_bottom(), c2);
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(1, 2, 3);
            mesh
        };
        painter.add(fade(top, true));
        painter.add(fade(bottom, false));
        let white = Color32::from_white_alpha(alpha);
        if let Some(e) = self.current() {
            painter.text(
                top.left_top() + vec2(28.0, 22.0),
                egui::Align2::LEFT_TOP,
                &e.title,
                egui::FontId::proportional(22.0),
                white,
            );
            if !e.subtitle.is_empty() {
                painter.text(
                    top.left_top() + vec2(28.0, 52.0),
                    egui::Align2::LEFT_TOP,
                    &e.subtitle,
                    egui::FontId::proportional(14.0),
                    Color32::from_white_alpha((k * 180.0) as u8),
                );
            }
        }
        // Close (top right).
        let close = Rect::from_center_size(pos2(rect.right() - 36.0, rect.top() + 36.0), vec2(36.0, 36.0));
        let r = ui.interact(close, ui.id().with("close"), Sense::click());
        ui.painter().text(
            close.center(),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::X,
            egui::FontId::proportional(22.0),
            white,
        );
        if r.clicked() {
            action = PlayerAction::Close;
        }
        // Seek bar.
        let bar = Rect::from_min_max(
            pos2(rect.left() + 28.0, rect.bottom() - 78.0),
            pos2(rect.right() - 28.0, rect.bottom() - 62.0),
        );
        let r = ui.interact(bar, ui.id().with("seek"), Sense::click_and_drag());
        let frac_at = |x: f32| ((x - bar.left()) / bar.width()).clamp(0.0, 1.0);
        if let Some(p) = r.interact_pointer_pos() {
            self.seek_drag = Some(frac_at(p.x));
        }
        if (r.drag_stopped() || r.clicked())
            && let Some(f) = self.seek_drag.take()
        {
            self.seek(f as f64 * dur, false);
        }
        let shown = self.seek_drag.map(|f| f as f64 * dur).unwrap_or(pos);
        let frac = if dur > 0.0 { (shown / dur) as f32 } else { 0.0 };
        let h = if r.hovered() || self.seek_drag.is_some() { 6.0 } else { 4.0 };
        let line = Rect::from_center_size(bar.center(), vec2(bar.width(), h));
        let painter = ui.painter();
        painter.rect_filled(line, CornerRadius::same(3), Color32::from_white_alpha((k * 70.0) as u8));
        painter.rect_filled(
            Rect::from_min_size(line.min, vec2(line.width() * frac.clamp(0.0, 1.0), h)),
            CornerRadius::same(3),
            p.accent.gamma_multiply(k),
        );
        {
            let s = crate::util::lock(&self.status);
            for (c, _) in &s.chapters {
                if dur > 0.0 {
                    let x = line.left() + line.width() * (*c / dur) as f32;
                    painter.rect_filled(
                        Rect::from_center_size(pos2(x, line.center().y), vec2(2.0, h)),
                        0.0,
                        Color32::from_black_alpha(alpha / 2),
                    );
                }
            }
        }
        painter.circle_filled(pos2(line.left() + line.width() * frac.clamp(0.0, 1.0), line.center().y), h + 2.0, white);
        if let Some(hp) = r.hover_pos().filter(|_| dur > 0.0) {
            let t = frac_at(hp.x) as f64 * dur;
            let cue = crate::util::with_lock(&self.trick, |c| cue_at(c, t).cloned());
            if let Some(cue) = cue
                && let Some(tex) = self.sheet(ui.ctx(), &cue.sheet)
            {
                let [x, y, w, h] = cue.rect.map(|v| v as f32);
                let size = tex.size_vec2();
                let shown = vec2(180.0, 180.0 * h / w.max(1.0));
                let center_x = hp.x.clamp(bar.left() + shown.x / 2.0, bar.right() - shown.x / 2.0);
                let thumb = Rect::from_center_size(pos2(center_x, bar.top() - 30.0 - shown.y / 2.0), shown);
                let uv = Rect::from_min_max(pos2(x / size.x, y / size.y), pos2((x + w) / size.x, (y + h) / size.y));
                painter.rect_filled(thumb.expand(2.0), CornerRadius::same(6), Color32::from_black_alpha(alpha));
                painter.image(tex.id(), thumb, uv, Color32::WHITE);
            }
            painter.text(
                pos2(hp.x, bar.top() - 10.0),
                egui::Align2::CENTER_BOTTOM,
                fmt_time(t),
                egui::FontId::proportional(13.0),
                white,
            );
        }
        // Buttons row. A narrow window drops the extras instead of overlapping them: below
        // NARROW the volume slider goes and the time moves above the seek bar, below TINY also
        // previous/next, watch together and cast.
        let (narrow, tiny) = (rect.width() < NARROW, rect.width() < TINY);
        let y = rect.bottom() - 32.0;
        let mut x = rect.left() + 28.0;
        let mut button = |ui: &mut Ui, icon: &str, size: f32, id: &str| -> bool {
            let r = Rect::from_center_size(pos2(x + size / 2.0, y), vec2(size + 8.0, size + 8.0));
            x += size + 18.0;
            let resp = ui.interact(r, ui.id().with(id), Sense::click());
            let c = if resp.hovered() { Color32::WHITE } else { white };
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(size), c);
            resp.clicked()
        };
        use egui_phosphor::regular as pf;
        use egui_phosphor::regular as pr;
        if !tiny && button(ui, pf::SKIP_BACK, 22.0, "prev") {
            self.previous();
        }
        if button(ui, if paused { pf::PLAY } else { pf::PAUSE }, 30.0, "pp") {
            self.toggle_pause();
        }
        if !tiny && button(ui, pf::SKIP_FORWARD, 22.0, "next") {
            self.next();
        }
        let (vol, muted) = crate::util::with_lock(&self.status, |s| (s.volume, s.muted));
        if button(ui, if muted { pr::SPEAKER_X } else { pr::SPEAKER_HIGH }, 22.0, "mute") {
            self.cmd(&["cycle", "mute"]);
        }
        let time = format!("{} / {}", fmt_time(shown), fmt_time(dur));
        // "Use this frame as the thumbnail", right after the time.
        let thumb_at = |ui: &mut Ui, c: egui::Pos2| -> bool {
            let r = Rect::from_center_size(c, vec2(28.0, 28.0));
            let resp = ui
                .interact(r, ui.id().with("set-thumb"), Sense::click())
                .on_hover_text(t!("Use this frame as the thumbnail (T)"));
            let c = if resp.hovered() { Color32::WHITE } else { white };
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                pr::IMAGE_SQUARE,
                egui::FontId::proportional(18.0),
                c,
            );
            resp.clicked()
        };
        let can_thumb = !tiny && self.can_set_thumbnail();
        if narrow {
            let g = ui.painter().layout_no_wrap(time.clone(), egui::FontId::monospace(12.0), white);
            let tw = g.size().x;
            ui.painter().text(
                pos2(bar.right(), bar.top() - 4.0),
                egui::Align2::RIGHT_BOTTOM,
                time,
                egui::FontId::monospace(12.0),
                white,
            );
            if can_thumb && thumb_at(ui, pos2(bar.right() - tw - 20.0, bar.top() - 12.0)) {
                self.set_thumbnail(shown);
            }
        } else {
            // Volume slider.
            let vr = Rect::from_min_size(pos2(x - 8.0, y - 3.0), vec2(90.0, 6.0));
            let resp = ui.interact(vr.expand(8.0), ui.id().with("vol"), Sense::click_and_drag());
            if let Some(pp) = resp.interact_pointer_pos() {
                let v = ((pp.x - vr.left()) / vr.width()).clamp(0.0, 1.0) * 100.0;
                self.cmd(&["set", "volume", &format!("{v:.0}")]);
            }
            ui.painter().rect_filled(vr, CornerRadius::same(3), Color32::from_white_alpha((k * 70.0) as u8));
            ui.painter().rect_filled(
                Rect::from_min_size(vr.min, vec2(vr.width() * (vol as f32 / 100.0).clamp(0.0, 1.0), vr.height())),
                CornerRadius::same(3),
                white,
            );
            x += 106.0;
            let g = ui.painter().layout_no_wrap(time.clone(), egui::FontId::monospace(13.0), white);
            let tw = g.size().x;
            ui.painter().text(pos2(x, y), egui::Align2::LEFT_CENTER, time, egui::FontId::monospace(13.0), white);
            if can_thumb && thumb_at(ui, pos2(x + tw + 22.0, y)) {
                self.set_thumbnail(shown);
            }
        }
        // Right side, from the edge inwards.
        let mut rx = rect.right() - 44.0;
        let mut slot = || {
            let c = pos2(rx, y);
            rx -= 48.0;
            c
        };
        let right = Rect::from_center_size(slot(), vec2(30.0, 30.0));
        let r = ui.interact(right, ui.id().with("fs"), Sense::click());
        ui.painter().text(
            right.center(),
            egui::Align2::CENTER_CENTER,
            if self.fullscreen { pr::CORNERS_IN } else { pr::CORNERS_OUT },
            egui::FontId::proportional(22.0),
            white,
        );
        if r.clicked() {
            action = PlayerAction::ToggleFullscreen;
        }
        let tracks_btn = Rect::from_center_size(slot(), vec2(30.0, 30.0));
        let r = ui.interact(tracks_btn, ui.id().with("tracks"), Sense::click());
        ui.painter().text(
            tracks_btn.center(),
            egui::Align2::CENTER_CENTER,
            pr::SUBTITLES,
            egui::FontId::proportional(22.0),
            white,
        );
        if !tiny {
            self.room_button(ui, slot(), white, paused, pos);
            self.cast_button(ui, slot(), white, pos);
        }
        self.settings_button(ui, slot(), white);
        let pip = Rect::from_center_size(slot(), vec2(30.0, 30.0));
        let pip_r = ui.interact(pip, ui.id().with("pip"), Sense::click()).on_hover_text(t!("Picture in picture"));
        ui.painter().text(
            pip.center(),
            egui::Align2::CENTER_CENTER,
            pr::PICTURE_IN_PICTURE,
            egui::FontId::proportional(22.0),
            white,
        );
        if pip_r.clicked() {
            let ctx = ui.ctx().clone();
            self.toggle_pip(&ctx);
        }
        if let Some(c) = &self.casting {
            let line = match self.cast.status() {
                Some(s) if s.state == "error" => s
                    .error
                    .filter(|e| e != "cant_play")
                    .unwrap_or_else(|| t!("The device couldn’t play this.").to_string()),
                Some(s) if s.state == "connecting" || s.state == "loading" => t!("Connecting…").to_string(),
                _ => String::new(),
            };
            let center = rect.center();
            ui.painter().text(
                center,
                egui::Align2::CENTER_CENTER,
                t!("Playing on {name}", name = c.device.name),
                egui::FontId::proportional(22.0),
                Color32::WHITE,
            );
            if !line.is_empty() {
                ui.painter().text(
                    center + vec2(0.0, 28.0),
                    egui::Align2::CENTER_CENTER,
                    line,
                    egui::FontId::proportional(14.0),
                    Color32::from_white_alpha(200),
                );
            }
        }
        let tracks = crate::util::with_lock(&self.status, |s| s.tracks.clone());
        let mpv = self.mpv.clone();
        let mut pick = false;
        crate::ui::widgets::popup(egui::Popup::from_toggle_button_response(&r), |ui| {
            ui.set_min_width(260.0);
            for (kind, label, prop) in [("audio", t!("Audio"), "aid"), ("sub", t!("Subtitles"), "sid")] {
                ui.label(egui::RichText::new(label).strong());
                if kind == "sub"
                    && ui.selectable_label(!tracks.iter().any(|t| t.kind == "sub" && t.selected), t!("Off")).clicked()
                {
                    let _ = mpv.command_async(0, &["set", "sid", "no"]);
                }
                for tr in tracks.iter().filter(|t| t.kind == kind) {
                    if ui.selectable_label(tr.selected, &tr.title).clicked() {
                        let _ = mpv.command_async(0, &["set", prop, &tr.id.to_string()]);
                    }
                }
                if kind == "sub"
                    && ui.selectable_label(false, format!("{}  {}", pr::FOLDER_OPEN, t!("Load from file…"))).clicked()
                {
                    pick = true;
                }
                ui.separator();
            }
        });
        if pick {
            action = PlayerAction::PickSubtitle;
        }
        action
    }

    /// Picture in picture: a small window, so only what a small window needs. Back to the window
    /// (top left), close (top right), play/pause in the middle and a thin seek bar along the
    /// bottom. Dragging anywhere else moves the window.
    #[allow(clippy::too_many_arguments)]
    fn pip_controls(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        rect: Rect,
        k: f32,
        pos: f64,
        dur: f64,
        paused: bool,
    ) -> PlayerAction {
        use egui_phosphor::regular as pr;
        let mut action = PlayerAction::None;
        let white = Color32::from_white_alpha((k * 255.0) as u8);
        ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha((k * 90.0) as u8));
        let icon_button = |ui: &mut Ui, at: egui::Pos2, icon: &str, size: f32, id: &str, tip: &str| -> bool {
            let r = Rect::from_center_size(at, vec2(size + 12.0, size + 12.0));
            let resp = ui.interact(r, ui.id().with(id), Sense::click()).on_hover_text(tip);
            let c = if resp.hovered() { Color32::WHITE } else { white };
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(size), c);
            resp.clicked()
        };
        let back = icon_button(
            ui,
            rect.left_top() + vec2(24.0, 24.0),
            pr::ARROWS_OUT_SIMPLE,
            18.0,
            "pip-back",
            t!("Back to the window"),
        );
        if back {
            let ctx = ui.ctx().clone();
            self.toggle_pip(&ctx);
        }
        if icon_button(ui, rect.right_top() + vec2(-24.0, 24.0), pr::X, 18.0, "pip-close", t!("Close")) {
            let ctx = ui.ctx().clone();
            self.toggle_pip(&ctx);
            action = PlayerAction::Close;
        }
        let (pp, tip) = if paused { (pr::PLAY, t!("Play")) } else { (pr::PAUSE, t!("Pause")) };
        if icon_button(ui, rect.center(), pp, 40.0, "pip-pp", tip) {
            self.toggle_pause();
        }
        // Seek bar along the bottom edge.
        let bar = Rect::from_min_max(
            pos2(rect.left() + 12.0, rect.bottom() - 18.0),
            pos2(rect.right() - 12.0, rect.bottom() - 6.0),
        );
        let r = ui.interact(bar, ui.id().with("pip-seek"), Sense::click_and_drag());
        let frac_at = |x: f32| ((x - bar.left()) / bar.width()).clamp(0.0, 1.0);
        if let Some(pt) = r.interact_pointer_pos() {
            self.seek_drag = Some(frac_at(pt.x));
        }
        if (r.drag_stopped() || r.clicked())
            && let Some(f) = self.seek_drag.take()
        {
            self.seek(f as f64 * dur, false);
        }
        let shown = self.seek_drag.map(|f| f as f64 * dur).unwrap_or(pos);
        let frac = if dur > 0.0 { (shown / dur) as f32 } else { 0.0 };
        let line = Rect::from_center_size(bar.center(), vec2(bar.width(), 3.0));
        ui.painter().rect_filled(line, CornerRadius::same(2), Color32::from_white_alpha((k * 70.0) as u8));
        ui.painter().rect_filled(
            Rect::from_min_size(line.min, vec2(line.width() * frac.clamp(0.0, 1.0), 3.0)),
            CornerRadius::same(2),
            p.accent.gamma_multiply(k),
        );
        ui.painter().text(
            pos2(bar.left(), bar.top() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{} / {}", fmt_time(shown), fmt_time(dur)),
            egui::FontId::monospace(11.0),
            white,
        );
        action
    }

    /// Speed, delays, subtitle size, picture shape, screenshot and stats.
    fn settings_button(&mut self, ui: &mut Ui, at: egui::Pos2, white: Color32) {
        let r = Rect::from_center_size(at, vec2(30.0, 30.0));
        let resp = ui.interact(r, ui.id().with("psettings"), Sense::click()).on_hover_text(t!("Playback settings"));
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::GEAR_SIX,
            egui::FontId::proportional(22.0),
            white,
        );
        let s = crate::util::with_lock(&self.status, |s| (s.speed, s.sub_delay, s.audio_delay, s.sub_scale));
        let mut cmds: Vec<Vec<String>> = Vec::new();
        let mut shot = false;
        crate::ui::widgets::popup(
            egui::Popup::from_toggle_button_response(&resp)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
            |ui| {
                ui.set_min_width(300.0);
                let row = |ui: &mut Ui,
                           label: &str,
                           value: String,
                           minus: [&str; 3],
                           plus: [&str; 3],
                           cmds: &mut Vec<Vec<String>>| {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("+").clicked() {
                                cmds.push(plus.iter().map(|s| s.to_string()).collect());
                            }
                            ui.label(egui::RichText::new(value).monospace());
                            if ui.small_button("−").clicked() {
                                cmds.push(minus.iter().map(|s| s.to_string()).collect());
                            }
                        });
                    });
                };
                row(
                    ui,
                    t!("Speed"),
                    format!("{:.2}×", s.0),
                    ["multiply", "speed", "0.9"],
                    ["multiply", "speed", "1.1"],
                    &mut cmds,
                );
                row(
                    ui,
                    t!("Subtitle delay"),
                    format!("{:+.1} s", s.1),
                    ["add", "sub-delay", "-0.1"],
                    ["add", "sub-delay", "0.1"],
                    &mut cmds,
                );
                row(
                    ui,
                    t!("Audio delay"),
                    format!("{:+.2} s", s.2),
                    ["add", "audio-delay", "-0.05"],
                    ["add", "audio-delay", "0.05"],
                    &mut cmds,
                );
                row(
                    ui,
                    t!("Subtitle size"),
                    format!("{:.0}%", s.3 * 100.0),
                    ["add", "sub-scale", "-0.1"],
                    ["add", "sub-scale", "0.1"],
                    &mut cmds,
                );
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(t!("Picture"));
                    for (label, aspect) in
                        [(t!("Original"), "-1"), ("16:9", "16:9"), ("4:3", "4:3"), ("2.35:1", "2.35:1")]
                    {
                        if ui.small_button(label).clicked() {
                            cmds.push(vec!["set".into(), "video-aspect-override".into(), aspect.into()]);
                        }
                    }
                    if ui.small_button(t!("Fill")).clicked() {
                        cmds.push(vec!["cycle-values".into(), "panscan".into(), "1.0".into(), "0.0".into()]);
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(t!("Reset")).clicked() {
                        for (k, v) in [
                            ("speed", "1"),
                            ("sub-delay", "0"),
                            ("audio-delay", "0"),
                            ("sub-scale", "1"),
                            ("video-aspect-override", "-1"),
                            ("panscan", "0"),
                        ] {
                            cmds.push(vec!["set".into(), k.into(), v.into()]);
                        }
                    }
                    if ui.button(t!("Screenshot")).clicked() {
                        shot = true;
                    }
                    ui.checkbox(&mut self.stats, t!("Stats"));
                });
            },
        );
        for c in cmds {
            let args: Vec<&str> = c.iter().map(String::as_str).collect();
            self.cmd(&args);
        }
        if shot {
            self.screenshot();
        }
    }

    /// "Stats for nerds": what is playing and how.
    fn stats_overlay(&self, ui: &mut Ui, rect: Rect) {
        let s = crate::util::lock(&self.status);
        let st = &s.stats;
        let text = format!(
            "{} {}×{}  {:.2} fps  {}\n{}\n{} {} · {} {:.1} s · {} {}",
            st.video_codec,
            st.width,
            st.height,
            st.fps,
            if s.hwdec.is_empty() || s.hwdec == "no" { t!("software decoding").to_string() } else { s.hwdec.clone() },
            st.audio_codec,
            t!("Speed"),
            format_args!("{:.2}×", s.speed),
            t!("Buffered"),
            st.cache_secs,
            t!("Dropped frames"),
            st.dropped,
        );
        let pos = rect.left_top() + vec2(20.0, 90.0);
        let galley = ui.painter().layout_no_wrap(text, egui::FontId::monospace(12.0), Color32::WHITE);
        let bg = Rect::from_min_size(pos - vec2(8.0, 6.0), galley.size() + vec2(16.0, 12.0));
        ui.painter().rect_filled(bg, CornerRadius::same(6), Color32::from_black_alpha(170));
        ui.painter().galley(pos, galley, Color32::WHITE);
    }

    /// The last seconds of an episode: what comes next, with "Play now" and "Cancel".
    fn countdown(&mut self, ui: &mut Ui, p: &Palette, rect: Rect, pos: f64, dur: f64) {
        let Some(cur) = self.current().map(|e| e.item_id) else { return };
        let Some(next) = self.queue.get(self.index + 1).cloned() else { return };
        if self.audio_only || dur <= 0.0 || dur - pos > 10.0 || self.no_countdown == Some(cur) || self.casting.is_some()
        {
            return;
        }
        let left = (dur - pos).ceil().max(0.0) as i64;
        let area = Rect::from_min_size(pos2(rect.right() - 380.0, rect.bottom() - 220.0), vec2(350.0, 90.0));
        let mut play = false;
        let mut cancel = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            egui::Frame::new()
                .fill(Color32::from_black_alpha(200))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(t!("Up next in {n} s", n = left)).color(p.text_dim));
                    ui.label(egui::RichText::new(&next.title).strong().color(Color32::WHITE));
                    ui.horizontal(|ui| {
                        if ui.button(t!("Play now")).clicked() {
                            play = true;
                        }
                        if ui.button(t!("Cancel")).clicked() {
                            cancel = true;
                        }
                    });
                });
        });
        if play {
            self.next();
        }
        if cancel {
            self.no_countdown = Some(cur);
        }
    }

    /// "Cast to a device": the devices the server found; casting pauses the local player.
    fn cast_button(&mut self, ui: &mut Ui, at: egui::Pos2, white: Color32, pos: f64) {
        let r = Rect::from_center_size(at, vec2(30.0, 30.0));
        let resp = ui.interact(r, ui.id().with("cast"), Sense::click()).on_hover_text(t!("Cast to a device"));
        let c = if self.casting.is_some() { Color32::from_rgb(120, 190, 255) } else { white };
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            egui_phosphor::regular::SCREENCAST,
            egui::FontId::proportional(22.0),
            c,
        );
        if resp.clicked() && self.casting.is_none() {
            self.cast.find(ui.ctx(), false);
        }
        let item = self.current().map(|e| e.item_id);
        crate::ui::widgets::popup(egui::Popup::from_toggle_button_response(&resp), |ui| {
            ui.set_min_width(260.0);
            ui.label(egui::RichText::new(t!("Cast to")).strong());
            if let Some(c) = &self.casting {
                ui.label(t!("Playing on {name}", name = c.device.name));
                if ui.button(t!("Stop casting")).clicked() {
                    self.cast.command(c, CastCommand::Stop);
                    let at = self.cast.status().map(|s| s.position_ms as f64 / 1000.0).unwrap_or(pos);
                    self.casting = None;
                    self.cmd(&["seek", &format!("{at}"), "absolute"]);
                }
                return;
            }
            match self.cast.devices() {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t!("Looking for devices…"));
                    });
                }
                Some(Err(e)) => {
                    ui.colored_label(Color32::LIGHT_RED, e);
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(t!("No Chromecasts found on the server’s network."));
                }
                Some(Ok(list)) => {
                    for d in list {
                        let label =
                            if d.model.is_empty() { d.name.clone() } else { format!("{}  ·  {}", d.name, d.model) };
                        if let Some(item) = item
                            && ui.selectable_label(false, label).clicked()
                        {
                            self.cmd(&["set", "pause", "yes"]);
                            self.casting = Some(self.cast.start(d, item, pos));
                        }
                    }
                }
            }
            if ui.button(t!("Look again")).clicked() {
                self.cast.find(ui.ctx(), true);
            }
        });
    }

    /// "Watch together": start a room, join one by its code, copy its link, leave.
    fn room_button(&mut self, ui: &mut Ui, at: egui::Pos2, white: Color32, paused: bool, pos: f64) {
        let r = Rect::from_center_size(at, vec2(30.0, 30.0));
        let resp = ui.interact(r, ui.id().with("room"), Sense::click()).on_hover_text(t!("Watch together"));
        let icon =
            if self.room.is_some() { egui_phosphor::regular::USERS_THREE } else { egui_phosphor::regular::USERS };
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(22.0), white);
        let item = self.current().map(|e| e.item_id);
        crate::ui::widgets::popup(
            egui::Popup::from_toggle_button_response(&resp)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
            |ui| {
                ui.set_min_width(280.0);
                ui.label(egui::RichText::new(t!("Watch together")).strong());
                if let Some(room) = &self.room {
                    match room.state() {
                        Some(s) => {
                            ui.label(t!("Room {code}", code = s.id));
                            let n = s.members.len();
                            ui.label(if n == 1 { t!("Only you so far").to_string() } else { t!("{n} people", n = n) });
                            if Some(s.item_id) != item {
                                ui.label(t!("The room is watching something else."));
                            }
                            if ui.button(t!("Copy link")).clicked()
                                && let Some(l) = room.link()
                            {
                                ui.ctx().copy_text(l);
                            }
                        }
                        None => match room.error() {
                            Some(e) => {
                                ui.colored_label(Color32::LIGHT_RED, e);
                            }
                            None => {
                                ui.spinner();
                            }
                        },
                    }
                    if ui.button(t!("Leave the room")).clicked() {
                        self.room = None;
                    }
                } else {
                    ui.label(t!("Play, pause and seek together with others, wherever they watch."));
                    if let Some(item) = item
                        && ui.button(t!("Start a room")).clicked()
                    {
                        self.room = Some(super::syncplay::Room::create(
                            self.client.clone(),
                            self.rt.clone(),
                            ui.ctx().clone(),
                            item,
                            pos,
                            paused,
                        ));
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.room_code)
                                .hint_text(t!("Room code"))
                                .desired_width(120.0),
                        );
                        if ui.add_enabled(!self.room_code.trim().is_empty(), egui::Button::new(t!("Join"))).clicked() {
                            self.room = Some(super::syncplay::Room::join(
                                self.client.clone(),
                                self.rt.clone(),
                                ui.ctx().clone(),
                                &self.room_code,
                            ));
                            self.room_code.clear();
                        }
                    });
                }
            },
        );
    }

    /// The bottom bar while music plays and the UI is browsing.
    pub fn mini(&mut self, ui: &mut Ui, p: &Palette) -> PlayerAction {
        let mut action = PlayerAction::None;
        let (pos, dur, paused) = self.position();
        ui.horizontal(|ui| {
            use egui_phosphor::regular as pf;
            if ui.add(egui::Button::new(egui::RichText::new(pf::SKIP_BACK).size(18.0)).frame(false)).clicked() {
                self.previous();
            }
            if ui
                .add(
                    egui::Button::new(egui::RichText::new(if paused { pf::PLAY } else { pf::PAUSE }).size(24.0))
                        .frame(false),
                )
                .clicked()
            {
                self.toggle_pause();
            }
            if ui.add(egui::Button::new(egui::RichText::new(pf::SKIP_FORWARD).size(18.0)).frame(false)).clicked() {
                self.next();
            }
            ui.add_space(8.0);
            if let Some(e) = self.current() {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&e.title).strong().color(p.text));
                    ui.label(egui::RichText::new(&e.subtitle).small().color(p.text_dim));
                });
            }
            ui.add_space(12.0);
            let w = (ui.available_width() - 160.0).max(80.0);
            let (r, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click());
            let line = Rect::from_center_size(r.center(), vec2(r.width(), 4.0));
            ui.painter().rect_filled(line, CornerRadius::same(2), p.bg3);
            let frac = if dur > 0.0 { (pos / dur) as f32 } else { 0.0 };
            ui.painter().rect_filled(
                Rect::from_min_size(line.min, vec2(line.width() * frac.clamp(0.0, 1.0), 4.0)),
                CornerRadius::same(2),
                p.accent,
            );
            if let Some(pp) = resp.interact_pointer_pos()
                && resp.clicked()
            {
                self.seek(((pp.x - r.left()) / r.width()).clamp(0.0, 1.0) as f64 * dur, false);
            }
            ui.label(
                egui::RichText::new(format!("{} / {}", fmt_time(pos), fmt_time(dur))).monospace().color(p.text_dim),
            );
            let lr = ui
                .add(
                    egui::Button::new(egui::RichText::new(egui_phosphor::regular::MICROPHONE_STAGE).size(16.0))
                        .frame(false),
                )
                .on_hover_text(t!("Lyrics"));
            self.lyrics_popup(&lr, p, pos);
            if ui
                .add(egui::Button::new(egui::RichText::new(egui_phosphor::regular::X).size(16.0)).frame(false))
                .clicked()
            {
                action = PlayerAction::Close;
            }
        });
        action
    }

    /// Lyrics of the current track, the line at `pos` highlighted (synced ones scroll along).
    fn lyrics_popup(&mut self, button: &egui::Response, p: &Palette, pos: f64) {
        let Some(item) = self.current().map(|e| e.item_id) else { return };
        let open = egui::Popup::is_id_open(&button.ctx, egui::Popup::default_response_id(button));
        if (open || button.clicked()) && self.lyrics.as_ref().is_none_or(|(i, _)| *i != item) {
            let c = self.client.clone();
            let slot = super::client::fetch(&self.rt, &button.ctx, async move {
                c.get(&format!("/api/v1/items/{item}/lyrics")).await
            });
            self.lyrics = Some((item, slot));
        }
        let got = self.lyrics.as_ref().and_then(|(_, s)| super::client::ready(s));
        crate::ui::widgets::popup(egui::Popup::from_toggle_button_response(button), |ui| {
            ui.set_width(340.0);
            ui.label(egui::RichText::new(t!("Lyrics")).strong());
            match got {
                None => {
                    ui.spinner();
                }
                Some(Err(_)) => {
                    ui.label(egui::RichText::new(t!("No lyrics for this track.")).color(p.text_dim));
                }
                Some(Ok(l)) => {
                    let now = (pos * 1000.0) as i64;
                    let at =
                        if l.synced { l.lines.iter().rposition(|x| x.time_ms.is_some_and(|t| t <= now)) } else { None };
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        for (i, line) in l.lines.iter().enumerate() {
                            let text = if line.text.is_empty() { "♪" } else { &line.text };
                            let rich = if Some(i) == at {
                                egui::RichText::new(text).strong().color(p.text)
                            } else {
                                egui::RichText::new(text).color(p.text_dim)
                            };
                            let r = ui.label(rich);
                            if Some(i) == at {
                                r.scroll_to_me(Some(egui::Align::Center));
                            }
                        }
                    });
                    if l.synced {
                        ui.ctx().request_repaint_after(Duration::from_millis(250));
                    }
                }
            }
        });
    }

    /// A trickplay sheet as a texture: fetched and decoded in the background on first use.
    fn sheet(&mut self, ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
        let entry = self.sheets.entry(path.to_string()).or_insert_with(|| {
            let (client, path) = (self.client.clone(), path.to_string());
            Sheet::Loading(super::client::fetch(&self.rt, ctx, async move {
                let bytes = client.bytes(&path).await?;
                tokio::task::spawn_blocking(move || {
                    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.to_rgba8();
                    Ok(egui::ColorImage::from_rgba_unmultiplied(
                        [img.width() as usize, img.height() as usize],
                        img.as_raw(),
                    ))
                })
                .await
                .map_err(|e| e.to_string())?
            }))
        });
        if let Sheet::Loading(slot) = entry {
            match super::client::ready(slot) {
                None => return None,
                Some(Ok(img)) => {
                    *entry = Sheet::Ready(ctx.load_texture(format!("trick-{path}"), img, Default::default()))
                }
                Some(Err(_)) => *entry = Sheet::Failed,
            }
        }
        match entry {
            Sheet::Ready(t) => Some(t.clone()),
            _ => None,
        }
    }

    /// "Skip intro / recap / credits" while such a chapter plays (shown even with the controls
    /// hidden; above them when they are up).
    fn skip_button(&mut self, ui: &mut Ui, p: &Palette, rect: Rect, pos: f64, dur: f64, k: f32) -> PlayerAction {
        use corrent_proto::library::SkipKind;
        let (chapters, segments) = crate::util::with_lock(&self.status, |s| (s.chapters.clone(), s.segments.clone()));
        let skip = corrent_proto::library::skip_at(&chapters, pos, dur)
            .or_else(|| corrent_proto::library::segment_at(&segments, pos));
        let Some((kind, end)) = skip else { return PlayerAction::None };
        let label = match kind {
            SkipKind::Intro => t!("Skip intro"),
            SkipKind::Recap => t!("Skip recap"),
            SkipKind::Credits => t!("Skip credits"),
        };
        let lift = 96.0 * k;
        let at = egui::pos2(rect.right() - 40.0, rect.bottom() - 40.0 - lift);
        let mut action = PlayerAction::None;
        egui::Area::new(ui.id().with("skip"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .show(ui.ctx(), |ui| {
                let b = egui::Button::new(
                    egui::RichText::new(format!("{}  {label}", egui_phosphor::regular::SKIP_FORWARD)).size(16.0),
                )
                .fill(Color32::from_black_alpha(190))
                .stroke(egui::Stroke::new(1.0, p.accent))
                .min_size(vec2(150.0, 40.0));
                if ui.add(b).clicked() {
                    // Credits at the end: on to the next item when there is one.
                    if kind == SkipKind::Credits && dur - end < 5.0 && self.next() {
                        return;
                    }
                    self.seek(end, false);
                    action = PlayerAction::None;
                }
            });
        action
    }

    /// Registers the native texture (needs eframe's `Frame`).
    pub fn register(&mut self, frame: &mut eframe::Frame) {
        if let (Some(tex), Video::Gl(_, id)) = (self.needs_registration.take(), &mut self.video) {
            *id = Some(frame.register_native_glow_texture(tex));
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.report(true);
        // Nothing here may block the UI thread: a stalled network read can hold mpv up.
        let _ = self.mpv.command_async(0, &["stop"]);
        self.stop_events.store(true, Ordering::Relaxed);
        self.mpv.wakeup();
        // The last reference (and `mpv_terminate_destroy`) goes to a helper thread once the
        // render context, dropped with `video`, has let go of its own.
        let mpv = self.mpv.clone();
        let _ = std::thread::Builder::new().name("corrent-mpv-close".into()).spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(mpv);
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerAction {
    None,
    Close,
    ToggleFullscreen,
    /// "Load from file…": the app opens a picker and hands the file to `add_subtitle`.
    PickSubtitle,
}

impl PlayerAction {
    fn or(self, other: PlayerAction) -> PlayerAction {
        if self == PlayerAction::None { other } else { self }
    }
}

fn create_target(gl: &glow::Context) -> Result<(glow::Framebuffer, glow::Texture), String> {
    // SAFETY: called from the UI thread while eframe's GL context is current.
    unsafe {
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        for (p, v) in [
            (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
        }
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            16,
            16,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        Ok((fbo, tex))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trickplay_index() {
        let vtt = "WEBVTT\n\n00:00:00.000 --> 00:00:10.000\nsheet-0.jpg#xywh=0,0,160,90\n\n00:00:10.000 --> 00:00:20.000\nsheet-0.jpg#xywh=160,0,160,90\n";
        let cues = parse_trickplay(vtt, "/api/v1/files/7/trickplay/index.vtt");
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[1].sheet, "/api/v1/files/7/trickplay/sheet-0.jpg");
        assert_eq!(cues[1].rect, [160, 0, 160, 90]);
        assert_eq!(cue_at(&cues, 12.0).map(|c| c.rect[0]), Some(160));
        assert!(cue_at(&cues, 25.0).is_none());
    }
}
