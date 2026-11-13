//! Video effects in the player: the sheet, and bringing mpv to what the settings say.
//!
//! The effects themselves, and the shader text made from the settings, are in
//! `corrent_core::video_fx`. mpv takes a shader as a file, so [`Applier`] has a thread that writes
//! the text into the profile's cache and then sets `glsl-shaders`; the UI thread waits for neither.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak, mpsc};
use std::time::{Duration, Instant};

use corrent_core::config::Playback;
use corrent_core::video_fx::{self, ADJUST, CATALOGUE, Caps, GROUPS, Group, PRESETS, Param, VideoFx};
use corrent_mpv::Mpv;
use egui::Ui;

use super::glass::{self, Tone};

/// `Event::SetReply` of a shader change.
pub const REPLY: u64 = 8;

/// A drag sends a new shader at most this often: each one is compiled (about 3 ms) and kept by
/// mpv's renderer until it goes.
const EVERY: Duration = Duration::from_millis(60);

/// mpv's compiled-shader cache may grow to this before it is emptied.
const COMPILED_LIMIT: u64 = 48 * 1024 * 1024;

/// Where this profile keeps shader files: ours, and under `compiled` what mpv's renderer caches
/// (by default it would write that into the user's own mpv cache).
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("shaders")
}

pub fn compiled_dir(dir: &Path) -> PathBuf {
    dir.join("compiled")
}

/// Keeps a player's mpv at the settings' video effects.
pub struct Applier {
    jobs: mpsc::Sender<String>,
    /// What mpv was last brought to.
    from: Option<(VideoFx, bool)>,
    props: Vec<(&'static str, String)>,
    source: String,
    set_at: Option<Instant>,
    /// The text mpv's renderer refused: not tried again.
    refused: Option<String>,
}

impl Applier {
    /// Starts the thread that writes shader files into `dir` and hands them to `mpv`. It ends
    /// when the applier is dropped.
    pub fn new(mpv: &Arc<Mpv>, dir: PathBuf) -> Applier {
        let (jobs, rx) = mpsc::channel::<String>();
        let mpv = Arc::downgrade(mpv);
        let spawned = std::thread::Builder::new().name("corrent-shaders".into()).spawn(move || worker(&mpv, &dir, &rx));
        if let Err(e) = spawned {
            tracing::warn!("video effects: no thread for the shader files ({e})");
        }
        Applier {
            jobs,
            from: None,
            props: video_fx::plan(&VideoFx::default(), &Caps { gpu: true }).props,
            source: String::new(),
            set_at: None,
            refused: None,
        }
    }

    /// The effects were refused by this graphics card and are off until they change.
    pub fn refused(&self) -> bool {
        self.refused.is_some()
    }

    /// Brings mpv to `fx`, sending only what changed; nothing here waits for mpv. `error`: mpv's
    /// renderer just refused a shader. Returns how long until this should run again (a change
    /// held back while a slider is dragged), and whether the effects were turned off just now.
    pub fn sync(&mut self, mpv: &Mpv, fx: &VideoFx, gpu: bool, error: bool) -> (Option<Duration>, bool) {
        let mut turned_off = false;
        if error && !self.source.is_empty() {
            // mpv draws a blue picture until the shader is gone.
            tracing::warn!("video effects: the renderer refused the shader; turned off");
            self.refused = Some(std::mem::take(&mut self.source));
            let _ = self.jobs.send(String::new());
            turned_off = true;
        }
        if self.from.as_ref().is_some_and(|(f, g)| f == fx && *g == gpu) {
            return (None, turned_off);
        }
        let plan = video_fx::plan(fx, &Caps { gpu });
        for (new, old) in plan.props.iter().zip(&mut self.props) {
            if new != old {
                let _ = mpv.command_async(0, &["set", new.0, &new.1]);
                *old = new.clone();
            }
        }
        if self.refused.as_ref() != Some(&plan.source) {
            self.refused = None;
        }
        if plan.source != self.source && self.refused.is_none() {
            let wait = EVERY.saturating_sub(self.set_at.map_or(Duration::MAX, |t| t.elapsed()));
            if !wait.is_zero() {
                return (Some(wait), turned_off);
            }
            self.set_at = Some(Instant::now());
            self.source = plan.source;
            let _ = self.jobs.send(self.source.clone());
        }
        self.from = Some((fx.clone(), gpu));
        (None, turned_off)
    }
}

fn worker(mpv: &Weak<Mpv>, dir: &Path, jobs: &mpsc::Receiver<String>) {
    // Texts of earlier sessions, and mpv's cache once it has grown.
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name();
            if name.to_string_lossy().starts_with("fx-") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let compiled = compiled_dir(dir);
    let size: u64 = std::fs::read_dir(&compiled)
        .map(|d| d.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum())
        .unwrap_or(0);
    if size > COMPILED_LIMIT {
        let _ = std::fs::remove_dir_all(&compiled);
    }
    while let Ok(mut source) = jobs.recv() {
        // Only the latest counts.
        while let Ok(newer) = jobs.try_recv() {
            source = newer;
        }
        let Some(mpv) = mpv.upgrade() else { return };
        if source.is_empty() {
            let _ = mpv.set_string_list_async(REPLY, "glsl-shaders", &[]);
            continue;
        }
        let file = dir.join(video_fx::file_name(&source));
        let written =
            file.exists() || std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&file, &source)).is_ok();
        if !written {
            tracing::warn!("video effects: couldn't write {}", file.display());
            continue;
        }
        let _ = mpv.set_string_list_async(REPLY, "glsl-shaders", &[&file.to_string_lossy()]);
    }
}

/// CORRENT_DEV_PICTURE=<look>|<id>[,<id>…]|all: those effects for the player being opened, to look
/// at. They are saved only if something else about the sound or the picture is then changed.
#[cfg(debug_assertions)]
pub fn dev_hook(pb: &mut Playback) {
    let Ok(want) = std::env::var("CORRENT_DEV_PICTURE") else { return };
    let fx = &mut pb.picture;
    if PRESETS.contains(&want.as_str()) {
        fx.apply_preset(&want);
        return;
    }
    fx.flatten();
    fx.enabled = true;
    for s in CATALOGUE.iter().filter(|s| want == "all" || want.split(',').any(|id| id == s.id)) {
        fx.toggle(s.id, true);
    }
}

/// An effect's name.
pub fn label(id: &str) -> &'static str {
    match id {
        "denoise" => t!("Reduce noise"),
        "sharpen" => t!("Sharpen"),
        "clarity" => t!("Clarity"),
        "soften" => t!("Soften"),
        "levels" => t!("Levels"),
        "temperature" => t!("Colour temperature"),
        "tone" => t!("Shadows and highlights"),
        "vibrance" => t!("Vibrance"),
        "cinema" => t!("Teal and orange"),
        "cross_process" => t!("Cross-process"),
        "mono" => t!("Black and white"),
        "sepia" => t!("Sepia"),
        "daltonize" => t!("Colour-blind correction"),
        "colour_blind" => t!("Colour-blind preview"),
        "posterize" => t!("Posterize"),
        "invert" => t!("Invert colours"),
        "night_vision" => t!("Night vision"),
        "pixelate" => t!("Pixelate"),
        "chromatic" => t!("Colour fringes"),
        "bloom" => t!("Glow"),
        "outline" => t!("Outlines"),
        "crt" => t!("Tube television"),
        "halftone" => t!("Dither"),
        "vignette" => t!("Vignette"),
        "grain" => t!("Film grain"),
        "zebra" => t!("Zebra stripes"),
        _ => "",
    }
}

/// What an effect does, for its hover text.
pub fn hint(id: &str) -> &'static str {
    match id {
        "denoise" => t!("Smooths grain and compression noise, keeping edges. Works on the source picture."),
        "sharpen" => t!("Brings out fine detail without halos on strong edges."),
        "clarity" => t!("More local contrast in the midtones: textures stand out."),
        "soften" => t!("A slight blur that takes the edge off a harsh picture."),
        "levels" => t!("Sets the black point, the white point and the midtones."),
        "temperature" => t!("Warmer or cooler, and a green–magenta tint."),
        "tone" => t!("Lifts or darkens the dark and the bright parts separately."),
        "vibrance" => t!("Strengthens pale colours more than strong ones."),
        "cinema" => t!("Cool shadows, warm lights and a little more contrast."),
        "cross_process" => t!("The shifted colours of film developed in the wrong chemicals."),
        "mono" => t!("Takes the colour out."),
        "sepia" => t!("The brown tone of old photographs."),
        "daltonize" => t!("Moves colours that are hard to tell apart to ones that are easier."),
        "colour_blind" => t!("Shows the picture as someone with a colour vision deficiency sees it."),
        "posterize" => t!("Fewer steps of each colour, like a poster."),
        "invert" => t!("A negative of the picture."),
        "night_vision" => t!("Green, amplified and noisy."),
        "pixelate" => t!("Big square pixels."),
        "chromatic" => t!("Red and blue drift apart towards the corners, like a cheap lens."),
        "bloom" => t!("Bright parts glow into their surroundings."),
        "outline" => t!("Darkens edges, like a drawing."),
        "crt" => t!("Scanlines, a shadow mask and curved glass."),
        "halftone" => t!("A few levels per colour in a fixed pattern, like old computer graphics."),
        "vignette" => t!("Darker towards the corners."),
        "grain" => t!("Fine moving noise, as on film."),
        "zebra" => t!("Stripes over parts that are at full brightness."),
        _ => "",
    }
}

/// A value's name, short enough for a slider.
pub fn param_label(id: &str, key: &str) -> &'static str {
    match (id, key) {
        ("levels", "black") => t!("Black"),
        ("levels", "white") => t!("White"),
        ("levels", "midtones") => t!("Midtones"),
        ("temperature", "warmth") => t!("Warmth"),
        ("temperature", "tint") => t!("Tint"),
        ("tone", "shadows") => t!("Shadows"),
        ("tone", "highlights") => t!("Highlights"),
        ("denoise", "edges") => t!("Edges"),
        ("clarity", "radius") => t!("Radius"),
        ("vignette", "radius") => t!("Size"),
        ("colour_blind", "severity") => t!("Severity"),
        ("posterize" | "halftone", "levels") => t!("Levels"),
        ("night_vision", "gain") => t!("Gain"),
        ("night_vision", "noise") => t!("Noise"),
        ("pixelate", "block") => t!("Block"),
        ("bloom", "threshold") | ("zebra", "threshold") => t!("Threshold"),
        ("bloom", "intensity") => t!("Intensity"),
        ("crt", "curvature") => t!("Curve"),
        ("crt", "scanlines") => t!("Scanlines"),
        ("crt", "mask") => t!("Mask"),
        ("halftone", "cell") | ("grain", "size") => t!("Size"),
        (_, "strength") => t!("Strength"),
        (_, "amount") => t!("Amount"),
        _ => "",
    }
}

fn group_label(group: Group) -> &'static str {
    match group {
        Group::Colour => t!("Colour"),
        Group::Detail => t!("Detail"),
        Group::Style => t!("Style"),
        Group::Accessibility => t!("Colour vision"),
        Group::Tools => t!("Tools"),
    }
}

fn preset_label(name: &str) -> &'static str {
    match name {
        "vivid" => t!("Vivid"),
        "cinema" => t!("Cinema"),
        "warm" => t!("Warm"),
        "old_film" => t!("Old film"),
        "black_white" => t!("Black and white"),
        "retro_tv" => t!("Retro TV"),
        "comic" => t!("Comic"),
        "night" => t!("Night"),
        _ => "",
    }
}

fn adjust_label(key: &str) -> &'static str {
    match key {
        "brightness" => t!("Brightness"),
        "contrast" => t!("Contrast"),
        "saturation" => t!("Saturation"),
        "gamma" => t!("Gamma"),
        "hue" => t!("Hue"),
        _ => "",
    }
}

/// A value as its slider shows it: as many decimals as its steps have.
fn value_text(p: &Param, v: f32) -> String {
    if p.step >= 1.0 {
        format!("{v:.0}")
    } else if p.step >= 0.1 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// The video effects sheet: on/off, looks, mpv's picture controls, the effects by group, then saved
/// looks. Used over the picture and in Settings → Playback. `refused`: this graphics card didn't
/// take the effects as they are set. Returns whether something changed.
pub fn sheet(ui: &mut Ui, tone: &Tone, pb: &mut Playback, caps: Caps, refused: bool) -> bool {
    let before = pb.clone();
    effects_sheet(ui, tone, pb, caps, refused);
    glass::separator(ui, tone);
    let looks: Vec<String> = pb.looks.iter().map(|l| l.name.clone()).collect();
    super::sound::named_section(ui, tone, t!("Saved looks"), t!("Save these effects"), looks, |name, action| {
        match action {
            super::sound::Named::Save => pb.save_look(name),
            super::sound::Named::Rename(to) => pb.rename_look(name, to),
            super::sound::Named::Delete => pb.delete_look(name),
        }
    });
    *pb != before
}

fn effects_sheet(ui: &mut Ui, tone: &Tone, pb: &mut Playback, caps: Caps, refused: bool) {
    let saved: Vec<String> = pb.looks.iter().map(|l| l.name.clone()).collect();
    let mut use_saved = None;
    let fx = &mut pb.picture;
    let before = fx.clone();
    let mut picked = false;
    if !caps.gpu {
        let text = t!("Video effects need the graphics card, and this picture is drawn without it.");
        ui.label(egui::RichText::new(text).color(tone.warn));
    } else if refused {
        let text = t!("This graphics card didn't take these effects, so they are off. Change one to try again.");
        ui.label(egui::RichText::new(text).color(tone.warn));
    }
    ui.add_enabled_ui(caps.gpu, |ui| {
        glass::switch(ui, tone, &mut fx.enabled, t!("On (off to compare)"));
        ui.add_space(2.0);
        let mut looks: Vec<(String, bool, Option<&str>)> =
            vec![(t!("Original").to_string(), !fx.has_effects() && fx.preset.is_none(), None)];
        for name in PRESETS {
            looks.push((preset_label(name).to_string(), fx.preset.as_deref() == Some(name), Some(name)));
        }
        let builtin = looks.len();
        for name in &saved {
            looks.push((name.clone(), fx.preset.as_deref() == Some(name), None));
        }
        let items: Vec<(&str, bool)> = looks.iter().map(|(l, on, _)| (l.as_str(), *on)).collect();
        if let Some(i) = glass::chips(ui, tone, &items) {
            picked = true;
            if i == 0 {
                fx.flatten();
            } else if let Some(name) = looks.get(i).and_then(|l| l.2) {
                fx.apply_preset(name);
            } else {
                use_saved = saved.get(i - builtin).cloned();
            }
        }
        ui.add_space(4.0);
        glass::section(ui, tone, t!("Adjust"));
        for key in ADJUST {
            if let Some(v) = fx.adjust.get_mut(key) {
                let text = format!("{:+.0}", *v);
                glass::slider(ui, tone, adjust_label(key), v, -100.0..=100.0, 1.0, 0.0, text);
            }
        }
        glass::switch(ui, tone, &mut fx.deband, t!("Smooth gradients"))
            .on_hover_text(t!("Removes the bands in skies and dark scenes of compressed video."));
        for group in GROUPS {
            ui.add_space(4.0);
            glass::section(ui, tone, group_label(group));
            for s in CATALOGUE.iter().filter(|s| s.group == group) {
                let mut on = fx.is_on(s.id);
                if glass::switch(ui, tone, &mut on, label(s.id)).on_hover_text(hint(s.id)).changed() {
                    fx.toggle(s.id, on);
                }
                if !on {
                    continue;
                }
                for p in s.params {
                    let was = fx.effects.get(s.id).and_then(|e| e.params.get(p.key)).copied().unwrap_or(p.default);
                    let mut v = was;
                    if p.choice {
                        let kinds = [t!("Red-weak"), t!("Green-weak"), t!("Blue-weak")];
                        let items: Vec<(&str, bool)> =
                            kinds.iter().enumerate().map(|(i, k)| (*k, (v - i as f32).abs() < 0.5)).collect();
                        if let Some(i) = glass::chips(ui, tone, &items) {
                            v = i as f32;
                        }
                    } else {
                        // Under the effect's id: several effects have an "Amount".
                        ui.push_id(s.id, |ui| {
                            let text = value_text(p, v);
                            glass::slider(
                                ui,
                                tone,
                                param_label(s.id, p.key),
                                &mut v,
                                p.min..=p.max,
                                p.step,
                                p.default,
                                text,
                            );
                        });
                    }
                    if v != was {
                        fx.set(s.id, p.key, v);
                    }
                }
            }
        }
    });
    if *fx != before && !picked {
        // By hand: no longer the look, and on if an effect changed (the switch itself stays as set).
        let switch_only = VideoFx { enabled: before.enabled, ..fx.clone() } == before;
        if !switch_only {
            fx.preset = None;
            fx.enabled = true;
        }
    }
    if let Some(name) = use_saved {
        pb.use_look(&name);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Color32, vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn harness_with(pb: Playback, caps: Caps) -> Harness<'static, Playback> {
        let p = crate::ui::theme::Palette::dark(Color32::BLUE);
        Harness::builder().with_size(vec2(360.0, 1800.0)).build_ui_state(
            move |ui, pb: &mut Playback| {
                sheet(ui, &Tone::theme(&p), pb, caps, false);
            },
            pb,
        )
    }

    fn harness(pb: Playback) -> Harness<'static, Playback> {
        harness_with(pb, Caps { gpu: true })
    }

    #[test]
    fn a_switch_turns_an_effect_on_and_a_change_by_hand_leaves_the_look() {
        let mut pb = Playback::default();
        pb.picture.apply_preset("cinema");
        let mut h = harness(pb);
        h.run();
        h.get_by_label("Sepia").click();
        h.run();
        assert!(h.state().picture.is_on("sepia") && h.state().picture.is_on("cinema"));
        assert_eq!(h.state().picture.preset, None);
        assert!(h.state().picture.enabled);
        h.get_by_label("Smooth gradients").click();
        h.run();
        assert!(h.state().picture.deband);
    }

    #[test]
    fn the_first_effect_switched_on_turns_the_effects_on() {
        let mut h = harness(Playback::default());
        h.run();
        h.get_by_label("Film grain").click();
        h.run();
        let fx = &h.state().picture;
        assert!(fx.enabled && fx.is_on("grain") && fx.has_effects());
        assert_eq!(video_fx::passes(fx), 1);
    }

    #[test]
    fn the_on_switch_compares_without_losing_the_effects() {
        let mut pb = Playback::default();
        pb.picture.apply_preset("old_film");
        let mut h = harness(pb);
        h.run();
        h.get_by_label("On (off to compare)").click();
        h.run();
        assert!(!h.state().picture.enabled);
        assert_eq!(h.state().picture.preset.as_deref(), Some("old_film"));
        assert!(h.state().picture.is_on("sepia"));
    }

    #[test]
    fn a_look_chip_applies_it_and_original_clears_it() {
        let mut h = harness(Playback::default());
        h.run();
        h.get_by_label("Retro TV").click();
        h.run();
        assert_eq!(h.state().picture.preset.as_deref(), Some("retro_tv"));
        assert!(h.state().picture.is_on("crt"));
        h.get_by_label("Original").click();
        h.run();
        assert_eq!(h.state().picture, VideoFx::default());
    }

    #[test]
    fn focusing_a_slider_changes_nothing() {
        let mut pb = Playback::default();
        pb.picture.apply_preset("vivid");
        let before = pb.picture.clone();
        let mut h = harness(pb);
        h.run();
        h.get_by_label("Radius").focus();
        h.run();
        h.get_by_label("Brightness").focus();
        h.run();
        assert_eq!(h.state().picture, before);
    }

    #[test]
    fn an_arrow_key_moves_a_value_by_its_step() {
        let mut pb = Playback::default();
        pb.picture.apply_preset("vivid");
        let mut h = harness(pb);
        h.run();
        h.get_by_label("Radius").focus();
        h.run();
        h.key_press(egui::Key::ArrowRight);
        h.run();
        assert_eq!(h.state().picture.value("clarity", "radius"), 7.0);
        assert_eq!(h.state().picture.preset, None);
    }

    #[test]
    fn without_the_graphics_card_nothing_can_be_switched() {
        let mut h = harness_with(Playback::default(), Caps { gpu: false });
        h.run();
        h.get_by_label_contains("need the graphics card");
        h.get_by_label("Sepia").click();
        h.run();
        assert_eq!(h.state().picture, VideoFx::default());
    }

    #[test]
    fn everything_has_a_name() {
        for s in CATALOGUE {
            assert!(!label(s.id).is_empty() && !hint(s.id).is_empty(), "{}", s.id);
            for p in s.params.iter().filter(|p| !p.choice) {
                assert!(!param_label(s.id, p.key).is_empty(), "{}.{}", s.id, p.key);
            }
        }
        // Two effects of one group can't share a name: the switches are found by it.
        for group in GROUPS {
            let names: Vec<&str> = CATALOGUE.iter().filter(|s| s.group == group).map(|s| label(s.id)).collect();
            let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
            assert_eq!(unique.len(), names.len(), "{group:?}");
            assert!(!group_label(group).is_empty());
        }
        assert!(PRESETS.iter().all(|n| !preset_label(n).is_empty()));
        assert!(ADJUST.iter().all(|k| !adjust_label(k).is_empty()));
    }

    #[test]
    fn the_applier_sends_only_changes_and_never_a_refused_text_twice() {
        let Ok(mpv) = Mpv::headless(&[("vo", "null"), ("ao", "null")]) else {
            eprintln!("libmpv not found; skipping");
            return;
        };
        let mpv = Arc::new(mpv);
        let dir = std::env::temp_dir().join(format!("corrent-applier-test-{}", std::process::id()));
        let mut a = Applier::new(&mpv, dir.clone());
        let list = |mpv: &Mpv| -> Vec<String> {
            mpv.get("glsl-shaders").unwrap().as_array().iter().filter_map(|n| n.as_str().map(str::to_string)).collect()
        };
        let wait_for = |want: &dyn Fn(&[String]) -> bool| {
            let until = Instant::now() + Duration::from_secs(10);
            while Instant::now() < until && !want(&list(&mpv)) {
                std::thread::sleep(Duration::from_millis(10));
            }
            list(&mpv)
        };
        let off = VideoFx::default();
        assert_eq!(a.sync(&mpv, &off, true, false), (None, false));
        assert!(list(&mpv).is_empty(), "off sets nothing");

        let mut fx = VideoFx::default();
        fx.apply_preset("cinema");
        fx.adjust.contrast = 10.0;
        assert_eq!(a.sync(&mpv, &fx, true, false), (None, false));
        let set = wait_for(&|l| !l.is_empty());
        assert_eq!(set.len(), 1);
        let text = std::fs::read_to_string(&set[0]).unwrap();
        assert_eq!(text, video_fx::source(&fx));
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && mpv.get_f64("contrast").unwrap() != Some(10.0) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(mpv.get_f64("contrast").unwrap(), Some(10.0));

        // A change right after is held back for a moment, then goes out.
        fx.set("vignette", "strength", 0.8);
        let (wait, _) = a.sync(&mpv, &fx, true, false);
        assert!(wait.is_some_and(|w| w <= EVERY), "{wait:?}");
        std::thread::sleep(EVERY);
        assert_eq!(a.sync(&mpv, &fx, true, false), (None, false));
        let second = wait_for(&|l| l != set.as_slice());
        assert_ne!(second, set);

        // Refused: off at once, and not tried again until something changes.
        assert_eq!(a.sync(&mpv, &fx, true, true), (None, true));
        assert!(a.refused());
        assert!(wait_for(&|l| l.is_empty()).is_empty());
        assert_eq!(a.sync(&mpv, &fx, true, false), (None, false));
        std::thread::sleep(Duration::from_millis(100));
        assert!(list(&mpv).is_empty());
        std::thread::sleep(EVERY);
        fx.set("vignette", "strength", 0.3);
        a.sync(&mpv, &fx, true, false);
        assert!(!a.refused());
        assert!(!wait_for(&|l| !l.is_empty()).is_empty());

        // Without the graphics card, or switched off: no shader and neutral controls.
        std::thread::sleep(EVERY);
        a.sync(&mpv, &fx, false, false);
        assert!(wait_for(&|l| l.is_empty()).is_empty());
        drop(a);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
