//! What mpv's GPU renderer does with user shaders (`glsl-shaders`) through the render API: the
//! measurements behind `corrent_core::video_fx`. The findings are in `docs/spikes/mpv-shaders.md`.
//!
//! These need libmpv and an offscreen OpenGL context ([`crate::test_gl`]); without either they
//! skip. The ones marked `#[ignore]` are measurements, run with `--ignored --nocapture`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::render::RenderContext;
use crate::test_gl::Offscreen;
use crate::{Event, Library, Mpv};

const W: i32 = 320;
const H: i32 = 240;

const INVERT: &str = "//!HOOK MAIN\n//!BIND HOOKED\n//!DESC invert\n\
    vec4 hook() { vec4 c = HOOKED_tex(HOOKED_pos); return vec4(1.0 - c.rgb, c.a); }\n";

struct Rig {
    // Dropped in this order: the render context before mpv, both before the GL context.
    rc: RenderContext,
    mpv: Arc<Mpv>,
    gl: Offscreen,
    logs: Vec<(String, String, String)>,
    dir: std::path::PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Rig {
    /// A paused test picture with its first frame ready.
    fn new() -> Option<Rig> {
        let Ok(lib) = Library::get() else {
            eprintln!("libmpv not found; skipping");
            return None;
        };
        let Some(gl) = Offscreen::new() else {
            eprintln!("no offscreen OpenGL context; skipping");
            return None;
        };
        let mut opts: Vec<(&str, &str)> = crate::HEADLESS.to_vec();
        opts.extend([("vo", "libmpv"), ("ao", "null"), ("pause", "yes"), ("hwdec", "no")]);
        let mpv = Arc::new(Mpv::new(lib, &opts).unwrap());
        // Compiled programs would otherwise be written to the user's ~/.cache/mpv. Older mpv
        // has no such option.
        let _ = mpv.set_property("gpu-shader-cache", "no");
        mpv.request_log_messages("warn").unwrap();
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("corrent-shader-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rc = RenderContext::new_gl(mpv.clone(), gl.resolver(), false).unwrap();
        mpv.command(&["loadfile", "av://lavfi:testsrc2=size=320x240:rate=30:duration=600"]).unwrap();
        let mut rig = Rig { rc, mpv, gl, logs: Vec::new(), dir };
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut started = false;
        while Instant::now() < deadline && !started {
            started = rig.pump(0.1);
        }
        assert!(started, "playback started");
        rig.frame();
        Some(rig)
    }

    /// Reads events for up to `wait` seconds; true once playback has (re)started.
    fn pump(&mut self, wait: f64) -> bool {
        let mut started = false;
        let mut wait = wait;
        while let Some(e) = self.mpv.wait_event(wait) {
            wait = 0.0;
            match e {
                Event::PlaybackRestart => started = true,
                Event::Log { prefix, level, text } => self.logs.push((prefix, level, text)),
                _ => {}
            }
        }
        started
    }

    /// The picture as mpv draws it now.
    fn frame(&mut self) -> Vec<u8> {
        // A changed option only asks for a redraw; give mpv a moment to say so.
        for _ in 0..20 {
            self.pump(0.01);
            if self.rc.update() {
                break;
            }
        }
        self.gl.render_rgba(&mut self.rc, W, H).unwrap()
    }

    /// A file per text, named by what is in it: mpv keeps a loaded file by its path.
    fn files(&self, texts: &[&str]) -> Vec<String> {
        use std::hash::{Hash, Hasher};
        texts
            .iter()
            .map(|t| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                t.hash(&mut h);
                let path = self.dir.join(format!("{:016x}.glsl", h.finish()));
                std::fs::write(&path, t).unwrap();
                path.to_string_lossy().into_owned()
            })
            .collect()
    }

    fn shaders(&mut self, texts: &[&str]) {
        let files = self.files(texts);
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        self.mpv.set_string_list("glsl-shaders", &files).unwrap();
    }

    fn errors(&mut self) -> Vec<String> {
        self.pump(0.05);
        let out = self
            .logs
            .iter()
            .filter(|l| l.1 == "error" || l.1 == "fatal")
            .map(|l| format!("[{}] {}", l.0, l.2))
            .collect();
        self.logs.clear();
        out
    }
}

/// The mean absolute difference per channel, 0..255.
fn distance(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a.iter().zip(b).map(|(x, y)| u64::from(x.abs_diff(*y))).sum();
    sum as f64 / a.len() as f64
}

fn mean(a: &[u8]) -> [f64; 3] {
    let mut m = [0.0; 3];
    for px in a.as_chunks::<4>().0 {
        for c in 0..3 {
            m[c] += f64::from(px[c]);
        }
    }
    m.map(|v| v / (a.len() / 4) as f64)
}

#[test]
fn a_string_list_keeps_its_items_whole() {
    let Ok(lib) = Library::get() else {
        eprintln!("libmpv not found; skipping");
        return;
    };
    let mpv = Mpv::new(lib, &[("config", "no"), ("vo", "null"), ("ao", "null"), ("idle", "yes")]).unwrap();
    let items = ["memory://a,b;c:d\n//!x", "second one"];
    mpv.set_string_list("glsl-shaders", &items).unwrap();
    let got = mpv.get("glsl-shaders").unwrap();
    let got: Vec<&str> = got.as_array().iter().filter_map(|n| n.as_str()).collect();
    assert_eq!(got, items);
    mpv.set_string_list("glsl-shaders", &[]).unwrap();
    assert!(mpv.get("glsl-shaders").unwrap().as_array().is_empty());
}

/// Q1, Q2: a shader set while a file is open (and paused) changes the picture, from memory.
#[test]
fn a_hook_changes_the_picture() {
    let Some(mut rig) = Rig::new() else { return };
    eprintln!("GL: {}", rig.gl.describe());
    let plain = rig.frame();
    assert!(distance(&plain, &vec![0u8; plain.len()]) > 10.0, "the test picture isn't black");
    rig.shaders(&[INVERT]);
    let inverted = rig.frame();
    assert_eq!(rig.errors(), Vec::<String>::new());
    let (a, b) = (mean(&plain), mean(&inverted));
    for c in 0..3 {
        assert!((a[c] + b[c] - 255.0).abs() < 12.0, "channel {c} is inverted: {a:?} and {b:?}");
    }
    rig.shaders(&[]);
    assert!(distance(&plain, &rig.frame()) < 1.0, "and back");
}

/// Q1 as the player sees it: paused, does mpv ask for a new frame after the list changed?
#[test]
#[ignore]
fn a_change_while_paused_asks_for_a_frame() {
    let Some(mut rig) = Rig::new() else { return };
    rig.frame();
    println!("update() with nothing changed: {}", rig.rc.update());
    rig.shaders(&[INVERT]);
    let t = Instant::now();
    let mut asked = false;
    while t.elapsed() < Duration::from_secs(2) && !asked {
        rig.pump(0.005);
        asked = rig.rc.update();
    }
    println!("update() after a shader change while paused: {asked} after {:?}", t.elapsed());
    rig.mpv.set_property("brightness", "10").unwrap();
    let t = Instant::now();
    let mut asked = false;
    while t.elapsed() < Duration::from_secs(2) && !asked {
        rig.pump(0.005);
        asked = rig.rc.update();
    }
    println!("update() after brightness while paused: {asked} after {:?}", t.elapsed());
    let text = format!("memory://{INVERT}");
    rig.mpv.set_string_list("glsl-shaders", &[&text]).unwrap();
    rig.frame();
    println!("memory://: {:?}", rig.errors().first());
}

/// Q3, Q4, Q5: what a shader that doesn't compile, or one with a bad header, does.
#[test]
#[ignore]
fn a_broken_shader() {
    let Some(mut rig) = Rig::new() else { return };
    let plain = rig.frame();
    for (name, text) in [
        ("compile error", "//!HOOK MAIN\n//!BIND HOOKED\nvec4 hook() { return nonsense(1.0); }\n"),
        (
            "bad directive",
            "//!HOOK MAIN\n//!BIND HOOKED\n//!NONSENSE 1\nvec4 hook() { return HOOKED_tex(HOOKED_pos); }\n",
        ),
        ("bad hook name", "//!HOOK NOWHERE\n//!BIND HOOKED\nvec4 hook() { return HOOKED_tex(HOOKED_pos); }\n"),
        ("not a shader", "hello"),
    ] {
        rig.shaders(&[text]);
        let broken = rig.frame();
        println!("{name}: distance from plain {:.1}, mean {:?}", distance(&plain, &broken), mean(&broken));
        for e in rig.errors() {
            println!("  {e}");
        }
        rig.shaders(&[]);
        println!("  cleared: distance from plain {:.2}", distance(&plain, &rig.frame()));
        rig.shaders(&[INVERT]);
        println!("  a good one after it: distance {:.1}", distance(&plain, &rig.frame()));
        rig.shaders(&[]);
        rig.frame();
        rig.errors();
    }
}

/// Q6, Q7: what a change of one constant costs, and whether the variants pile up somewhere.
#[test]
#[ignore]
fn recompiling() {
    fn rss_kb() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
            })
            .unwrap_or(0)
    }
    let Some(mut rig) = Rig::new() else { return };
    println!("GL: {}", rig.gl.describe());
    println!("gpu-shader-cache: {:?}", rig.mpv.get("gpu-shader-cache"));
    println!("gpu-shader-cache-dir: {:?}", rig.mpv.get("gpu-shader-cache-dir"));
    let text = |v: f32| {
        format!(
            "//!HOOK OUTPUT\n//!BIND HOOKED\n#define AMOUNT {v:.6}\n\
             vec4 hook() {{ vec4 c = HOOKED_tex(HOOKED_pos); return vec4(mix(c.rgb, vec3(dot(c.rgb, vec3(0.2126, 0.7152, 0.0722))), AMOUNT), c.a); }}\n"
        )
    };
    let before = rss_kb();
    let mut times = Vec::new();
    for i in 0..500 {
        let t = Instant::now();
        rig.shaders(&[&text(i as f32 / 500.0)]);
        let set = t.elapsed();
        let t = Instant::now();
        rig.gl.render_rgba(&mut rig.rc, W, H).unwrap();
        times.push((t.elapsed(), set));
    }
    let mut total: Vec<Duration> = times.iter().map(|t| t.0).collect();
    total.sort();
    println!(
        "500 distinct texts, render after each: first {:?} (set {:?}), median {:?}, p95 {:?}, max {:?}",
        times[0].0, times[0].1, total[250], total[475], total[499]
    );
    println!("RSS {before} kB -> {} kB", rss_kb());
    rig.shaders(&[&text(0.5)]);
    let t = Instant::now();
    rig.gl.render_rgba(&mut rig.rc, W, H).unwrap();
    println!("a text seen before: {:?}", t.elapsed());
    let t = Instant::now();
    rig.gl.render_rgba(&mut rig.rc, W, H).unwrap();
    println!("a frame with nothing changed: {:?}", t.elapsed());
    println!(
        "~/.cache/mpv exists: {}",
        std::env::var("HOME").is_ok_and(|h| std::path::Path::new(&h).join(".cache/mpv").exists())
    );
    println!("errors: {:?}", rig.errors());
}

/// Q8: OUTPUT with intermediate passes (`SAVE`, `WIDTH`, `HEIGHT`), and the uniforms there.
#[test]
#[ignore]
fn passes_on_output() {
    let Some(mut rig) = Rig::new() else { return };
    let plain = rig.frame();
    let cases = [
        (
            "save + size on OUTPUT",
            "//!HOOK OUTPUT\n//!BIND HOOKED\n//!SAVE SMALL\n//!WIDTH HOOKED.w 4 /\n//!HEIGHT HOOKED.h 4 /\n\
             vec4 hook() { return HOOKED_tex(HOOKED_pos); }\n\
             //!HOOK OUTPUT\n//!BIND HOOKED\n//!BIND SMALL\n\
             vec4 hook() { return vec4(SMALL_tex(SMALL_pos).rgb, 1.0); }\n",
        ),
        (
            "uniforms on OUTPUT",
            "//!HOOK OUTPUT\n//!BIND HOOKED\n\
             vec4 hook() { vec4 c = HOOKED_tex(HOOKED_pos); float n = fract(random + float(frame) * 0.0 + HOOKED_pt.x * HOOKED_size.x); \
             return vec4(c.rgb * 0.5 + 0.5 * vec3(n) + 0.0 * input_size.x + 0.0 * target_size.x, c.a); }\n",
        ),
        ("texOff", "//!HOOK OUTPUT\n//!BIND HOOKED\nvec4 hook() { return HOOKED_texOff(vec2(8.0, 0.0)); }\n"),
        (
            "MAIN then OUTPUT, SAVE across hooks",
            "//!HOOK MAIN\n//!BIND HOOKED\n//!SAVE BRIGHT\n//!WIDTH HOOKED.w 4 /\n//!HEIGHT HOOKED.h 4 /\n\
             vec4 hook() { return max(HOOKED_tex(HOOKED_pos) - 0.5, 0.0); }\n\
             //!HOOK OUTPUT\n//!BIND HOOKED\n//!BIND BRIGHT\n\
             vec4 hook() { return HOOKED_tex(HOOKED_pos) + vec4(BRIGHT_tex(BRIGHT_pos).rgb, 0.0); }\n",
        ),
        (
            "LINEAR (may not run)",
            "//!HOOK LINEAR\n//!BIND HOOKED\nvec4 hook() { return vec4(1.0) - HOOKED_tex(HOOKED_pos); }\n",
        ),
        (
            "MAINPRESUB",
            "//!HOOK MAINPRESUB\n//!BIND HOOKED\nvec4 hook() { vec4 c = HOOKED_tex(HOOKED_pos); return vec4(1.0 - c.rgb, c.a); }\n",
        ),
        (
            "SCALED",
            "//!HOOK SCALED\n//!BIND HOOKED\nvec4 hook() { vec4 c = HOOKED_tex(HOOKED_pos); return vec4(1.0 - c.rgb, c.a); }\n",
        ),
    ];
    for (name, text) in cases {
        rig.shaders(&[text]);
        let f = rig.frame();
        println!("{name}: distance from plain {:.1}, errors {:?}", distance(&plain, &f), rig.errors());
    }
    // Is OUTPUT at the target's size, and is what it returns what lands in the framebuffer?
    rig.shaders(&[
        "//!HOOK OUTPUT\n//!BIND HOOKED\nvec4 hook() { return vec4(HOOKED_size.x / 1000.0, HOOKED_size.y / 1000.0, 0.25, 1.0); }\n",
    ]);
    let big = rig.gl.render_rgba(&mut rig.rc, 640, 480).unwrap();
    println!("OUTPUT at 640x480 says size/1000 = {:?} (of 255)", &big[..4]);
    rig.shaders(&[
        "//!HOOK MAIN\n//!BIND HOOKED\nvec4 hook() { return vec4(HOOKED_size.x / 1000.0, HOOKED_size.y / 1000.0, 0.25, 1.0); }\n",
    ]);
    rig.frame();
    let big = rig.gl.render_rgba(&mut rig.rc, 640, 480).unwrap();
    println!("MAIN at 640x480 says size/1000 = {:?} (of 255)", &big[..4]);
    println!("errors {:?}", rig.errors());
}

/// Q9: mpv's own picture controls under the render API, with OpenGL and in software.
#[test]
#[ignore]
fn the_picture_properties() {
    let Some(mut rig) = Rig::new() else { return };
    let plain = rig.frame();
    for (name, value) in
        [("brightness", "30"), ("contrast", "40"), ("saturation", "-100"), ("gamma", "40"), ("hue", "50")]
    {
        rig.mpv.set_property(name, value).unwrap();
        let f = rig.frame();
        println!("GL {name}={value}: distance {:.1}, mean {:?}", distance(&plain, &f), mean(&f));
        rig.mpv.set_property(name, "0").unwrap();
    }
    rig.mpv.set_property("deband", "yes").unwrap();
    println!("GL deband: distance {:.2}, errors {:?}", distance(&plain, &rig.frame()), rig.errors());
    drop(rig);

    let mut opts: Vec<(&str, &str)> = crate::HEADLESS.to_vec();
    opts.extend([("vo", "libmpv"), ("ao", "null"), ("pause", "yes")]);
    let mpv = Arc::new(Mpv::new(Library::get().unwrap(), &opts).unwrap());
    let mut rc = RenderContext::new_sw(mpv.clone()).unwrap();
    mpv.command(&["loadfile", "av://lavfi:testsrc2=size=320x240:rate=30:duration=600"]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && mpv.wait_event(0.1) != Some(Event::PlaybackRestart) {}
    let shot = |rc: &mut RenderContext| {
        let mut buf = vec![0u8; (W * H * 4) as usize];
        for _ in 0..20 {
            mpv.wait_event(0.02);
            if rc.update() {
                break;
            }
        }
        rc.render_sw(W, H, &mut buf).unwrap();
        buf
    };
    let plain = shot(&mut rc);
    for (name, value) in
        [("brightness", "30"), ("contrast", "40"), ("saturation", "-100"), ("gamma", "40"), ("hue", "50")]
    {
        mpv.set_property(name, value).unwrap();
        let f = shot(&mut rc);
        println!("SW {name}={value}: distance {:.1}", distance(&plain, &f));
        mpv.set_property(name, "0").unwrap();
    }
    let file = std::env::temp_dir().join(format!("corrent-shader-test-sw-{}.glsl", std::process::id()));
    std::fs::write(&file, INVERT).unwrap();
    mpv.set_string_list("glsl-shaders", &[&file.to_string_lossy()]).unwrap();
    println!("SW invert shader: distance {:.1}", distance(&plain, &shot(&mut rc)));
    let _ = std::fs::remove_file(&file);
}

/// Q10: `//!PARAM` with `glsl-shader-opts` (libplacebo's way), which would spare the recompile.
#[test]
#[ignore]
fn shader_parameters() {
    let Some(mut rig) = Rig::new() else { return };
    let plain = rig.frame();
    rig.shaders(&[
        "//!PARAM amount\n//!TYPE float\n//!MINIMUM 0.0\n//!MAXIMUM 1.0\n0.0\n\n\
         //!HOOK MAIN\n//!BIND HOOKED\nvec4 hook() { vec4 c = HOOKED_tex(HOOKED_pos); return vec4(mix(c.rgb, 1.0 - c.rgb, amount), c.a); }\n",
    ]);
    let at_default = rig.frame();
    println!("PARAM at its default: distance {:.1}, errors {:?}", distance(&plain, &at_default), rig.errors());
    let set = rig.mpv.set_property("glsl-shader-opts", "amount=1.0");
    println!("glsl-shader-opts: {set:?}, distance {:.1}, errors {:?}", distance(&plain, &rig.frame()), rig.errors());
}

/// The asynchronous set answers, and a frame drawn after the answer has the shader.
#[test]
fn an_asynchronous_set_answers() {
    let Some(mut rig) = Rig::new() else { return };
    let plain = rig.frame();
    let files = rig.files(&[INVERT]);
    rig.mpv.set_string_list_async(77, "glsl-shaders", &[&files[0]]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut answer = None;
    while Instant::now() < deadline && answer.is_none() {
        if let Some(Event::SetReply { userdata: 77, error }) = rig.mpv.wait_event(0.1) {
            answer = Some(error);
        }
    }
    assert_eq!(answer, Some(0));
    assert!(distance(&plain, &rig.frame()) > 20.0);
}
