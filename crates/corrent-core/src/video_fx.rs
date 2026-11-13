//! Video effects: the settings, and what mpv gets from them.
//!
//! The effects are shaders that run in mpv's GPU renderer (its `glsl-shaders` hooks), so they cost
//! nothing on the CPU and see the picture at full quality. [`source`] is a pure function from the
//! settings to one shader text holding every pass that is on, and [`plan`] adds mpv's own picture
//! controls. `VideoFx::default()` gives an empty text: with everything off mpv is given no shader.
//!
//! mpv's renderer under the render API has no shader parameters, so values are written into the
//! text as constants and a change means a new text (about 2.5 ms to compile, measured in
//! `crates/corrent-mpv/src/video_tests.rs`; findings in `docs/spikes/mpv-shaders.md`). The sliders
//! therefore move in steps ([`Param::step`]), which keeps the number of texts small.
//!
//! Only numbers from the settings reach a shader (each clamped to its range and written with a
//! fixed format), never text: effect ids and parameter names from a settings file are only used to
//! look things up in [`CATALOGUE`].

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// A value of an effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Param {
    pub key: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// What a slider moves by.
    pub step: f32,
    /// A choice between a few things (0, 1, 2…), not an amount.
    pub choice: bool,
}

const fn p(key: &'static str, min: f32, max: f32, default: f32, step: f32) -> Param {
    Param { key, min, max, default, step, choice: false }
}

impl Param {
    /// `v` as this parameter takes it: in range, on a step, and the default for nonsense.
    pub fn fit(&self, v: f32) -> f32 {
        if !v.is_finite() {
            return self.default;
        }
        // In f64 and rounded to a few decimals, so that 0.1 stays 0.1 and fitting twice changes nothing.
        let (v, min, max, step) = (f64::from(v), f64::from(self.min), f64::from(self.max), f64::from(self.step));
        let fitted = min + ((v.clamp(min, max) - min) / step).round() * step;
        ((fitted.clamp(min, max) * 1e4).round() / 1e4) as f32
    }
}

/// Where in mpv's renderer an effect runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    /// The source picture, before it is scaled.
    Main,
    /// The picture as it goes to the screen, after scaling and colour management, under the
    /// subtitles.
    Output,
}

impl Hook {
    fn name(self) -> &'static str {
        match self {
            Hook::Main => "MAIN",
            Hook::Output => "OUTPUT",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Changes a pixel from that pixel alone (`vec3 fx_<id>(vec3 c, vec2 pos)`). Neighbours in the
    /// list share one pass.
    Point,
    /// Reads other pixels: one or more passes of its own.
    Pass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Detail,
    Colour,
    Style,
    Accessibility,
    Tools,
}

pub const GROUPS: [Group; 5] = [Group::Colour, Group::Detail, Group::Style, Group::Accessibility, Group::Tools];

/// A built-in effect.
#[derive(Clone, Copy, Debug)]
pub struct Shader {
    pub id: &'static str,
    pub group: Group,
    pub hook: Hook,
    pub kind: Kind,
    pub params: &'static [Param],
    glsl: &'static str,
}

impl Shader {
    pub fn param(&self, key: &str) -> Option<&'static Param> {
        self.params.iter().find(|p| p.key == key)
    }

    /// How many passes over the picture it adds when it runs alone.
    pub fn passes(&self) -> usize {
        match self.kind {
            Kind::Point => 1,
            Kind::Pass => self.glsl.matches("//!HOOK").count(),
        }
    }
}

macro_rules! glsl {
    ($name:literal) => {
        include_str!(concat!("video_fx/shaders/", $name, ".glsl"))
    };
}

const PRELUDE: &str = glsl!("prelude");

/// The built-in effects, in the order they run.
pub const CATALOGUE: &[Shader] = &[
    Shader {
        id: "denoise",
        group: Group::Detail,
        hook: Hook::Main,
        kind: Kind::Pass,
        params: &[p("strength", 0.0, 1.0, 0.5, 0.05), p("edges", 0.02, 0.3, 0.1, 0.01)],
        glsl: glsl!("denoise"),
    },
    Shader {
        id: "sharpen",
        group: Group::Detail,
        hook: Hook::Output,
        kind: Kind::Pass,
        params: &[p("amount", 0.0, 1.0, 0.5, 0.05)],
        glsl: glsl!("sharpen"),
    },
    Shader {
        id: "clarity",
        group: Group::Detail,
        hook: Hook::Output,
        kind: Kind::Pass,
        params: &[p("amount", 0.0, 1.0, 0.4, 0.05), p("radius", 2.0, 12.0, 6.0, 1.0)],
        glsl: glsl!("clarity"),
    },
    Shader {
        id: "soften",
        group: Group::Detail,
        hook: Hook::Output,
        kind: Kind::Pass,
        params: &[p("amount", 0.0, 1.0, 0.5, 0.05)],
        glsl: glsl!("soften"),
    },
    Shader {
        id: "levels",
        group: Group::Colour,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[
            p("black", 0.0, 0.3, 0.05, 0.01),
            p("white", 0.7, 1.0, 0.95, 0.01),
            p("midtones", 0.5, 2.0, 1.0, 0.05),
        ],
        glsl: glsl!("levels"),
    },
    Shader {
        id: "temperature",
        group: Group::Colour,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("warmth", -1.0, 1.0, 0.3, 0.05), p("tint", -1.0, 1.0, 0.0, 0.05)],
        glsl: glsl!("temperature"),
    },
    Shader {
        id: "tone",
        group: Group::Colour,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("shadows", -1.0, 1.0, 0.4, 0.05), p("highlights", -1.0, 1.0, -0.2, 0.05)],
        glsl: glsl!("tone"),
    },
    Shader {
        id: "vibrance",
        group: Group::Colour,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("amount", -1.0, 1.0, 0.4, 0.05)],
        glsl: glsl!("vibrance"),
    },
    Shader {
        id: "cinema",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("amount", 0.0, 1.0, 0.6, 0.05)],
        glsl: glsl!("cinema"),
    },
    Shader {
        id: "cross_process",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("amount", 0.0, 1.0, 0.6, 0.05)],
        glsl: glsl!("cross_process"),
    },
    Shader {
        id: "mono",
        group: Group::Colour,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("amount", 0.0, 1.0, 1.0, 0.05)],
        glsl: glsl!("mono"),
    },
    Shader {
        id: "sepia",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("amount", 0.0, 1.0, 0.8, 0.05)],
        glsl: glsl!("sepia"),
    },
    Shader {
        id: "daltonize",
        group: Group::Accessibility,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[
            Param { key: "kind", min: 0.0, max: 2.0, default: 1.0, step: 1.0, choice: true },
            p("strength", 0.0, 1.0, 1.0, 0.05),
        ],
        glsl: glsl!("daltonize"),
    },
    Shader {
        id: "colour_blind",
        group: Group::Accessibility,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[
            Param { key: "kind", min: 0.0, max: 2.0, default: 1.0, step: 1.0, choice: true },
            p("severity", 0.0, 1.0, 1.0, 0.05),
        ],
        glsl: glsl!("colour_blind"),
    },
    Shader {
        id: "posterize",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("levels", 2.0, 16.0, 5.0, 1.0)],
        glsl: glsl!("posterize"),
    },
    Shader {
        id: "invert",
        group: Group::Colour,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[],
        glsl: glsl!("invert"),
    },
    Shader {
        id: "night_vision",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("gain", 1.0, 4.0, 2.0, 0.1), p("noise", 0.0, 0.3, 0.1, 0.01)],
        glsl: glsl!("night_vision"),
    },
    Shader {
        id: "pixelate",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Pass,
        params: &[p("block", 2.0, 48.0, 8.0, 1.0)],
        glsl: glsl!("pixelate"),
    },
    Shader {
        id: "chromatic",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Pass,
        params: &[p("amount", 0.0, 8.0, 3.0, 0.5)],
        glsl: glsl!("chromatic"),
    },
    Shader {
        id: "bloom",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Pass,
        params: &[p("threshold", 0.4, 1.0, 0.7, 0.05), p("intensity", 0.0, 1.0, 0.5, 0.05)],
        glsl: glsl!("bloom"),
    },
    Shader {
        id: "outline",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Pass,
        params: &[p("amount", 0.0, 1.0, 0.7, 0.05)],
        glsl: glsl!("outline"),
    },
    Shader {
        id: "crt",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Pass,
        params: &[
            p("curvature", 0.0, 0.3, 0.1, 0.01),
            p("scanlines", 0.0, 1.0, 0.5, 0.05),
            p("mask", 0.0, 1.0, 0.3, 0.05),
        ],
        glsl: glsl!("crt"),
    },
    Shader {
        id: "halftone",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("levels", 2.0, 8.0, 2.0, 1.0), p("cell", 1.0, 6.0, 2.0, 1.0)],
        glsl: glsl!("halftone"),
    },
    Shader {
        id: "vignette",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("strength", 0.0, 1.0, 0.5, 0.05), p("radius", 0.3, 1.0, 0.75, 0.05)],
        glsl: glsl!("vignette"),
    },
    Shader {
        id: "grain",
        group: Group::Style,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("amount", 0.0, 0.3, 0.08, 0.01), p("size", 1.0, 4.0, 1.5, 0.5)],
        glsl: glsl!("grain"),
    },
    Shader {
        id: "zebra",
        group: Group::Tools,
        hook: Hook::Output,
        kind: Kind::Point,
        params: &[p("threshold", 0.8, 1.0, 0.95, 0.01)],
        glsl: glsl!("zebra"),
    },
];

pub fn shader(id: &str) -> Option<&'static Shader> {
    CATALOGUE.iter().find(|s| s.id == id)
}

/// One effect's switch and values. A value that is missing is the default.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Effect {
    pub on: bool,
    pub params: BTreeMap<String, f32>,
}

/// mpv's own picture controls, each -100..100 with 0 for "as it is".
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Adjust {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub gamma: f32,
    pub hue: f32,
}

/// The names of [`Adjust`]'s values, which are also mpv's properties.
pub const ADJUST: [&str; 5] = ["brightness", "contrast", "saturation", "gamma", "hue"];

impl Adjust {
    pub fn get(&self, key: &str) -> f32 {
        match key {
            "brightness" => self.brightness,
            "contrast" => self.contrast,
            "saturation" => self.saturation,
            "gamma" => self.gamma,
            "hue" => self.hue,
            _ => 0.0,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut f32> {
        Some(match key {
            "brightness" => &mut self.brightness,
            "contrast" => &mut self.contrast,
            "saturation" => &mut self.saturation,
            "gamma" => &mut self.gamma,
            "hue" => &mut self.hue,
            _ => return None,
        })
    }

    fn is_neutral(&self) -> bool {
        ADJUST.iter().all(|k| self.get(k) == 0.0)
    }

    fn normalize(&mut self) {
        for k in ADJUST {
            if let Some(v) = self.get_mut(k) {
                *v = if v.is_finite() { v.clamp(-100.0, 100.0).round() } else { 0.0 };
            }
        }
    }
}

/// The video effects settings. The default is no processing at all.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VideoFx {
    /// The master switch: off shows the picture as it is, and keeps everything below.
    pub enabled: bool,
    /// The look last picked (one of [`PRESETS`] or a saved look's name), until something is changed
    /// by hand.
    pub preset: Option<String>,
    pub adjust: Adjust,
    /// mpv's debanding: smooths the steps in skies and dark gradients.
    pub deband: bool,
    /// By [`Shader::id`]. An effect that isn't here is off, at its defaults.
    pub effects: BTreeMap<String, Effect>,
}

/// A saved look.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VideoPreset {
    pub name: String,
    pub fx: VideoFx,
}

/// The built-in looks a sheet offers, by the name kept in [`VideoFx::preset`].
pub const PRESETS: [&str; 8] = ["vivid", "cinema", "warm", "old_film", "black_white", "retro_tv", "comic", "night"];

impl VideoFx {
    /// What a settings file may hold made harmless: effects and values this version doesn't know
    /// are dropped, numbers are brought into range. After this, equal settings compare equal (a
    /// NaN never does).
    pub fn normalize(&mut self) {
        self.adjust.normalize();
        self.effects.retain(|id, e| {
            let Some(s) = shader(id) else { return false };
            e.params.retain(|k, v| match s.param(k) {
                Some(p) => {
                    *v = p.fit(*v);
                    true
                }
                None => false,
            });
            true
        });
    }

    /// The picture as it is. The values of the effects go back to their defaults.
    pub fn flatten(&mut self) {
        *self = VideoFx::default();
    }

    /// Starts from a built-in look; its values can then be changed.
    pub fn apply_preset(&mut self, name: &str) {
        let look: &[(&str, &[(&str, f32)])] = match name {
            "vivid" => {
                &[("vibrance", &[("amount", 0.5)]), ("sharpen", &[("amount", 0.4)]), ("clarity", &[("amount", 0.3)])]
            }
            "cinema" => {
                &[("cinema", &[("amount", 0.6)]), ("vignette", &[("strength", 0.35)]), ("grain", &[("amount", 0.04)])]
            }
            "warm" => &[("temperature", &[("warmth", 0.35)]), ("tone", &[("shadows", 0.2), ("highlights", -0.1)])],
            "old_film" => &[
                ("sepia", &[("amount", 0.7)]),
                ("grain", &[("amount", 0.14), ("size", 2.0)]),
                ("vignette", &[("strength", 0.6), ("radius", 0.65)]),
                ("soften", &[("amount", 0.3)]),
            ],
            "black_white" => &[("mono", &[("amount", 1.0)]), ("levels", &[("black", 0.04), ("white", 0.94)])],
            "retro_tv" => &[("crt", &[]), ("bloom", &[("intensity", 0.3)])],
            "comic" => {
                &[("posterize", &[("levels", 8.0)]), ("outline", &[("amount", 0.8)]), ("vibrance", &[("amount", 0.4)])]
            }
            "night" => &[("temperature", &[("warmth", 0.6)]), ("tone", &[("shadows", 0.0), ("highlights", -0.5)])],
            _ => return,
        };
        self.flatten();
        for (id, values) in look {
            self.toggle(id, true);
            for (key, v) in *values {
                self.set(id, key, *v);
            }
        }
        if name == "night" {
            self.adjust.brightness = -8.0;
        }
        self.enabled = true;
        self.preset = Some(name.to_string());
    }

    pub fn is_on(&self, id: &str) -> bool {
        self.effects.get(id).is_some_and(|e| e.on)
    }

    /// Turns a built-in effect on or off; its values stay.
    pub fn toggle(&mut self, id: &str, on: bool) {
        if let Some(s) = shader(id) {
            self.effects.entry(s.id.to_string()).or_default().on = on;
        }
    }

    /// A value of an effect: in range, the default when it isn't set.
    pub fn value(&self, id: &str, key: &str) -> f32 {
        let Some(p) = shader(id).and_then(|s| s.param(key)) else { return 0.0 };
        self.effects.get(id).and_then(|e| e.params.get(key)).map_or(p.default, |v| p.fit(*v))
    }

    pub fn set(&mut self, id: &str, key: &str, v: f32) {
        if let Some((s, p)) = shader(id).and_then(|s| Some((s, s.param(key)?))) {
            self.effects.entry(s.id.to_string()).or_default().params.insert(p.key.to_string(), p.fit(v));
        }
    }

    /// The effects that are switched on, in the order they run (whatever the master switch says).
    pub fn active(&self) -> impl Iterator<Item = &'static Shader> + '_ {
        CATALOGUE.iter().filter(|s| self.is_on(s.id))
    }

    /// Something would change the picture.
    pub fn has_effects(&self) -> bool {
        self.enabled && (self.deband || !self.adjust.is_neutral() || self.active().next().is_some())
    }
}

/// What the renderer in use can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// mpv draws with the graphics card. Its software renderer runs neither shaders nor its own
    /// picture controls.
    pub gpu: bool,
}

/// What mpv needs for some settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The text of the one shader file, empty for none.
    pub source: String,
    /// mpv's properties with their values, always all of them: off sets them back.
    pub props: Vec<(&'static str, String)>,
}

pub fn plan(fx: &VideoFx, caps: &Caps) -> Plan {
    let on = fx.enabled && caps.gpu;
    let mut props: Vec<(&'static str, String)> = ADJUST
        .iter()
        .map(|k| {
            let v = if on { fx.adjust.get(k) } else { 0.0 };
            (*k, format!("{}", if v.is_finite() { v.clamp(-100.0, 100.0).round() as i32 } else { 0 }))
        })
        .collect();
    props.push(("deband", if on && fx.deband { "yes" } else { "no" }.to_string()));
    Plan { source: if on { source(fx) } else { String::new() }, props }
}

/// A number as GLSL reads a float: always with a decimal point, never an exponent.
fn num(v: f32) -> String {
    format!("{v:.6}")
}

fn defines(out: &mut String, fx: &VideoFx, s: &Shader) {
    for p in s.params {
        let _ =
            writeln!(out, "#define {}_{} {}", s.id.to_uppercase(), p.key.to_uppercase(), num(fx.value(s.id, p.key)));
    }
}

/// The shader text for `fx`, whatever its master switch says: every pass of the effects that are
/// on, in [`CATALOGUE`]'s order. Empty when none is.
pub fn source(fx: &VideoFx) -> String {
    let mut active = fx.active().peekable();
    let mut out = String::new();
    while let Some(s) = active.next() {
        match s.kind {
            Kind::Pass => {
                let mut defs = String::from(PRELUDE);
                defines(&mut defs, fx, s);
                out.push_str(&s.glsl.replace("@DEFS@\n", &defs));
            }
            Kind::Point => {
                // The run of per-pixel effects from here becomes one pass.
                let mut run = vec![s];
                while let Some(next) = active.next_if(|n| n.kind == Kind::Point && n.hook == s.hook) {
                    run.push(next);
                }
                let names: Vec<&str> = run.iter().map(|s| s.id).collect();
                let _ =
                    writeln!(out, "//!HOOK {}\n//!BIND HOOKED\n//!DESC Corrent: {}", s.hook.name(), names.join(", "));
                out.push_str(PRELUDE);
                for s in &run {
                    defines(&mut out, fx, s);
                    out.push_str(s.glsl);
                }
                out.push_str("vec4 hook() {\n    vec4 px = HOOKED_tex(HOOKED_pos);\n    vec3 c = px.rgb;\n");
                for s in &run {
                    let _ = writeln!(out, "    c = fx_{}(c, HOOKED_pos);", s.id);
                }
                out.push_str("    return vec4(clamp(c, 0.0, 1.0), px.a);\n}\n");
            }
        }
    }
    out
}

/// How many passes over the picture `fx` adds.
pub fn passes(fx: &VideoFx) -> usize {
    source(fx).matches("//!HOOK").count()
}

/// The file name for a shader text: mpv keeps a loaded shader by its path, so a new text needs a
/// new name, and the same text can use its file again.
pub fn file_name(source: &str) -> String {
    // FNV-1a.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in source.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fx-{h:016x}.glsl")
}

/// A line of mpv's log that says a shader was refused (it then draws a blue picture until the
/// shader is taken away). `libmpv_render` logs "fragment shader compile log (status=0):" and
/// "shader link log (status=0):" at the error level, with the driver's words after them.
pub fn is_shader_error(prefix: &str, level: &str, text: &str) -> bool {
    prefix.contains("libmpv_render")
        && matches!(level, "error" | "fatal")
        && text.contains("shader")
        && text.contains("(status=0)")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(id: &str) -> VideoFx {
        let mut fx = VideoFx { enabled: true, ..Default::default() };
        fx.toggle(id, true);
        fx
    }

    fn everything() -> VideoFx {
        let mut fx = VideoFx { enabled: true, ..Default::default() };
        for s in CATALOGUE {
            fx.toggle(s.id, true);
        }
        fx
    }

    #[test]
    fn the_default_is_no_shader_at_all() {
        let fx = VideoFx::default();
        assert!(!fx.has_effects());
        let plan = plan(&fx, &Caps { gpu: true });
        assert_eq!(plan.source, "");
        assert!(plan.props.iter().all(|(k, v)| if *k == "deband" { v == "no" } else { v == "0" }), "{:?}", plan.props);
        assert_eq!(passes(&fx), 0);
    }

    #[test]
    fn there_are_more_than_twenty_builtins_with_ids_of_their_own() {
        assert!(CATALOGUE.len() >= 25);
        let ids: std::collections::BTreeSet<&str> = CATALOGUE.iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), CATALOGUE.len());
        for s in CATALOGUE {
            assert!(s.id.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{}", s.id);
            for p in s.params {
                assert!(p.min < p.max && p.step > 0.0, "{}.{}", s.id, p.key);
                assert_eq!(p.fit(p.default), p.default, "{}.{}: the default is on a step", s.id, p.key);
            }
        }
    }

    #[test]
    fn every_builtin_generates_a_pass() {
        for s in CATALOGUE {
            let text = source(&only(s.id));
            assert!(text.starts_with("//!HOOK "), "{}: {text}", s.id);
            assert!(text.contains(&format!("//!HOOK {}\n", s.hook.name())), "{}", s.id);
            assert!(!text.contains("@DEFS@"), "{}", s.id);
            assert_eq!(text.matches("//!HOOK").count(), s.passes(), "{}", s.id);
            assert_eq!(text.matches("vec4 hook()").count(), s.passes(), "{}", s.id);
        }
    }

    /// Every `<ID>_<KEY>` a template uses is a parameter, and every parameter is used.
    #[test]
    fn templates_use_exactly_their_parameters() {
        for s in CATALOGUE {
            let prefix = format!("{}_", s.id.to_uppercase());
            let mut used = std::collections::BTreeSet::new();
            let mut rest = s.glsl;
            while let Some(at) = rest.find(&prefix) {
                // Not the tail of another word (FX_BLOOM_A is a texture, not a parameter).
                let own = !rest[..at].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
                rest = &rest[at + prefix.len()..];
                let key: String = rest.chars().take_while(|c| c.is_ascii_uppercase() || *c == '_').collect();
                if own {
                    used.insert(key.to_lowercase());
                }
            }
            let params: std::collections::BTreeSet<String> = s.params.iter().map(|p| p.key.to_string()).collect();
            assert_eq!(used, params, "{}", s.id);
            match s.kind {
                Kind::Point => {
                    assert!(s.glsl.contains(&format!("vec3 fx_{}(vec3 c, vec2 pos)", s.id)), "{}", s.id);
                    assert!(!s.glsl.contains("//!"), "{}", s.id);
                }
                Kind::Pass => {
                    assert_eq!(s.glsl.matches("@DEFS@\n").count(), s.passes(), "{}", s.id);
                }
            }
        }
    }

    /// What mpv's renderer and the drivers it runs on all take.
    #[test]
    fn templates_keep_to_plain_glsl() {
        for (name, text) in CATALOGUE.iter().map(|s| (s.id, s.glsl)).chain([("prelude", PRELUDE)]) {
            for banned in ["#version", "precision ", "textureLod", "texelFetch", "uniform ", "gl_FragCoord", "\t", "\r"]
            {
                assert!(!text.contains(banned), "{name} has {banned:?}");
            }
            assert!(text.is_ascii(), "{name}");
            // A helper of one effect can't collide with another's in a shared pass.
            for line in
                text.lines().filter(|l| l.starts_with("float ") || l.starts_with("vec3 ") || l.starts_with("vec2 "))
            {
                let function = line.split_whitespace().nth(1).unwrap_or_default();
                assert!(function.starts_with("fx_"), "{name}: {line}");
                if name != "prelude" {
                    assert!(function.starts_with(&format!("fx_{name}")), "{name}: {line}");
                }
            }
        }
    }

    #[test]
    fn neighbouring_point_effects_share_one_pass() {
        let mut fx = only("levels");
        fx.toggle("vibrance", true);
        fx.toggle("sepia", true);
        fx.toggle("grain", true);
        let text = source(&fx);
        assert_eq!(text.matches("//!HOOK").count(), 1, "{text}");
        let order: Vec<usize> = ["fx_levels(c,", "fx_vibrance(c,", "fx_sepia(c,", "fx_grain(c,"]
            .iter()
            .map(|f| text.find(f).unwrap_or_else(|| panic!("{f} in {text}")))
            .collect();
        assert!(order.is_sorted(), "in the catalogue's order");
        // A pass of its own between them splits the run.
        fx.toggle("crt", true);
        let text = source(&fx);
        assert_eq!(text.matches("//!HOOK").count(), 3, "{text}");
        assert!(text.find("fx_sepia(c,") < text.find("tube television"));
        assert!(text.find("tube television") < text.find("fx_grain(c,"));
    }

    #[test]
    fn everything_at_once() {
        let fx = everything();
        let text = source(&fx);
        // denoise, sharpen, clarity, soften, the colours, pixelate, fringes, glow x4, outline, crt,
        // the overlays.
        assert_eq!(passes(&fx), 14, "{text}");
        assert!(text.starts_with("//!HOOK MAIN\n"), "denoise comes first, on the source");
        assert!(text.len() < 40_000, "{} bytes", text.len());
    }

    #[test]
    fn values_are_written_as_plain_floats() {
        let mut fx = only("grain");
        fx.set("grain", "amount", 0.2);
        let text = source(&fx);
        assert!(text.contains("#define GRAIN_AMOUNT 0.200000\n"), "{text}");
        assert!(text.contains("#define GRAIN_SIZE 1.500000\n"), "the default: {text}");
        fx.set("temperature", "warmth", -0.35);
        fx.toggle("temperature", true);
        assert!(source(&fx).contains("#define TEMPERATURE_WARMTH -0.350000\n"));
    }

    #[test]
    fn values_stay_in_range_and_on_steps() {
        let mut fx = only("grain");
        fx.set("grain", "amount", 99.0);
        assert_eq!(fx.value("grain", "amount"), 0.3);
        fx.set("grain", "amount", f32::NAN);
        assert_eq!(fx.value("grain", "amount"), 0.08);
        fx.set("grain", "amount", 0.123);
        assert!((fx.value("grain", "amount") - 0.12).abs() < 1e-6);
        fx.set("grain", "no such value", 1.0);
        fx.set("no such effect", "amount", 1.0);
        fx.toggle("no such effect", true);
        assert_eq!(fx.effects.len(), 1);
        assert_eq!(fx.effects["grain"].params.len(), 1);
    }

    /// A settings file can hold anything. None of it reaches the text.
    #[test]
    fn garbage_from_a_settings_file_is_harmless() {
        let evil = "x; } void main() { discard; } //!HOOK LUMA";
        let mut fx = only("vignette");
        fx.effects.insert(evil.to_string(), Effect { on: true, params: BTreeMap::from([(evil.to_string(), 1.0)]) });
        let vignette = fx.effects.get_mut("vignette").unwrap();
        vignette.params.insert("strength".into(), f32::NAN);
        vignette.params.insert("radius".into(), 1e30);
        vignette.params.insert(evil.into(), f32::INFINITY);
        fx.adjust.brightness = f32::NAN;
        fx.adjust.hue = -1e9;
        let text = source(&fx);
        assert!(!text.contains("discard") && !text.contains("LUMA"), "{text}");
        assert!(text.contains("#define VIGNETTE_STRENGTH 0.500000\n"), "{text}");
        assert!(text.contains("#define VIGNETTE_RADIUS 1.000000\n"), "{text}");
        let p = plan(&fx, &Caps { gpu: true });
        assert_eq!(p.props[0], ("brightness", "0".to_string()));
        assert_eq!(p.props[4], ("hue", "-100".to_string()));
        for line in text.lines().filter(|l| l.starts_with("#define") && !l.starts_with("#define PX ")) {
            let value = line.rsplit(' ').next().unwrap();
            assert!(
                value.parse::<f32>().is_ok_and(f32::is_finite) && value.contains('.') && !value.contains('e'),
                "{line}"
            );
        }

        #[allow(clippy::eq_op)]
        {
            assert!(fx != fx, "a NaN never equals itself: this is what normalize is for");
        }
        fx.normalize();
        assert_eq!(fx, fx.clone());
        assert_eq!(fx.effects.keys().collect::<Vec<_>>(), ["vignette"]);
        assert_eq!(fx.effects["vignette"].params.len(), 2);
        assert_eq!(fx.adjust.brightness, 0.0);
        assert_eq!(source(&fx), text, "normalizing changes nothing that was drawn");
    }

    #[test]
    fn presets_and_original() {
        for name in PRESETS {
            let mut fx = VideoFx::default();
            fx.apply_preset(name);
            assert!(fx.enabled && fx.has_effects(), "{name}");
            assert_eq!(fx.preset.as_deref(), Some(name));
            assert!(passes(&fx) >= 1, "{name}");
            let mut again = fx.clone();
            again.normalize();
            assert_eq!(again, fx, "{name}: a preset's values are in range and on their steps");
        }
        let mut fx = VideoFx::default();
        fx.apply_preset("vivid");
        fx.apply_preset("no such look");
        assert_eq!(fx.preset.as_deref(), Some("vivid"), "an unknown name changes nothing");
        fx.apply_preset("old_film");
        assert!(!fx.is_on("vibrance"), "a look replaces the one before");
        fx.flatten();
        assert_eq!(fx, VideoFx::default());
    }

    /// The web client's model reads `web/src/lib/videofx.catalogue.json` (the effects with their
    /// values, the looks): a change here without that file, or the other way round, fails. The
    /// file is the text this prints.
    #[test]
    fn the_web_catalogue_matches() {
        use serde_json::{Map, Value, json};
        // An f32 as it was written (0.05, not 0.05000000074505806).
        let n = |v: f32| (f64::from(v) * 1e4).round() / 1e4;
        let group = |g: Group| match g {
            Group::Detail => "detail",
            Group::Colour => "colour",
            Group::Style => "style",
            Group::Accessibility => "accessibility",
            Group::Tools => "tools",
        };
        let effects: Vec<Value> = CATALOGUE
            .iter()
            .map(|s| {
                let params: Vec<Value> = s
                    .params
                    .iter()
                    .map(|p| {
                        json!({
                            "key": p.key,
                            "min": n(p.min),
                            "max": n(p.max),
                            "default": n(p.default),
                            "step": n(p.step),
                            "choice": p.choice,
                        })
                    })
                    .collect();
                json!({ "id": s.id, "group": group(s.group), "params": params })
            })
            .collect();
        let presets: Vec<Value> = PRESETS
            .iter()
            .map(|name| {
                let mut fx = VideoFx::default();
                fx.apply_preset(name);
                let adjust: Map<String, Value> = ADJUST
                    .iter()
                    .filter(|k| fx.adjust.get(k) != 0.0)
                    .map(|k| (k.to_string(), json!(n(fx.adjust.get(k)))))
                    .collect();
                let effects: Map<String, Value> = fx
                    .effects
                    .iter()
                    .map(|(id, e)| {
                        assert!(e.on, "{name}: a look only lists effects that are on");
                        (id.clone(), Value::Object(e.params.iter().map(|(k, v)| (k.clone(), json!(n(*v)))).collect()))
                    })
                    .collect();
                json!({ "name": name, "adjust": adjust, "deband": fx.deband, "effects": effects })
            })
            .collect();
        let ours = json!({ "adjust": ADJUST, "groups": GROUPS.map(group), "effects": effects, "presets": presets });
        let theirs: Value = serde_json::from_str(include_str!("../../../web/src/lib/videofx.catalogue.json"))
            .expect("videofx.catalogue.json is JSON");
        assert!(
            theirs == ours,
            "web/src/lib/videofx.catalogue.json should be:\n{}",
            serde_json::to_string_pretty(&ours).unwrap()
        );
    }

    #[test]
    fn off_keeps_the_values_and_draws_nothing() {
        let mut fx = VideoFx::default();
        fx.apply_preset("cinema");
        fx.adjust.contrast = 12.0;
        fx.deband = true;
        let on = plan(&fx, &Caps { gpu: true });
        assert!(on.source.contains("fx_cinema"));
        assert!(on.props.contains(&("contrast", "12".into())) && on.props.contains(&("deband", "yes".into())));
        fx.enabled = false;
        assert!(!fx.has_effects());
        let off = plan(&fx, &Caps { gpu: true });
        assert_eq!(off, plan(&VideoFx::default(), &Caps { gpu: true }));
        assert!(fx.is_on("cinema") && fx.adjust.contrast == 12.0);
        fx.enabled = true;
        assert_eq!(plan(&fx, &Caps { gpu: false }), off, "and nothing without the graphics card");
    }

    #[test]
    fn settings_round_trip_as_toml() {
        #[derive(Serialize, Deserialize, PartialEq, Debug, Default)]
        #[serde(default)]
        struct File {
            picture: VideoFx,
        }
        let mut fx = VideoFx::default();
        fx.apply_preset("old_film");
        fx.adjust.gamma = -5.0;
        let file = File { picture: fx };
        let text = toml::to_string(&file).unwrap();
        assert!(text.contains("[picture.effects.sepia"), "{text}");
        assert_eq!(toml::from_str::<File>(&text).unwrap(), file);
        assert_eq!(toml::from_str::<File>("").unwrap(), File::default());
        let partial: File = toml::from_str("[picture]\nenabled = true\n[picture.effects.grain]\non = true\n").unwrap();
        assert!(partial.picture.is_on("grain"));
        assert_eq!(partial.picture.value("grain", "amount"), 0.08);
    }

    #[test]
    fn file_names_follow_the_text() {
        let a = file_name(&source(&only("grain")));
        assert_eq!(a, file_name(&source(&only("grain"))));
        assert_ne!(a, file_name(&source(&only("sepia"))));
        assert!(a.starts_with("fx-") && a.ends_with(".glsl") && a.len() == 3 + 16 + 5);
    }

    #[test]
    fn a_refused_shader_is_recognised_in_mpvs_log() {
        assert!(is_shader_error("libmpv_render", "error", "fragment shader compile log (status=0):"));
        assert!(is_shader_error("libmpv_render", "error", "shader link log (status=0): Fragment info"));
        assert!(!is_shader_error("libmpv_render", "error", "0(37) : error C1503: undefined variable"));
        assert!(!is_shader_error("libmpv_render", "v", "fragment shader compile log (status=1):"));
        assert!(!is_shader_error("ffmpeg", "error", "shader (status=0)"));
    }
}
