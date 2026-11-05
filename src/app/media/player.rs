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
    /// The tracks chosen last time (`Stream::index`; subtitle 0 = off).
    pub audio_track: Option<i64>,
    pub subtitle_track: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub selected: bool,
    /// The file or URL of a subtitle added with `sub-add`.
    pub external: Option<String>,
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
    /// The entry that was playing reached its end, and mpv has moved on or is about to.
    entry_ended: bool,
    /// `playlist-playing-pos`.
    playlist_pos: i64,
    buffering: bool,
    error: Option<String>,
    hwdec: String,
    aspect: f64,
    speed: f64,
    sub_delay: f64,
    audio_delay: f64,
    sub_scale: f64,
    stats: Stats,
    /// Tracks to select once the file is loaded (the choice remembered for the item).
    want: Option<(Option<i64>, Option<i64>)>,
    loaded: bool,
    /// The server's external subtitles added to mpv: (URL, `Stream::index`).
    ext_subs: Vec<(String, i64)>,
    /// What the filter chain needs to know about the source (kept between files).
    source: corrent_core::audio_fx::Source,
    meters: super::sound::Meters,
    /// The metering filters run, the Audio page is up, the A/V page is up, the controls are up.
    meters_on: bool,
    meter_page: bool,
    stats_page: bool,
    controls_visible: bool,
    /// The whole second of `pos` last drawn.
    shown_second: i64,
    /// When the metering filters were last read.
    meter_read: Option<Instant>,
}

impl Status {
    /// Selects the remembered tracks when the file's tracks are known (those it still has).
    /// External subtitles are selected as they are added.
    fn apply_wanted(&mut self, mpv: &Mpv) {
        if !self.loaded || self.tracks.is_empty() {
            return;
        }
        let Some((aid, sid)) = self.want.take() else { return };
        let has =
            |kind: &str, id: i64| self.tracks.iter().any(|t| t.kind == kind && t.id == id && t.external.is_none());
        if let Some(a) = aid.filter(|a| has("audio", *a)) {
            let _ = mpv.command_async(0, &["set", "aid", &a.to_string()]);
        }
        match sid {
            Some(0) => {
                let _ = mpv.command_async(0, &["set", "sid", "no"]);
            }
            Some(s) if has("sub", s) => {
                let _ = mpv.command_async(0, &["set", "sid", &s.to_string()]);
            }
            _ => {}
        }
    }
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
    /// `video-params`: what a nerd wants to see of the picture.
    pixelformat: String,
    primaries: String,
    gamma: String,
    sig_peak: f64,
    container_fps: f64,
    display_fps: f64,
    vsync_jitter: f64,
    decoder_drops: i64,
    vo_delayed: i64,
    avsync: f64,
    cache_bytes: i64,
    input_rate: f64,
    video_bitrate: f64,
    audio_bitrate: f64,
    audio_in: String,
    audio_out: String,
    current_ao: String,
    /// The labels of the filters mpv actually runs.
    chain: Vec<String>,
    mpv_version: String,
    ffmpeg_version: String,
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
            // Feeds mpv's timing statistics (display fps, jitter on the A/V page).
            self.rc.report_swap();
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
    /// Showing the Audio page (the metering filters run only while it is up).
    meter_page: bool,
    /// The extra properties are observed from the first time a page needs them, so that a player
    /// nobody asks anything of doesn't get them at all.
    observing_meters: bool,
    observing_stats: bool,
    /// The sound sheet is up.
    sound_open: bool,
    /// CORRENT_DEV_WINDOW=<w>x<h> or `pip`: the layout to check, once.
    #[cfg(debug_assertions)]
    dev_window: Option<String>,
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
    /// The sound settings mpv has, to send only what changes.
    audio: Mutex<corrent_core::config::Playback>,
    /// The filters running in mpv, what they were made from, and when `af` was last set.
    fx_applied: Vec<corrent_core::audio_fx::Stage>,
    fx_from: Option<FxInputs>,
    fx_set_at: Option<Instant>,
    /// The position last reported: a paused player has nothing new to say.
    last_reported: f64,
    /// The queue index of the entry at position 0 of mpv's playlist.
    playlist_start: usize,
    /// The queue index whose `loadfile append` was started, and that it was sent (it asks the
    /// server first).
    appending: Option<usize>,
    appended: Arc<std::sync::atomic::AtomicI64>,
    /// Counts replaced playlists: an append still on its way after one is dropped.
    playlist_generation: Arc<std::sync::atomic::AtomicU64>,
    /// The effects last handed over to be saved, and since when they differ.
    fx_saved: Mutex<corrent_core::config::Playback>,
    fx_moved: Option<Instant>,
    /// `fx.meters` as last seen: changing that setting brings the Audio page up or down.
    fx_meters_setting: bool,
    /// The volume and mute last handed over to be saved, and since when they differ.
    volume_saved: (f64, bool),
    volume_moved: Option<Instant>,
}

/// What the filter chain is built from.
#[derive(Clone, Debug, PartialEq)]
struct FxInputs {
    fx: corrent_core::audio_fx::AudioFx,
    passthrough: bool,
    source: corrent_core::audio_fx::Source,
    caps: &'static corrent_core::audio_fx::Caps,
}

/// `Event::CommandReply` of a filter change: a failure is worth a line in the log.
const FX_REPLY: u64 = 7;

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
                external: t.get("external-filename").and_then(Node::as_str).map(str::to_string),
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
            // Pausing at the end would carry over to the next file; `tick` pauses where it stays.
            ("keep-open-pause", "no"),
            ("hwdec", "auto-safe"),
            ("http-header-fields", header.as_str()),
            ("audio-display", "no"),
        ]);
        let audio_opts = audio.mpv_options();
        opts.extend(audio_opts.iter().map(|(k, v)| (*k, v.as_str())));
        // As it was left: no film at 100 % after a late evening at 35 %.
        let volume = format!("{}", if audio.volume.is_finite() { audio.volume.clamp(0.0, 130.0) } else { 100.0 });
        opts.extend([("volume", volume.as_str()), ("mute", if audio.muted { "yes" } else { "no" })]);
        opts.push(if audio_only { ("vid", "no") } else { ("vo", "libmpv") });
        // The texture is 8 bits a channel: HDR tone-mapped or 10-bit sources would band without.
        opts.push(("dither-depth", "8"));
        // Albums go through mpv's own playlist, opened ahead of time: see `gapless_tick`.
        if audio_only && audio.gapless {
            opts.push(("prefetch-playlist", "yes"));
        }
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
        let status = Arc::new(Mutex::new(Status {
            volume: audio.volume as f64,
            muted: audio.muted,
            speed: 1.0,
            sub_scale: 1.0,
            ..Default::default()
        }));
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
            "audio-params",
            "eof-reached",
            "playlist-playing-pos",
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
                    let (wait, poll) = {
                        let s = st.lock().unwrap_or_else(|e| e.into_inner());
                        let late = s.meter_read.is_none_or(|t| t.elapsed() > Duration::from_millis(300));
                        (if s.meters_on { 0.1 } else { 0.5 }, s.meters_on && late)
                    };
                    // Metadata normally arrives as events (about 10 a second); where a libmpv
                    // doesn't send them, it is read instead. This thread may wait for mpv.
                    if poll {
                        let (input, output) = (m.get("af-metadata/in"), m.get("af-metadata/out"));
                        let mut s = st.lock().unwrap_or_else(|e| e.into_inner());
                        s.meter_read = Some(Instant::now());
                        if let Ok(v) = input {
                            s.meters.read_input(&v);
                        }
                        if let Ok(v) = output {
                            s.meters.read_output(&v, Instant::now());
                        }
                        if s.meter_page {
                            c2.request_repaint();
                        }
                    }
                    let Some(ev) = m.wait_event(wait) else { continue };
                    let mut s = st.lock().unwrap_or_else(|e| e.into_inner());
                    let mut repaint = true;
                    match ev {
                        Event::Shutdown => break,
                        Event::PropertyChange { name, value, .. } => {
                            repaint = needs_repaint(&name, &value, &mut s);
                            match name.as_str() {
                                "time-pos" => s.pos = value.as_f64().unwrap_or(s.pos),
                                // With `keep-open` the last entry doesn't end with `EndFile`.
                                "eof-reached" if value.as_bool() == Some(true) => s.ended = true,
                                "playlist-playing-pos" => s.playlist_pos = value.as_i64().unwrap_or(-1),
                                "duration" => s.duration = value.as_f64().unwrap_or(0.0),
                                "pause" => s.paused = value.as_bool().unwrap_or(false),
                                "volume" => s.volume = value.as_f64().unwrap_or(100.0),
                                "mute" => s.muted = value.as_bool().unwrap_or(false),
                                "track-list" => {
                                    s.tracks = track_list(&value);
                                    s.apply_wanted(&m);
                                }
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
                                "audio-params" => {
                                    if let Some(channels) = value.get("channel-count").and_then(Node::as_i64) {
                                        s.source = corrent_core::audio_fx::Source {
                                            channels: Some(channels),
                                            layout: value.get("channels").and_then(Node::as_str).map(str::to_string),
                                        };
                                    }
                                    s.stats.audio_in = super::sound::audio_format(&value);
                                }
                                "audio-out-params" => s.stats.audio_out = super::sound::audio_format(&value),
                                "af-metadata/in" => {
                                    s.meter_read = Some(Instant::now());
                                    s.meters.read_input(&value);
                                }
                                "af-metadata/out" => {
                                    s.meter_read = Some(Instant::now());
                                    s.meters.read_output(&value, Instant::now());
                                }
                                "af" => {
                                    s.stats.chain = value
                                        .as_array()
                                        .iter()
                                        .filter_map(|f| f.get("label").and_then(Node::as_str).map(str::to_string))
                                        .collect();
                                }
                                "current-ao" => s.stats.current_ao = value.as_str().unwrap_or("").to_string(),
                                "video-params/pixelformat" => {
                                    s.stats.pixelformat = value.as_str().unwrap_or("").to_string()
                                }
                                "video-params/primaries" => {
                                    s.stats.primaries = value.as_str().unwrap_or("").to_string()
                                }
                                "video-params/gamma" => s.stats.gamma = value.as_str().unwrap_or("").to_string(),
                                "video-params/sig-peak" => s.stats.sig_peak = value.as_f64().unwrap_or(0.0),
                                "container-fps" => s.stats.container_fps = value.as_f64().unwrap_or(0.0),
                                "estimated-display-fps" => s.stats.display_fps = value.as_f64().unwrap_or(0.0),
                                "vsync-jitter" => s.stats.vsync_jitter = value.as_f64().unwrap_or(0.0),
                                "decoder-frame-drop-count" => s.stats.decoder_drops = value.as_i64().unwrap_or(0),
                                "vo-delayed-frame-count" => s.stats.vo_delayed = value.as_i64().unwrap_or(0),
                                "avsync" => s.stats.avsync = value.as_f64().unwrap_or(0.0),
                                "demuxer-cache-state" => {
                                    s.stats.cache_bytes = value.get("fw-bytes").and_then(Node::as_i64).unwrap_or(0);
                                    s.stats.input_rate =
                                        value.get("raw-input-rate").and_then(Node::as_f64).unwrap_or(0.0);
                                }
                                "video-bitrate" => s.stats.video_bitrate = value.as_f64().unwrap_or(0.0),
                                "audio-bitrate" => s.stats.audio_bitrate = value.as_f64().unwrap_or(0.0),
                                "mpv-version" => s.stats.mpv_version = value.as_str().unwrap_or("").to_string(),
                                "ffmpeg-version" => s.stats.ffmpeg_version = value.as_str().unwrap_or("").to_string(),
                                _ => {}
                            }
                        }
                        Event::FileLoaded => {
                            s.ended = false;
                            s.error = None;
                            s.loaded = true;
                            s.apply_wanted(&m);
                        }
                        Event::CommandReply { userdata: FX_REPLY, error } if error < 0 => {
                            tracing::warn!("sound filter change: {}", m.library().error_text(error))
                        }
                        Event::EndFile(EndReason::Eof) => s.entry_ended = true,
                        Event::EndFile(EndReason::Error(code)) => {
                            s.error = Some(t!("Playback failed: {why}", why = m.library().error_text(code)))
                        }
                        _ => {}
                    }
                    drop(s);
                    if repaint {
                        c2.request_repaint();
                    }
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
            stats: cfg!(debug_assertions) && std::env::var_os("CORRENT_DEV_METERS").is_some(),
            // CORRENT_DEV_METERS: both pages up.
            meter_page: audio.fx.meters || (cfg!(debug_assertions) && std::env::var_os("CORRENT_DEV_METERS").is_some()),
            observing_meters: false,
            observing_stats: false,
            // CORRENT_DEV_SOUND: with the sheet up.
            sound_open: cfg!(debug_assertions) && std::env::var_os("CORRENT_DEV_SOUND").is_some(),
            #[cfg(debug_assertions)]
            dev_window: std::env::var("CORRENT_DEV_WINDOW").ok(),
            pip: None,
            no_countdown: None,
            cast,
            casting: None,
            lyrics: None,
            thumb_job: None,
            notice: None,
            ctx: ctx.clone(),
            offline: None,
            audio: Mutex::new(audio.clone()),
            fx_applied: audio.start_chain(),
            fx_from: None,
            fx_set_at: None,
            last_reported: -1.0,
            playlist_start: 0,
            appending: None,
            appended: Arc::new(std::sync::atomic::AtomicI64::new(-1)),
            playlist_generation: Arc::default(),
            fx_saved: Mutex::new(audio.clone()),
            fx_meters_setting: audio.fx.meters,
            fx_moved: None,
            volume_saved: (audio.volume as f64, audio.muted),
            volume_moved: None,
        };
        p.load_current();
        Ok(p)
    }

    pub fn current(&self) -> Option<&QueueEntry> {
        self.queue.get(self.index)
    }

    fn load_current(&mut self) {
        let Some(e) = self.current().cloned() else { return };
        self.playlist_start = self.index;
        self.appending = None;
        self.appended.store(-1, Ordering::Relaxed);
        self.playlist_generation.fetch_add(1, Ordering::Relaxed);
        {
            let mut s = crate::util::lock(&self.status);
            s.playlist_pos = -1;
            s.entry_ended = false;
            s.tracks.clear();
            s.ext_subs.clear();
            s.loaded = false;
            // External subtitles (1000 and up) only come from the server, selected as added.
            let sid = e.subtitle_track.filter(|i| *i < 1000);
            s.want = (e.stream.is_none() && (e.audio_track.is_some() || sid.is_some())).then_some((e.audio_track, sid));
        }
        if let Some(url) = &e.stream {
            let title = format!("force-media-title=%{}%{}", e.title.len(), e.title);
            let (mpv, url) = (self.mpv.clone(), url.clone());
            // `loadfile_args` asks mpv the first time, which isn't for the UI thread.
            self.rt.spawn_blocking(move || {
                let args = mpv.loadfile_args(&url, &title);
                let _ = mpv.command_async(0, &args.iter().map(String::as_str).collect::<Vec<_>>());
            });
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
            let (mpv, path) = (self.mpv.clone(), path.to_string_lossy().into_owned());
            self.rt.spawn_blocking(move || {
                let args = mpv.loadfile_args(&path, &format!("{start},{title}"));
                let _ = mpv.command_async(0, &args.iter().map(String::as_str).collect::<Vec<_>>());
            });
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
                    let args = mpv.loadfile_args(&url, &format!("{start},{title}"));
                    let _ = mpv.command(&args.iter().map(String::as_str).collect::<Vec<_>>());
                    for s in info.subtitles.iter().filter_map(|s| s.url.as_ref().map(|u| (u, s))) {
                        let url = client.url(s.0);
                        crate::util::lock(&status).ext_subs.push((url.clone(), s.1.index));
                        // mpv opens the URL during `sub-add`: don't hold a runtime worker for it.
                        let _ = mpv.command_async(
                            0,
                            &[
                                "sub-add",
                                &url,
                                if e.subtitle_track == Some(s.1.index) { "select" } else { "auto" },
                                s.1.title.as_deref().unwrap_or(""),
                                s.1.lang.as_deref().unwrap_or(""),
                            ],
                        );
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

    /// Remembers a track chosen in the menu for the item (`None`: subtitles off). A subtitle file
    /// from this computer isn't remembered (the server doesn't know it).
    fn remember_track(&mut self, kind: &str, track: Option<&Track>) {
        let index = match track {
            None => Some(0),
            Some(t) => match &t.external {
                Some(f) => {
                    crate::util::with_lock(&self.status, |s| s.ext_subs.iter().find(|(u, _)| u == f).map(|(_, i)| *i))
                }
                None => Some(t.id),
            },
        };
        let Some(index) = index else { return };
        let Some(e) = self.queue.get_mut(self.index).filter(|e| e.stream.is_none() && e.item_id > 0) else { return };
        let body = if kind == "audio" {
            e.audio_track = Some(index);
            serde_json::json!({ "audio_track": index })
        } else {
            e.subtitle_track = Some(index);
            serde_json::json!({ "subtitle_track": index })
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
    }

    /// Commands from the UI thread are asynchronous: the UI never waits for mpv's core.
    fn cmd(&self, args: &[&str]) {
        let _ = self.mpv.command_async(0, args);
    }

    /// New sound settings while playing: only what changed, as a device option re-creates the
    /// audio output (an audible dropout). The filters follow on the next frame (`sync_fx`).
    pub fn apply_audio(&self, audio: &corrent_core::config::Playback) {
        *crate::util::lock(&self.fx_saved) = audio.clone();
        let mut current = crate::util::lock(&self.audio);
        for (k, v) in audio.changed_options(&current) {
            self.cmd(&["set", k, &v]);
        }
        *current = audio.clone();
        self.ctx.request_repaint();
    }

    /// Brings mpv's filters to what the settings say, at most once a frame, so that a dragged
    /// slider sends one command per value it passes: a value through `af-command`, and `af` is set
    /// only for a change of structure. Nothing here waits for mpv. Does nothing at all when nothing
    /// changed, or runs when everything is off.
    fn sync_fx(&mut self) {
        let (mut fx, passthrough) = crate::util::with_lock(&self.audio, |a| (a.fx.clone(), a.passthrough));
        if fx.meters != self.fx_meters_setting {
            self.fx_meters_setting = fx.meters;
            self.meter_page = fx.meters;
        }
        let source = crate::util::with_lock(&self.status, |s| s.source.clone());
        // The filters follow the page, and wait for the source: an output opened by the source's
        // own format keeps it when they are added.
        fx.meters = self.meter_page && source.channels.is_some();
        self.observe_pages();
        let inputs = FxInputs { fx, passthrough, source, caps: super::sound::caps() };
        if self.fx_from.as_ref() == Some(&inputs) {
            return;
        }
        let want = if inputs.passthrough {
            Vec::new()
        } else {
            corrent_core::audio_fx::chain(&inputs.fx, inputs.caps, &inputs.source)
        };
        let plan = corrent_core::audio_fx::plan(&self.fx_applied, &want, inputs.caps.target);
        if let Some(af) = &plan.set_af {
            // A structure change restarts the filters; never more than a few times a second
            // (without `af-command` targets, every value is one).
            let wait = Duration::from_millis(150).saturating_sub(self.fx_set_at.map_or(Duration::MAX, |t| t.elapsed()));
            if !wait.is_zero() {
                self.ctx.request_repaint_after(wait);
                return;
            }
            self.fx_set_at = Some(Instant::now());
            let _ = self.mpv.command_async(FX_REPLY, &["set", "af", af]);
        }
        for [label, option, value, target] in &plan.commands {
            let _ = self.mpv.command_async(FX_REPLY, &["af-command", label, option, value, target]);
        }
        let meters_on = self.fx_applied_meters(&plan.applied);
        crate::util::with_lock(&self.status, |s| {
            if meters_on && !s.meters_on {
                s.meters = Default::default();
            }
            s.meters_on = meters_on;
        });
        self.fx_applied = plan.applied;
        self.fx_from = Some(inputs);
    }

    fn fx_applied_meters(&self, stages: &[corrent_core::audio_fx::Stage]) -> bool {
        stages.iter().any(|s| s.label == "out")
    }

    /// Asks for what the pages show, the first time one is up, and tells the events thread which
    /// are up. Properties stay observed afterwards, but it only repaints for the pages that show.
    fn observe_pages(&mut self) {
        let (meter_page, stats) = (self.meter_page, self.stats);
        crate::util::with_lock(&self.status, |s| {
            s.meter_page = meter_page;
            s.stats_page = stats;
        });
        if meter_page && !self.observing_meters {
            self.observing_meters = true;
            for (i, p) in
                ["af-metadata/in", "af-metadata/out", "audio-out-params", "af", "current-ao"].iter().enumerate()
            {
                let _ = self.mpv.observe(100 + i as u64, p);
            }
        }
        if stats && !self.observing_stats {
            self.observing_stats = true;
            for (i, p) in [
                "video-params/pixelformat",
                "video-params/primaries",
                "video-params/gamma",
                "video-params/sig-peak",
                "container-fps",
                "estimated-display-fps",
                "vsync-jitter",
                "decoder-frame-drop-count",
                "vo-delayed-frame-count",
                "avsync",
                "demuxer-cache-state",
                "video-bitrate",
                "audio-bitrate",
                "audio-out-params",
                "current-ao",
                "af",
                "mpv-version",
                "ffmpeg-version",
            ]
            .iter()
            .enumerate()
            {
                let _ = self.mpv.observe(200 + i as u64, p);
            }
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
            // Already in mpv's playlist: let it move on.
            if self.appended.load(Ordering::Relaxed) == self.index as i64 {
                self.appending = None;
                self.cmd(&["playlist-next"]);
                let resume = crate::util::with_lock(&self.status, |s| {
                    s.pos = 0.0;
                    s.ended = false;
                    s.entry_ended = false;
                    s.pos
                });
                self.last_reported = resume;
            } else {
                self.load_current();
            }
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
        self.last_reported = pos;
    }

    /// Per-frame housekeeping: progress reports and advancing the queue at the end.
    pub fn tick(&mut self) {
        self.save_volume(false);
        self.save_fx(false);
        self.sync_fx();
        if self.last_report.elapsed() > Duration::from_secs(10) {
            // A film left paused overnight has nothing new to say every 10 s.
            let (pos, paused) = crate::util::with_lock(&self.status, |s| (s.pos, s.paused));
            if paused && pos == self.last_reported {
                self.last_report = Instant::now();
            } else {
                self.report(false);
            }
        }
        self.gapless_tick();
        let ended = crate::util::with_lock(&self.status, |s| s.ended);
        if ended {
            self.report(true);
            {
                let mut s = crate::util::lock(&self.status);
                s.ended = false;
            }
            // "Cancel" on the countdown: stay at the end. So does the end of the queue, paused.
            let on = self.no_countdown != self.current().map(|e| e.item_id) || self.audio_only;
            if !(on && self.next()) {
                self.cmd(&["set", "pause", "yes"]);
            }
        }
    }

    /// The sound sheet over the picture, above the controls at the right; too tall for the window,
    /// it scrolls. It stays while the picture plays, to hear what the knobs do.
    fn sound_area(&mut self, ui: &mut Ui, p: &Palette, rect: Rect) {
        let width = (rect.width() - 24.0).min(320.0);
        let max_height = (rect.height() - 170.0).max(120.0);
        let at = pos2(rect.right() - 12.0, rect.bottom() - 110.0);
        let mut close = false;
        egui::Area::new(ui.id().with("sound-sheet"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(t!("Sound")).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            close = ui
                                .add(egui::Button::new(egui_phosphor::regular::X).frame(false))
                                .on_hover_text(t!("Close"))
                                .clicked();
                        });
                    });
                    egui::ScrollArea::vertical().max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
                        self.sound_sheet(ui, p);
                    });
                });
            });
        if close {
            self.sound_open = false;
        }
    }

    /// The sound sheet's contents, for the player.
    fn sound_sheet(&mut self, ui: &mut Ui, p: &Palette) {
        let mut pb = crate::util::with_lock(&self.audio, |a| a.clone());
        let before = pb.clone();
        if super::sound::sheet(ui, p, &mut pb) {
            // A profile may have changed the output: the one thing that re-creates it.
            for (k, v) in pb.changed_options(&before) {
                self.cmd(&["set", k, &v]);
            }
            crate::util::with_lock(&self.audio, |a| *a = pb);
            // `sync_fx` runs next frame, and a released slider may not ask for one.
            ui.ctx().request_repaint();
        }
    }

    /// Albums play through mpv's own playlist, which is what makes them gapless: when an entry has
    /// loaded, the next one is resolved and appended (`prefetch-playlist` opens it ahead of time),
    /// and mpv moves on by itself. Only for music: a film's next episode has its subtitles, its
    /// countdown with a Cancel, and a start position, which all belong to one file at a time.
    fn gapless_tick(&mut self) {
        let (pos, loaded, entry_ended) =
            crate::util::with_lock(&self.status, |s| (s.playlist_pos, s.loaded, s.entry_ended));
        // mpv moved on by itself: the one before was played to its end if it said so.
        let playing = usize::try_from(pos).ok().map(|p| self.playlist_start + p);
        if loaded && let Some(playing) = playing.filter(|p| *p > self.index && *p < self.queue.len()) {
            crate::util::with_lock(&self.status, |s| s.ended = entry_ended);
            self.report(true);
            crate::util::with_lock(&self.status, |s| {
                s.ended = false;
                s.entry_ended = false;
            });
            self.index = playing;
            self.appending = None;
        }
        let gapless = self.audio_only && crate::util::with_lock(&self.audio, |a| a.gapless);
        let next = self.index + 1;
        if !gapless || !loaded || self.appending == Some(next) || self.appended.load(Ordering::Relaxed) >= next as i64 {
            return;
        }
        let Some(e) = self.queue.get(next).filter(|e| e.stream.is_none()).cloned() else { return };
        self.appending = Some(next);
        let (client, mpv, appended, generation) =
            (self.client.clone(), self.mpv.clone(), self.appended.clone(), self.playlist_generation.clone());
        let at = generation.load(Ordering::Relaxed);
        self.rt.spawn(async move {
            let url = match &e.local {
                Some(path) => path.to_string_lossy().into_owned(),
                None => {
                    let info: Result<corrent_proto::library::PlaybackInfo, String> = client
                        .send(
                            reqwest::Method::POST,
                            &format!("/api/v1/playback/{}", e.item_id),
                            &serde_json::json!({ "profile": "native" }),
                        )
                        .await;
                    match info {
                        Ok(info) => client.url(&info.url),
                        // At its end the queue falls back to loading the next one the usual way.
                        Err(e) => return tracing::debug!("next track: {e}"),
                    }
                }
            };
            if generation.load(Ordering::Relaxed) != at {
                return;
            }
            let title = format!("force-media-title=%{}%{}", e.title.len(), e.title);
            let args = mpv.loadfile_mode_args(&url, "append", &title);
            let _ = mpv.command_async(0, &args.iter().map(String::as_str).collect::<Vec<_>>());
            appended.store(next as i64, Ordering::Relaxed);
            // The playlist was replaced just now: this entry isn't in the new one.
            if generation.load(Ordering::Relaxed) != at {
                appended.store(-1, Ordering::Relaxed);
            }
        });
    }

    /// Hands changed effects over to be saved, once they have rested for 2 s (or now). The settings
    /// window's own changes come back the other way, and need no saving.
    fn save_fx(&mut self, now: bool) {
        let current = crate::util::with_lock(&self.audio, |a| a.clone());
        if current == *crate::util::lock(&self.fx_saved) {
            self.fx_moved = None;
            return;
        }
        let moved = *self.fx_moved.get_or_insert_with(Instant::now);
        if now || moved.elapsed() >= Duration::from_secs(2) {
            *crate::util::lock(&self.fx_saved) = current.clone();
            self.fx_moved = None;
            super::sound::post_sound(current);
        } else {
            self.ctx.request_repaint_after(Duration::from_secs(2));
        }
    }

    /// Hands a changed volume over to be saved, once it has rested for 2 s (or now).
    fn save_volume(&mut self, now: bool) {
        let current = crate::util::with_lock(&self.status, |s| (s.volume, s.muted));
        if current == self.volume_saved {
            self.volume_moved = None;
            return;
        }
        let moved = *self.volume_moved.get_or_insert_with(Instant::now);
        if now || moved.elapsed() >= Duration::from_secs(2) {
            self.volume_saved = current;
            self.volume_moved = None;
            super::sound::post_volume(current.0 as f32, current.1);
        } else {
            self.ctx.request_repaint_after(Duration::from_secs(2));
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
        #[cfg(debug_assertions)]
        if let Some(w) = self.dev_window.take() {
            match w.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))) {
                Some((a, b)) => ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(a, b))),
                None => self.toggle_pip(&ctx),
            }
        }
        // Drawing a popup closes it on Escape, so this has to be asked before the controls are drawn.
        let popup_open = egui::Popup::is_any_open(&ctx);
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
        // An open popup or the sound sheet keep them up: hiding the controls would close a popup
        // under someone reading it.
        let show = paused || Instant::now() < self.controls_until || popup_open || self.sound_open;
        crate::util::with_lock(&self.status, |s| s.controls_visible = show);
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
        if self.sound_open && self.pip.is_none() {
            self.sound_area(ui, p, rect);
        }
        let mut below = rect.top() + if self.pip.is_some() { 40.0 } else { 90.0 };
        if self.stats {
            below = self.stats_overlay(ui, rect, below) + 8.0;
        }
        if self.meter_page {
            self.meters_overlay(ui, rect, below);
        }
        self.keys(&ctx, &mut action, popup_open);
        self.extra_keys(&ctx);
        action
    }

    /// mpv's keys for the less common things: `[` `]` speed (Backspace resets), `z`/`x`
    /// subtitle delay, `,`/`.` frame step, `s` screenshot, `i` stats, `I` the Audio page,
    /// `n`/`p` next/previous.
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
                // The dotted and the dotless one: a Turkish keyboard has both.
                "i" | "ı" => self.stats = !self.stats,
                "I" | "İ" => self.meter_page = !self.meter_page,
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

    fn keys(&mut self, ctx: &egui::Context, action: &mut PlayerAction, popup_open: bool) {
        // Typing the room code: `m` isn't mute, and the arrows move the caret.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // T: this frame becomes the thumbnail.
        if ctx.input(|i| i.key_pressed(egui::Key::T)) && self.can_set_thumbnail() {
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
        // Escape closes what is on top: a popup, the sound sheet, then fullscreen, then the player.
        if esc && self.sound_open && !popup_open {
            self.sound_open = false;
        } else if esc && !popup_open {
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
        let mut chosen = None;
        crate::ui::widgets::popup(egui::Popup::from_toggle_button_response(&r), |ui| {
            ui.set_min_width(260.0);
            for (kind, label, prop) in [("audio", t!("Audio"), "aid"), ("sub", t!("Subtitles"), "sid")] {
                ui.label(egui::RichText::new(label).strong());
                if kind == "sub"
                    && ui.selectable_label(!tracks.iter().any(|t| t.kind == "sub" && t.selected), t!("Off")).clicked()
                {
                    let _ = mpv.command_async(0, &["set", "sid", "no"]);
                    chosen = Some((kind, None));
                }
                for tr in tracks.iter().filter(|t| t.kind == kind) {
                    if ui.selectable_label(tr.selected, &tr.title).clicked() {
                        let _ = mpv.command_async(0, &["set", prop, &tr.id.to_string()]);
                        chosen = Some((kind, Some(tr)));
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
        if let Some((kind, tr)) = chosen {
            self.remember_track(kind, tr);
        }
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
            // `Drop` puts the window back.
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

    /// Speed, delays, subtitle size, picture shape, screenshot, stats and sound.
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
                ui.separator();
                if ui
                    .selectable_label(self.sound_open, format!("{}  {}", egui_phosphor::regular::SLIDERS, t!("Sound")))
                    .clicked()
                {
                    self.sound_open = !self.sound_open;
                }
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

    /// "Stats for nerds": what is playing and how. Returns the bottom of the box. In picture in
    /// picture, the first lines only.
    fn stats_overlay(&self, ui: &mut Ui, rect: Rect, top: f32) -> f32 {
        let lines = {
            let s = crate::util::lock(&self.status);
            let render = match &self.video {
                Video::Gl(v, _) => {
                    let size = crate::util::with_lock(v, |g| g.size);
                    format!("OpenGL {}×{}", size[0], size[1])
                }
                Video::Sw { size, .. } => format!("{} {}×{}", t!("software"), size[0], size[1]),
                Video::None => String::new(),
            };
            stats_lines(&s, &render)
        };
        let lines = if self.pip.is_some() { lines.get(..3).unwrap_or(&lines) } else { &lines[..] };
        let pos = pos2(rect.left() + 20.0, top);
        let galley = ui.painter().layout_no_wrap(lines.join("\n"), egui::FontId::monospace(12.0), Color32::WHITE);
        let bg = Rect::from_min_size(pos - vec2(8.0, 6.0), galley.size() + vec2(16.0, 12.0));
        ui.painter().rect_filled(bg, CornerRadius::same(6), Color32::from_black_alpha(170));
        ui.painter().galley(pos, galley, Color32::WHITE);
        bg.bottom()
    }

    /// The Audio page: on the right when it fits next to the A/V page, or below it. Compact when
    /// the window is small.
    fn meters_overlay(&self, ui: &mut Ui, rect: Rect, below: f32) {
        let compact = rect.width() < 640.0 || rect.height() < 420.0;
        let width = if compact { 230.0 } else { 330.0 };
        // Beside the A/V page when there is room, and under it when not.
        let top = if self.stats && rect.width() < 900.0 {
            below
        } else {
            rect.top() + if self.pip.is_some() { 40.0 } else { 90.0 }
        };
        let left = if self.stats && rect.width() < 900.0 { rect.left() + 12.0 } else { rect.right() - width - 12.0 };
        let area = Rect::from_min_max(pos2(left, top), pos2(left + width, rect.bottom() - 8.0));
        self.paint_meters_into(ui.painter(), area, compact);
    }

    fn paint_meters_into(&self, painter: &egui::Painter, area: Rect, compact: bool) {
        let (passthrough, makeup) = crate::util::with_lock(&self.audio, |a| {
            (a.passthrough, a.fx.night.as_ref().filter(|_| a.fx.enabled).map(|n| n.makeup_db))
        });
        let s = crate::util::lock(&self.status);
        let info = super::sound::MeterInfo {
            layout: s.source.layout.as_deref().unwrap_or(""),
            chain: &s.stats.chain,
            audio_in: &s.stats.audio_in,
            audio_out: &s.stats.audio_out,
            ao: &s.stats.current_ao,
            running: s.meters_on,
            passthrough,
            makeup,
        };
        super::sound::paint_meters(painter, area, &s.meters, &info, compact);
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
            let sr = ui
                .add(egui::Button::new(egui::RichText::new(egui_phosphor::regular::SLIDERS).size(16.0)).frame(false))
                .on_hover_text(t!("Sound"));
            crate::ui::widgets::popup(
                egui::Popup::from_toggle_button_response(&sr)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
                |ui| {
                    ui.set_width(300.0);
                    self.sound_sheet(ui, p);
                },
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
        // Music has no picture to put them on: above the bar, at the right.
        if self.meter_page {
            let content = ui.ctx().content_rect();
            let area = Rect::from_min_max(
                pos2(content.right() - 342.0, content.top() + 120.0),
                pos2(content.right() - 12.0, ui.min_rect().top() - 8.0),
            );
            let layer = egui::LayerId::new(egui::Order::Foreground, ui.id().with("meters"));
            let painter = ui.ctx().layer_painter(layer);
            self.paint_meters_into(&painter, area, area.width() < 300.0 || area.height() < 200.0);
        }
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
        self.save_volume(true);
        self.save_fx(true);
        // Nothing here may block the UI thread: a stalled network read can hold mpv up.
        let _ = self.mpv.command_async(0, &["stop"]);
        self.stop_events.store(true, Ordering::Relaxed);
        self.mpv.wakeup();
        // Closed from picture in picture: the window goes back to what it was.
        if let Some(size) = self.pip.take() {
            self.ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
            self.ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
        // The last reference (and `mpv_terminate_destroy`) goes to a helper thread once the
        // render context, dropped with `video`, has let go of its own.
        let mpv = self.mpv.clone();
        let _ = std::thread::Builder::new().name("corrent-mpv-close".into()).spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(mpv);
        });
    }
}

/// Whether a property's change is worth a frame of the whole app. A status bar's worth of numbers
/// arrive many times a second; with no page up, they redraw nothing (and a playing album redraws
/// the app once a second, not at every audio frame).
fn needs_repaint(name: &str, value: &Node, s: &mut Status) -> bool {
    match name {
        "time-pos" => {
            let second = value.as_f64().unwrap_or(s.pos) as i64;
            let new = second != s.shown_second;
            s.shown_second = second;
            new || s.controls_visible || s.stats_page || s.meter_page
        }
        "af-metadata/in" | "af-metadata/out" => s.meter_page,
        "video-codec"
        | "video-params/w"
        | "video-params/h"
        | "estimated-vf-fps"
        | "frame-drop-count"
        | "demuxer-cache-duration"
        | "audio-codec-name"
        | "hwdec-current"
        | "video-params/pixelformat"
        | "video-params/primaries"
        | "video-params/gamma"
        | "video-params/sig-peak"
        | "container-fps"
        | "estimated-display-fps"
        | "vsync-jitter"
        | "decoder-frame-drop-count"
        | "vo-delayed-frame-count"
        | "avsync"
        | "demuxer-cache-state"
        | "video-bitrate"
        | "audio-bitrate"
        | "mpv-version"
        | "ffmpeg-version"
        | "current-ao"
        | "af"
        | "audio-out-params" => s.stats_page || s.meter_page,
        _ => true,
    }
}

/// The lines of the A/V page.
fn stats_lines(s: &Status, render: &str) -> Vec<String> {
    let st = &s.stats;
    let or_dash = |v: f64, text: String| if v > 0.0 { text } else { "—".to_string() };
    let kbps = |v: f64| or_dash(v, format!("{:.0} kb/s", v / 1000.0));
    let decoding =
        if s.hwdec.is_empty() || s.hwdec == "no" { t!("software decoding").to_string() } else { s.hwdec.clone() };
    let mut lines = vec![
        format!("{} {}×{}  {}", st.video_codec, st.width, st.height, decoding),
        format!(
            "{} {} · {} {:.2} · {} {}",
            t!("Dropped frames"),
            st.dropped,
            t!("Decoder drops"),
            st.decoder_drops,
            t!("Late frames"),
            st.vo_delayed,
        ),
        format!("{} {:.1} s · {} {:.2}×", t!("Buffered"), st.cache_secs, t!("Speed"), s.speed),
        format!(
            "{} {} {} {}",
            st.pixelformat,
            st.primaries,
            st.gamma,
            or_dash(st.sig_peak, format!("{:.1}", st.sig_peak))
        ),
        format!(
            "{} {} · {} {} · {} {}",
            t!("Container"),
            or_dash(st.container_fps, format!("{:.3}", st.container_fps)),
            t!("Filtered"),
            or_dash(st.fps, format!("{:.3}", st.fps)),
            t!("Display"),
            or_dash(st.display_fps, format!("{:.3}", st.display_fps)),
        ),
        format!(
            "{} {:+.1} ms · {} {}",
            t!("A/V sync"),
            st.avsync * 1000.0,
            t!("Jitter"),
            or_dash(st.vsync_jitter, format!("{:.3}", st.vsync_jitter)),
        ),
        format!(
            "{} {:.1} MB · {} {}",
            t!("Cache"),
            st.cache_bytes as f64 / 1e6,
            t!("Input"),
            or_dash(st.input_rate, format!("{:.1} MB/s", st.input_rate / 1e6)),
        ),
        format!("{} {} · {} {}", t!("Video"), kbps(st.video_bitrate), t!("Audio"), kbps(st.audio_bitrate)),
        format!("{} {} → {} ({})", st.audio_codec, st.audio_in, st.audio_out, st.current_ao),
    ];
    if !st.chain.is_empty() {
        lines.push(format!("{}: {}", t!("Filters"), st.chain.join(" · ")));
    }
    if !render.is_empty() {
        lines.push(render.to_string());
    }
    lines.push(format!("{} · FFmpeg {}", st.mpv_version, st.ffmpeg_version));
    lines
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

    /// With nothing up, what arrives all the time draws nothing.
    #[test]
    fn numbers_nobody_sees_draw_nothing() {
        let mut s = Status::default();
        let t = |v: f64| Node::Double(v);
        for name in ["frame-drop-count", "estimated-vf-fps", "demuxer-cache-duration", "avsync", "video-bitrate"] {
            assert!(!needs_repaint(name, &t(1.0), &mut s), "{name}");
        }
        assert!(!needs_repaint("af-metadata/out", &Node::None, &mut s));
        // The time: when the second changes, and not between.
        assert!(needs_repaint("time-pos", &t(1.0), &mut s));
        s.pos = 1.0;
        for v in [1.02, 1.5, 1.98] {
            assert!(!needs_repaint("time-pos", &t(v), &mut s), "{v}");
        }
        assert!(needs_repaint("time-pos", &t(2.0), &mut s));
        // Anything else on screen is drawn.
        assert!(needs_repaint("pause", &Node::Flag(true), &mut s));
        assert!(needs_repaint("volume", &t(40.0), &mut s));
        // A page or the controls up: all of it.
        s.controls_visible = true;
        assert!(needs_repaint("time-pos", &t(2.5), &mut s));
        s.controls_visible = false;
        s.meter_page = true;
        assert!(needs_repaint("af-metadata/out", &Node::None, &mut s));
        assert!(needs_repaint("time-pos", &t(2.6), &mut s));
        s.meter_page = false;
        s.stats_page = true;
        assert!(needs_repaint("frame-drop-count", &t(1.0), &mut s));
        assert!(!needs_repaint("af-metadata/out", &Node::None, &mut s));
    }

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
