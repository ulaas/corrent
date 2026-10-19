//! Spike MS0b/MS0c: libmpv video inside an eframe (glow) window, composed with egui widgets.
//!
//! `cargo run -p corrent-mpv --example egui_player -- [--sw] [--shot out.png] [file or URL]`
//!
//! GL path: mpv renders into our own FBO-backed texture inside an `egui_glow` paint callback, and
//! the texture is drawn as an ordinary egui image, so egui overlays compose on top of the video.
//! SW path (`--sw`): mpv renders RGBA into a CPU buffer uploaded as an egui texture.
//! `--shot` saves a screenshot after ~3 s (egui `ViewportCommand::Screenshot`) and exits.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use corrent_mpv::render::RenderContext;
use corrent_mpv::{Event, Mpv};
use eframe::egui;
use eframe::glow::{self, HasContext};

struct GlVideo {
    rc: RenderContext,
    gl: Arc<glow::Context>,
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    size: [i32; 2],
    renders: u64,
}

impl GlVideo {
    /// Runs inside the paint callback, where eframe's GL context is current.
    fn paint(&mut self, want: [i32; 2]) {
        let gl = self.gl.clone();
        let resized = want != self.size;
        // SAFETY: GL thread with eframe's context current; handles are ours.
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
            // update() must run after every update callback; render only when there is a frame
            // (or the texture was reallocated and lost its contents).
            if !self.rc.update() && !resized {
                return;
            }
            // egui_glow restores its own state after a callback, but not the framebuffer binding.
            let prev = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            if let Err(e) = self.rc.render_gl(self.fbo.0.get() as i32, want[0], want[1], false) {
                eprintln!("render: {e}");
            }
            self.renders += 1;
            gl.bind_framebuffer(glow::FRAMEBUFFER, std::num::NonZeroU32::new(prev as u32).map(glow::NativeFramebuffer));
        }
    }
}

enum Video {
    Gl { state: Arc<Mutex<GlVideo>>, id: Option<egui::TextureId> },
    Sw { rc: RenderContext, buf: Vec<u8>, tex: Option<egui::TextureHandle>, size: [usize; 2], renders: u64 },
}

#[derive(Default)]
struct Status {
    pos: f64,
    duration: f64,
    title: String,
    hwdec: String,
}

struct App {
    mpv: Arc<Mpv>,
    video: Video,
    status: Arc<Mutex<Status>>,
    started: Instant,
    shot: Option<String>,
    shot_sent: bool,
    last_frame: Instant,
    frame_ms: Vec<f32>,
}

fn create_target(gl: &glow::Context) -> (glow::Framebuffer, glow::Texture) {
    // SAFETY: eframe's GL context is current during app creation.
    unsafe {
        let tex = gl.create_texture().expect("texture");
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
        let fbo = gl.create_framebuffer().expect("framebuffer");
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        (fbo, tex)
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, input: &str, sw: bool, shot: Option<String>) -> Self {
        let mut opts: Vec<(&str, &str)> = corrent_mpv::HEADLESS.to_vec();
        opts.extend([("vo", "libmpv"), ("hwdec", "auto-safe"), ("loop-file", "inf"), ("ao", "null")]);
        let mpv = Arc::new(Mpv::new(corrent_mpv::Library::get().expect("libmpv"), &opts).expect("mpv"));
        let ctx = cc.egui_ctx.clone();
        let mut video = if sw {
            Video::Sw {
                rc: RenderContext::new_sw(mpv.clone()).expect("sw render"),
                buf: Vec::new(),
                tex: None,
                size: [0, 0],
                renders: 0,
            }
        } else {
            let gl = cc.gl.clone().expect("glow backend");
            let gpa = cc.get_proc_address.clone().expect("get_proc_address");
            let rc = RenderContext::new_gl(mpv.clone(), move |name| gpa(name)).expect("gl render");
            let (fbo, tex) = create_target(&gl);
            Video::Gl { state: Arc::new(Mutex::new(GlVideo { rc, gl, fbo, tex, size: [16, 16], renders: 0 })), id: None }
        };
        let repaint = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        match &mut video {
            Video::Gl { state, .. } => state.lock().unwrap().rc.set_update_callback(repaint),
            Video::Sw { rc, .. } => rc.set_update_callback(repaint),
        }

        let status = Arc::new(Mutex::new(Status::default()));
        for (i, p) in ["time-pos", "duration", "media-title", "hwdec-current"].iter().enumerate() {
            mpv.observe(i as u64, p).expect("observe");
        }
        let (m, st) = (mpv.clone(), status.clone());
        std::thread::Builder::new()
            .name("mpv-events".into())
            .spawn(move || {
                loop {
                    match m.wait_event(-1.0) {
                        Some(Event::Shutdown) => break,
                        Some(Event::PropertyChange { name, value, .. }) => {
                            let mut s = st.lock().unwrap();
                            match name.as_str() {
                                "time-pos" => s.pos = value.as_f64().unwrap_or(0.0),
                                "duration" => s.duration = value.as_f64().unwrap_or(0.0),
                                "media-title" => s.title = value.as_str().unwrap_or("").to_string(),
                                "hwdec-current" => s.hwdec = value.as_str().unwrap_or("no").to_string(),
                                _ => {}
                            }
                            ctx.request_repaint();
                        }
                        Some(Event::EndFile(r)) => eprintln!("end file: {r:?}"),
                        _ => {}
                    }
                }
            })
            .expect("thread");
        mpv.command(&["loadfile", input]).expect("loadfile");
        App {
            mpv,
            video,
            status,
            started: Instant::now(),
            shot,
            shot_sent: false,
            last_frame: Instant::now(),
            frame_ms: Vec::new(),
        }
    }
}

fn fit(avail: egui::Rect, aspect: f32) -> egui::Rect {
    let mut size = avail.size();
    if size.x / size.y > aspect {
        size.x = size.y * aspect;
    } else {
        size.y = size.x / aspect;
    }
    egui::Rect::from_center_size(avail.center(), size)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let now = Instant::now();
        self.frame_ms.push((now - self.last_frame).as_secs_f32() * 1000.0);
        self.last_frame = now;
        let aspect = self.mpv.get_f64("video-params/aspect").ok().flatten().unwrap_or(16.0 / 9.0) as f32;

        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(egui::Color32::BLACK)).show(ui, |ui| {
            let rect = fit(ui.max_rect(), aspect.max(0.1));
            let ppp = ctx.pixels_per_point();
            let px = [(rect.width() * ppp).round().max(1.0) as i32, (rect.height() * ppp).round().max(1.0) as i32];
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            match &mut self.video {
                Video::Gl { state, id } => {
                    let tex = state.lock().unwrap().tex;
                    let id = *id.get_or_insert_with(|| frame.register_native_glow_texture(tex));
                    let st = state.clone();
                    let cb = egui_glow_callback(move |_info| st.lock().unwrap().paint(px));
                    ui.painter().add(egui::PaintCallback { rect, callback: Arc::new(cb) });
                    ui.painter().image(id, rect, uv, egui::Color32::WHITE);
                }
                Video::Sw { rc, buf, tex, size, renders } => {
                    let want = [px[0] as usize, px[1] as usize];
                    let resized = want != *size;
                    if rc.update() || resized {
                        buf.resize(want[0] * want[1] * 4, 0);
                        if rc.render_sw(px[0], px[1], buf).is_ok() {
                            *renders += 1;
                            let img = egui::ColorImage::from_rgba_unmultiplied(want, buf);
                            match tex {
                                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                                None => *tex = Some(ctx.load_texture("video", img, egui::TextureOptions::LINEAR)),
                            }
                            *size = want;
                        }
                    }
                    if let Some(t) = tex {
                        ui.painter().image(t.id(), rect, uv, egui::Color32::WHITE);
                    }
                }
            }

            // An egui overlay over the video: proves composition.
            let s = self.status.lock().unwrap();
            let renders = match &self.video {
                Video::Gl { state, .. } => state.lock().unwrap().renders,
                Video::Sw { renders, .. } => *renders,
            };
            let bar = egui::Rect::from_min_size(rect.left_bottom() - egui::vec2(-16.0, 72.0), egui::vec2(rect.width() - 32.0, 56.0));
            ui.painter().rect_filled(bar, 12.0, egui::Color32::from_black_alpha(170));
            let frac = if s.duration > 0.0 { (s.pos / s.duration) as f32 } else { 0.0 };
            let track = egui::Rect::from_min_size(bar.left_bottom() + egui::vec2(16.0, -16.0), egui::vec2(bar.width() - 32.0, 4.0));
            ui.painter().rect_filled(track, 2.0, egui::Color32::from_white_alpha(60));
            ui.painter().rect_filled(
                egui::Rect::from_min_size(track.min, egui::vec2(track.width() * frac.clamp(0.0, 1.0), 4.0)),
                2.0,
                egui::Color32::from_rgb(90, 160, 255),
            );
            let mut sorted = self.frame_ms.clone();
            sorted.sort_by(f32::total_cmp);
            let p99 = sorted.get(sorted.len() * 99 / 100).copied().unwrap_or(0.0);
            ui.painter().text(
                bar.left_top() + egui::vec2(16.0, 10.0),
                egui::Align2::LEFT_TOP,
                format!(
                    "{}  {:.1}/{:.1}s  {}  hwdec={}  renders={renders}  frame p99={p99:.1} ms",
                    if matches!(self.video, Video::Gl { .. }) { "GL" } else { "SW" },
                    s.pos,
                    s.duration,
                    s.title,
                    s.hwdec
                ),
                egui::FontId::proportional(14.0),
                egui::Color32::WHITE,
            );
        });

        if let Some(path) = self.shot.clone() {
            if !self.shot_sent && self.started.elapsed().as_secs_f32() > 3.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                self.shot_sent = true;
            }
            let shot = ctx.input(|i| {
                i.raw.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = shot {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                image::save_buffer(&path, &bytes, w as u32, h as u32, image::ColorType::Rgba8).expect("save png");
                eprintln!("saved {path}");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
    }
}

fn egui_glow_callback(f: impl Fn(egui::PaintCallbackInfo) + Send + Sync + 'static) -> eframe::egui_glow::CallbackFn {
    eframe::egui_glow::CallbackFn::new(move |info, _painter| f(info))
}

fn main() -> eframe::Result {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let sw = args.iter().any(|a| a == "--sw");
    args.retain(|a| a != "--sw");
    let shot = args.iter().position(|a| a == "--shot").map(|i| {
        let p = args.remove(i + 1);
        args.remove(i);
        p
    });
    let input = args
        .first()
        .cloned()
        .unwrap_or_else(|| "av://lavfi:testsrc2=size=1280x720:rate=30:duration=30".to_string());
    eframe::run_native(
        "corrent-mpv spike",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([960.0, 600.0]),
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(App::new(cc, &input, sw, shot)))),
    )
}
