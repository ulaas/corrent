//! Real-time sound processing: the settings, and the mpv filter chain built from them.
//!
//! [`chain`] is a pure function from settings to labelled stages (`@label:lavfi=[graph]`), and
//! [`plan`] turns a change into what mpv needs: a value goes through `af-command` on the running
//! filter, and only a change of structure sets `af` again. `AudioFx::default()` makes no stage at
//! all, so with everything off playback costs nothing extra. The measurements behind the choices
//! here are in `crates/corrent-mpv/src/audio_tests.rs`.
//!
//! Only numbers from the settings reach a filter graph (each clamped to what its filter takes),
//! never text.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// The centres of the default equalizer (ISO octave bands).
pub const ISO_BANDS: [u32; 10] = [31, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

/// One parametric band.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Band {
    pub hz: u32,
    pub q: f32,
    pub db: f32,
}

impl Default for Band {
    fn default() -> Self {
        Band { hz: 1000, q: 1.0, db: 0.0 }
    }
}

/// Night mode: a compressor with makeup gain, and a limiter after it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Night {
    pub threshold_db: f32,
    pub ratio: f32,
    pub makeup_db: f32,
}

impl Default for Night {
    fn default() -> Self {
        Night { threshold_db: -24.0, ratio: 4.0, makeup_db: 6.0 }
    }
}

/// Headphone crossfeed, as FFmpeg's `crossfeed` takes them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Crossfeed {
    pub strength: f32,
    pub range: f32,
}

impl Default for Crossfeed {
    fn default() -> Self {
        Crossfeed { strength: 0.5, range: 0.5 }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Loudness {
    #[default]
    Off,
    /// A slow leveller.
    Live,
}

/// The sound processing settings. The default is no processing at all.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioFx {
    /// The master switch of the effects (the meters are separate).
    pub enabled: bool,
    /// The preset last picked ("night", "dialogue", "crossfeed", or a user preset's name), until
    /// something is changed by hand.
    pub preset: Option<String>,
    pub eq: Vec<Band>,
    pub night: Option<Night>,
    pub dialogue: bool,
    pub crossfeed: Option<Crossfeed>,
    pub loudness: Loudness,
    /// Show the Audio meters when playback starts. The meters only run while they are shown.
    pub meters: bool,
}

impl Default for AudioFx {
    fn default() -> Self {
        AudioFx {
            enabled: false,
            preset: None,
            eq: flat_eq(),
            night: None,
            dialogue: false,
            crossfeed: None,
            loudness: Loudness::Off,
            meters: false,
        }
    }
}

/// The ten default bands, flat.
pub fn flat_eq() -> Vec<Band> {
    ISO_BANDS.iter().map(|&hz| Band { hz, q: 1.0, db: 0.0 }).collect()
}

/// A saved set of effects.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UserPreset {
    pub name: String,
    pub fx: AudioFx,
}

/// An output and the effects that go with it ("Receiver", "Headphones"…).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioProfile {
    pub name: String,
    pub audio_device: String,
    pub channels: String,
    pub exclusive: bool,
    pub passthrough: bool,
    pub fx: AudioFx,
}

impl Default for AudioProfile {
    fn default() -> Self {
        AudioProfile {
            name: String::new(),
            audio_device: "auto".into(),
            channels: "auto-safe".into(),
            exclusive: false,
            passthrough: false,
            fx: AudioFx::default(),
        }
    }
}

/// The built-in presets a sheet offers, by the name kept in [`AudioFx::preset`].
pub const PRESETS: [&str; 3] = ["night", "dialogue", "crossfeed"];

impl AudioFx {
    /// No effects (Flat). Loudness and the meters are separate choices and stay.
    pub fn flatten(&mut self) {
        self.enabled = false;
        self.preset = None;
        self.eq = flat_eq();
        self.night = None;
        self.dialogue = false;
        self.crossfeed = None;
    }

    /// Starts from a built-in preset; its values can then be changed.
    pub fn apply_preset(&mut self, name: &str) {
        self.flatten();
        match name {
            "night" => self.night = Some(Night::default()),
            "dialogue" => self.dialogue = true,
            "crossfeed" => self.crossfeed = Some(Crossfeed::default()),
            _ => return,
        }
        self.enabled = true;
        self.preset = Some(name.to_string());
    }

    /// The effects that would change the sound, whichever are on.
    pub fn has_effects(&self) -> bool {
        self.enabled
            && (self.eq.iter().any(|b| b.db != 0.0)
                || self.night.is_some()
                || self.dialogue
                || self.crossfeed.is_some()
                || self.loudness == Loudness::Live)
    }

    /// The gain taken off in front of the effects so that their boosts can't clip at the output:
    /// the largest positive band, and dialogue's boosts on top (`centre_lift`: with the lift of a
    /// 5.1 source's centre channel, when that runs). Never positive.
    pub fn headroom_db(&self, centre_lift: bool) -> f32 {
        let band = self.eq.iter().map(|b| clamp(b.db, -24.0, 24.0, 0.0)).fold(0.0, f32::max);
        let dialogue = match (self.dialogue, centre_lift) {
            (false, _) => 0.0,
            (true, true) => DIALOGUE_BOOST_DB + DIALOGUE_CENTRE_DB,
            (true, false) => DIALOGUE_BOOST_DB,
        };
        -(band + dialogue)
    }
}

const DIALOGUE_BOOST_DB: f32 = 3.0;
const DIALOGUE_CUT_DB: f32 = -2.0;
const DIALOGUE_CENTRE_DB: f32 = 4.0;
/// -1 dB as a linear level (the limiter's `limit`).
const LIMIT_LINEAR: f32 = 0.891;

fn clamp(v: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { fallback }
}

/// What this libmpv can do: found by a probe on a running system, see `sound.rs` in the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caps {
    pub filters: BTreeSet<String>,
    /// `af-command` takes a target. Without one nothing can be addressed, so every change sets `af`.
    pub target: bool,
}

/// The filters [`chain`] may use.
pub const FILTERS: [&str; 10] = [
    "volume",
    "equalizer",
    "pan",
    "acompressor",
    "alimiter",
    "crossfeed",
    "dynaudnorm",
    "astats",
    "ebur128",
    "aphasemeter",
];

impl Caps {
    /// Everything, until the probe has answered.
    pub fn assumed() -> Caps {
        Caps { filters: FILTERS.iter().map(|f| f.to_string()).collect(), target: true }
    }

    pub fn has(&self, filter: &str) -> bool {
        self.filters.contains(filter)
    }

    pub fn has_all(&self, filters: &[&str]) -> bool {
        filters.iter().all(|f| self.has(f))
    }

    /// The equalizer (and dialogue, which is built from it) can run.
    pub fn eq(&self) -> bool {
        self.has("equalizer")
    }
    pub fn night(&self) -> bool {
        self.has_all(&["acompressor", "alimiter"])
    }
    pub fn crossfeed(&self) -> bool {
        self.has("crossfeed")
    }
    pub fn loudness(&self) -> bool {
        self.has("dynaudnorm")
    }
    /// The meters' two main filters; correlation is optional.
    pub fn meters(&self) -> bool {
        self.has_all(&["astats", "ebur128"])
    }
}

/// A value a running filter can take through `af-command`.
#[derive(Clone, Debug, PartialEq)]
struct Value {
    option: &'static str,
    value: String,
    /// What makes this filter do nothing (`None`: the option has no such value).
    neutral: Option<&'static str>,
}

/// A filter inside a stage's graph.
#[derive(Clone, Debug, PartialEq)]
struct Filter {
    name: &'static str,
    /// `name@instance`, to address one of several.
    instance: Option<String>,
    /// Fixed once the filter runs: changing one is a change of structure.
    fixed: Vec<(&'static str, String)>,
    values: Vec<Value>,
}

impl Filter {
    fn target(&self) -> String {
        match &self.instance {
            Some(i) => format!("{}@{i}", self.name),
            None => self.name.to_string(),
        }
    }

    fn graph(&self) -> String {
        let mut out = self.target();
        let mut sep = '=';
        for (k, v) in self
            .fixed
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .chain(self.values.iter().map(|v| (v.option, v.value.as_str())))
        {
            // A bare value (`pan`'s spec) has no name.
            if k.is_empty() {
                let _ = write!(out, "{sep}{v}");
            } else {
                let _ = write!(out, "{sep}{k}={v}");
            }
            sep = ':';
        }
        out
    }
}

/// One labelled stage of the chain.
#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    pub label: &'static str,
    filters: Vec<Filter>,
}

impl Stage {
    /// The filter graph (the part inside `lavfi=[…]`).
    pub fn graph(&self) -> String {
        self.filters.iter().map(Filter::graph).collect::<Vec<_>>().join(",")
    }

    /// The same filters, whatever their values.
    fn same_structure(&self, other: &Stage) -> bool {
        self.label == other.label
            && self.filters.len() == other.filters.len()
            && self.filters.iter().zip(&other.filters).all(|(a, b)| {
                a.name == b.name
                    && a.instance == b.instance
                    && a.fixed == b.fixed
                    && a.values.iter().map(|v| v.option).eq(b.values.iter().map(|v| v.option))
            })
    }

    /// This stage made to do nothing, when all its filters can be: flipping an effect off while
    /// listening doesn't re-initialise the chain.
    fn bypassed(&self) -> Option<Stage> {
        let mut out = self.clone();
        for f in &mut out.filters {
            let mut any = false;
            for v in &mut f.values {
                if let Some(n) = v.neutral {
                    v.value = n.to_string();
                    any = true;
                }
            }
            if !any {
                return None;
            }
        }
        Some(out)
    }
}

/// The value `af` takes for `stages`. An empty string for no stages.
pub fn af_string(stages: &[Stage]) -> String {
    stages.iter().map(|s| format!("@{}:lavfi=[{}]", s.label, s.graph())).collect::<Vec<_>>().join(",")
}

/// A float as FFmpeg reads it, without noise from the f32.
fn num(v: f32) -> String {
    let mut s = format!("{v:.3}");
    if s.contains('.') {
        s.truncate(s.trim_end_matches('0').trim_end_matches('.').len());
    }
    if s == "-0" { "0".to_string() } else { s }
}

fn band(instance: String, hz: f32, q: f32, db: f32) -> Filter {
    Filter {
        name: "equalizer",
        instance: Some(instance),
        fixed: vec![("t", "q".into())],
        values: vec![
            Value { option: "f", value: num(hz), neutral: None },
            Value { option: "w", value: num(q), neutral: None },
            Value { option: "g", value: num(db), neutral: Some("0") },
        ],
    }
}

/// The channels and their names as `pan` wants them, for the layouts it is used on.
fn centre_pan(layout: &str) -> Filter {
    let gain = 10f32.powf(DIALOGUE_CENTRE_DB / 20.0);
    Filter {
        name: "pan",
        instance: None,
        fixed: vec![("", format!("{layout}|c0=c0|c1=c1|c2={}*c2|c3=c3|c4=c4|c5=c5", num(gain)))],
        values: Vec::new(),
    }
}

/// What the chain needs to know about the playing source; both unknown until its audio is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    pub channels: Option<i64>,
    /// `audio-params/channels` ("5.1", "5.1(side)", "stereo"…).
    pub layout: Option<String>,
}

/// The stages for `fx`, in play order: gain, in, eq, dyn, spatial, norm, out. Effects whose filter
/// `caps` lacks are left out. `crossfeed` and correlation take stereo only (FFmpeg would quietly
/// downmix anything else), so they wait until the source is known to be stereo.
pub fn chain(fx: &AudioFx, caps: &Caps, source: &Source) -> Vec<Stage> {
    let channels = source.channels;
    let lifted_layout = source.layout.as_deref().filter(|l| {
        fx.dialogue && channels == Some(6) && caps.eq() && caps.has("pan") && matches!(*l, "5.1" | "5.1(side)")
    });
    let mut effects = Vec::new();
    if fx.enabled {
        // eq: the centre lift, then all bands, so moving a slider off 0 needs no new filter.
        let mut eq = Vec::new();
        if caps.eq() {
            if let Some(layout) = lifted_layout {
                eq.push(centre_pan(layout));
            }
            if fx.eq.iter().any(|b| b.db != 0.0) || fx.dialogue {
                for (i, b) in fx.eq.iter().enumerate() {
                    eq.push(band(
                        format!("b{i}"),
                        clamp(b.hz as f32, 20.0, 20000.0, 1000.0),
                        clamp(b.q, 0.1, 10.0, 1.0),
                        clamp(b.db, -24.0, 24.0, 0.0),
                    ));
                }
            }
            if fx.dialogue {
                eq.push(band("d0".into(), 2500.0, 1.0, DIALOGUE_BOOST_DB));
                eq.push(band("d1".into(), 250.0, 1.2, DIALOGUE_CUT_DB));
            }
        }
        if !eq.is_empty() {
            effects.push(Stage { label: "eq", filters: eq });
        }
        if let Some(n) = fx.night.as_ref().filter(|_| caps.night()) {
            effects.push(Stage {
                label: "dyn",
                filters: vec![
                    Filter {
                        name: "acompressor",
                        instance: None,
                        fixed: vec![("attack", "20".into()), ("release", "400".into()), ("knee", "6dB".into())],
                        values: vec![
                            Value {
                                option: "threshold",
                                value: format!("{}dB", num(clamp(n.threshold_db, -60.0, 0.0, -24.0))),
                                neutral: None,
                            },
                            Value { option: "ratio", value: num(clamp(n.ratio, 1.0, 20.0, 4.0)), neutral: None },
                            Value {
                                option: "makeup",
                                value: format!("{}dB", num(clamp(n.makeup_db, 0.0, 24.0, 6.0))),
                                neutral: Some("0dB"),
                            },
                            Value { option: "mix", value: "1".into(), neutral: Some("0") },
                        ],
                    },
                    Filter {
                        name: "alimiter",
                        instance: None,
                        fixed: vec![("level", "false".into())],
                        values: vec![Value { option: "limit", value: num(LIMIT_LINEAR), neutral: Some("1") }],
                    },
                ],
            });
        }
        if let Some(c) = fx.crossfeed.as_ref().filter(|_| caps.crossfeed() && channels == Some(2)) {
            effects.push(Stage {
                label: "spatial",
                filters: vec![Filter {
                    name: "crossfeed",
                    instance: None,
                    fixed: Vec::new(),
                    values: vec![
                        Value { option: "strength", value: num(clamp(c.strength, 0.0, 1.0, 0.5)), neutral: Some("0") },
                        Value { option: "range", value: num(clamp(c.range, 0.0, 1.0, 0.5)), neutral: None },
                    ],
                }],
            });
        }
        if fx.loudness == Loudness::Live && caps.loudness() {
            effects.push(Stage {
                label: "norm",
                filters: vec![Filter {
                    name: "dynaudnorm",
                    instance: None,
                    // Holds about g × f of audio (1 s here), without moving A/V sync.
                    fixed: vec![("f", "200".into()), ("g", "5".into()), ("p", "0.9".into()), ("m", "10".into())],
                    values: Vec::new(),
                }],
            });
        }
    }
    let mut out = Vec::new();
    if !effects.is_empty() && caps.has("volume") {
        // For the boosts that run: without an equalizer there are none, and it would only be quieter.
        let headroom = if caps.eq() { fx.headroom_db(lifted_layout.is_some()) } else { 0.0 };
        out.push(Stage {
            label: "gain",
            filters: vec![Filter {
                name: "volume",
                instance: None,
                fixed: Vec::new(),
                values: vec![Value { option: "volume", value: format!("{}dB", num(headroom)), neutral: Some("0dB") }],
            }],
        });
    }
    let meters = fx.meters && caps.meters();
    if meters && !effects.is_empty() {
        out.push(Stage { label: "in", filters: vec![astats()] });
    }
    out.extend(effects);
    if meters {
        let mut filters = vec![
            astats(),
            Filter {
                name: "ebur128",
                instance: None,
                fixed: vec![("metadata", "1".into()), ("peak", "true".into())],
                values: Vec::new(),
            },
        ];
        if channels == Some(2) && caps.has("aphasemeter") {
            filters.push(Filter {
                name: "aphasemeter",
                instance: None,
                fixed: vec![("video", "0".into()), ("phasing", "0".into())],
                values: Vec::new(),
            });
        }
        out.push(Stage { label: "out", filters });
    }
    out
}

fn astats() -> Filter {
    Filter {
        name: "astats",
        instance: None,
        fixed: vec![
            ("metadata", "1".into()),
            ("reset", "1".into()),
            ("measure_overall", "none".into()),
            ("measure_perchannel", "Peak_level+RMS_level".into()),
        ],
        values: Vec::new(),
    }
}

/// One `af-command`: label, option, value, target.
pub type Command = [String; 4];

/// How to get from the stages mpv runs to the wanted ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// A change of structure: the new value of `af`. mpv restarts the filters, which also starts
    /// the meters' integrated loudness over.
    pub set_af: Option<String>,
    pub commands: Vec<Command>,
    /// What runs afterwards.
    pub applied: Vec<Stage>,
}

/// The way from `applied` to `want`. Nothing wanted always clears the chain for real, so "all off"
/// has no filters left. Otherwise an effect turned off stays in place doing nothing if its filters
/// can (the next change of structure drops it); `target`: mpv can address a filter at all.
pub fn plan(applied: &[Stage], want: &[Stage], target: bool) -> Plan {
    if want.is_empty() {
        return Plan { set_af: (!applied.is_empty()).then(String::new), commands: Vec::new(), applied: Vec::new() };
    }
    // `want`, with what can stay bypassed kept in its place.
    let mut merged: Vec<Stage> = Vec::new();
    let mut wanted = want.iter().peekable();
    for a in applied {
        while let Some(w) = wanted.next_if(|w| !applied.iter().any(|x| x.label == w.label)) {
            merged.push(w.clone());
        }
        match wanted.next_if(|w| w.label == a.label) {
            Some(w) => merged.push(w.clone()),
            None => match a.bypassed() {
                Some(b) if !want.iter().any(|w| w.label == a.label) => merged.push(b),
                _ => {}
            },
        }
    }
    merged.extend(wanted.cloned());
    let same = target && merged.len() == applied.len() && merged.iter().zip(applied).all(|(m, a)| m.same_structure(a));
    if !same {
        let unchanged = want == applied;
        return Plan { set_af: (!unchanged).then(|| af_string(want)), commands: Vec::new(), applied: want.to_vec() };
    }
    let mut commands = Vec::new();
    for (m, a) in merged.iter().zip(applied) {
        for (mf, af) in m.filters.iter().zip(&a.filters) {
            for (mv, av) in mf.values.iter().zip(&af.values) {
                if mv.value != av.value {
                    commands.push([m.label.to_string(), mv.option.to_string(), mv.value.clone(), mf.target()]);
                }
            }
        }
    }
    Plan { set_af: None, commands, applied: merged }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEREO: fn() -> Source = || Source { channels: Some(2), layout: Some("stereo".into()) };

    fn on() -> AudioFx {
        AudioFx { enabled: true, ..Default::default() }
    }

    fn af(fx: &AudioFx, s: &Source) -> String {
        af_string(&chain(fx, &Caps::assumed(), s))
    }

    #[test]
    fn the_default_is_no_filter_at_all() {
        assert_eq!(af(&AudioFx::default(), &STEREO()), "");
        assert_eq!(af(&on(), &STEREO()), "", "on, with nothing to do");
        let mut off = AudioFx {
            eq: flat_eq(),
            night: Some(Night::default()),
            crossfeed: Some(Crossfeed::default()),
            ..Default::default()
        };
        off.eq[0].db = 6.0;
        assert_eq!(af(&off, &STEREO()), "", "effects chosen but switched off");
    }

    #[test]
    fn night_string() {
        let mut fx = AudioFx::default();
        fx.apply_preset("night");
        assert_eq!(
            af(&fx, &STEREO()),
            "@gain:lavfi=[volume=volume=0dB],\
             @dyn:lavfi=[acompressor=attack=20:release=400:knee=6dB:threshold=-24dB:ratio=4:makeup=6dB:mix=1,\
             alimiter=level=false:limit=0.891]"
        );
    }

    #[test]
    fn dialogue_strings() {
        let mut fx = AudioFx::default();
        fx.apply_preset("dialogue");
        let stereo = af(&fx, &STEREO());
        assert!(stereo.starts_with("@gain:lavfi=[volume=volume=-3dB],@eq:lavfi=[equalizer@b0="), "{stereo}");
        assert!(stereo.contains("equalizer@d0=t=q:f=2500:w=1:g=3,equalizer@d1=t=q:f=250:w=1.2:g=-2"), "{stereo}");
        assert!(!stereo.contains("pan="));
        let surround = Source { channels: Some(6), layout: Some("5.1".into()) };
        let s = af(&fx, &surround);
        assert!(s.contains("@eq:lavfi=[pan=5.1|c0=c0|c1=c1|c2=1.585*c2|c3=c3|c4=c4|c5=c5,equalizer@b0"), "{s}");
        assert!(s.starts_with("@gain:lavfi=[volume=volume=-7dB]"), "{s}");
    }

    #[test]
    fn crossfeed_only_on_stereo() {
        let mut fx = AudioFx::default();
        fx.apply_preset("crossfeed");
        assert_eq!(
            af(&fx, &STEREO()),
            "@gain:lavfi=[volume=volume=0dB],@spatial:lavfi=[crossfeed=strength=0.5:range=0.5]"
        );
        assert_eq!(af(&fx, &Source { channels: Some(6), layout: None }), "");
        assert_eq!(af(&fx, &Source::default()), "");
    }

    #[test]
    fn loudness_is_the_short_leveller() {
        let fx = AudioFx { loudness: Loudness::Live, ..on() };
        assert_eq!(af(&fx, &STEREO()), "@gain:lavfi=[volume=volume=0dB],@norm:lavfi=[dynaudnorm=f=200:g=5:p=0.9:m=10]");
        assert_eq!(af(&AudioFx { loudness: Loudness::Live, ..Default::default() }, &STEREO()), "");
    }

    #[test]
    fn meters_alone_add_only_the_out_stage() {
        let fx = AudioFx { meters: true, ..Default::default() };
        assert_eq!(
            af(&fx, &STEREO()),
            "@out:lavfi=[astats=metadata=1:reset=1:measure_overall=none:measure_perchannel=Peak_level+RMS_level,\
             ebur128=metadata=1:peak=true,aphasemeter=video=0:phasing=0]"
        );
        // Correlation would downmix anything else.
        assert!(!af(&fx, &Source { channels: Some(6), layout: None }).contains("aphasemeter"));
        assert!(!af(&fx, &Source::default()).contains("aphasemeter"));
    }

    #[test]
    fn stages_come_in_play_order() {
        let mut fx = AudioFx { meters: true, loudness: Loudness::Live, ..on() };
        fx.eq[3].db = 2.0;
        fx.night = Some(Night::default());
        fx.crossfeed = Some(Crossfeed::default());
        let labels: Vec<_> = chain(&fx, &Caps::assumed(), &STEREO()).iter().map(|s| s.label).collect();
        assert_eq!(labels, ["gain", "in", "eq", "dyn", "spatial", "norm", "out"]);
    }

    #[test]
    fn headroom_is_the_largest_boost() {
        let mut fx = on();
        assert_eq!(fx.headroom_db(false), 0.0);
        fx.eq[2].db = -8.0;
        assert_eq!(fx.headroom_db(false), 0.0, "cuts need none");
        fx.eq[2].db = 5.5;
        fx.eq[7].db = 2.0;
        assert_eq!(fx.headroom_db(false), -5.5);
        assert!(af(&fx, &STEREO()).starts_with("@gain:lavfi=[volume=volume=-5.5dB]"));
        fx.dialogue = true;
        assert_eq!(fx.headroom_db(false), -8.5);
        assert_eq!(fx.headroom_db(true), -12.5);
    }

    /// Only what runs is made room for: a quiet gain stage with no boost behind it just turns it down.
    #[test]
    fn headroom_counts_the_boosts_that_run() {
        let mut fx = on();
        fx.dialogue = true;
        // Six channels in a layout without a known centre: no lift, so no room for one.
        let odd = Source { channels: Some(6), layout: Some("hexagonal".into()) };
        let gain = |fx: &AudioFx, caps: &Caps, s: &Source| af_string(&chain(fx, caps, s));
        assert!(gain(&fx, &Caps::assumed(), &odd).starts_with("@gain:lavfi=[volume=volume=-3dB]"));
        let five = Source { channels: Some(6), layout: Some("5.1".into()) };
        assert!(gain(&fx, &Caps::assumed(), &five).starts_with("@gain:lavfi=[volume=volume=-7dB]"));
        // A libmpv without the equalizer, with something else on so that a chain is built.
        fx.eq[2].db = 12.0;
        fx.night = Some(Night::default());
        let caps =
            Caps { filters: Caps::assumed().filters.into_iter().filter(|f| f != "equalizer").collect(), target: true };
        assert!(
            gain(&fx, &caps, &STEREO()).starts_with("@gain:lavfi=[volume=volume=0dB]"),
            "{}",
            gain(&fx, &caps, &STEREO())
        );
    }

    #[test]
    fn garbage_from_a_settings_file_stays_in_range() {
        let mut fx = on();
        fx.eq = vec![Band { hz: 0, q: f32::NAN, db: f32::INFINITY }, Band { hz: 4_000_000, q: 1e9, db: -1e9 }];
        fx.night = Some(Night { threshold_db: 40.0, ratio: 0.0, makeup_db: f32::NAN });
        fx.crossfeed = Some(Crossfeed { strength: 9.0, range: -3.0 });
        let s = af(&fx, &STEREO());
        for part in [
            "equalizer@b0=t=q:f=20:w=1:g=0",
            "equalizer@b1=t=q:f=20000:w=10:g=-24",
            "threshold=0dB:ratio=1:makeup=6dB",
            "strength=1:range=0",
        ] {
            assert!(s.contains(part), "{part} not in {s}");
        }
    }

    #[test]
    fn missing_filters_leave_their_effect_out() {
        let mut fx = AudioFx { meters: true, loudness: Loudness::Live, ..on() };
        fx.eq[0].db = 3.0;
        fx.night = Some(Night::default());
        fx.crossfeed = Some(Crossfeed::default());
        let caps = Caps { filters: ["volume", "alimiter"].iter().map(|f| f.to_string()).collect(), target: true };
        assert!(chain(&fx, &caps, &STEREO()).is_empty());
    }

    #[test]
    fn presets_and_flat() {
        let mut fx = AudioFx { loudness: Loudness::Live, meters: true, ..Default::default() };
        fx.apply_preset("crossfeed");
        assert!(fx.enabled && fx.crossfeed.is_some());
        assert_eq!(fx.preset.as_deref(), Some("crossfeed"));
        fx.apply_preset("dialogue");
        assert!(fx.crossfeed.is_none() && fx.dialogue);
        fx.flatten();
        assert!(!fx.enabled && !fx.dialogue && fx.preset.is_none());
        assert_eq!(fx.loudness, Loudness::Live, "a separate choice");
        assert!(fx.meters, "a separate choice");
    }

    #[test]
    fn toml_round_trip() {
        #[derive(Serialize, Deserialize, PartialEq, Debug, Default)]
        #[serde(default)]
        struct Holder {
            fx: AudioFx,
            preset: UserPreset,
            profile: AudioProfile,
        }
        let mut h = Holder::default();
        assert_eq!(toml::from_str::<Holder>(&toml::to_string(&h).unwrap()).unwrap(), h);
        h.fx.apply_preset("night");
        h.fx.crossfeed = Some(Crossfeed { strength: 0.25, range: 0.75 });
        h.fx.loudness = Loudness::Live;
        h.fx.eq[4].db = -3.5;
        h.preset = UserPreset { name: "Late".into(), fx: h.fx.clone() };
        h.profile = AudioProfile {
            name: "Receiver".into(),
            audio_device: "wasapi/x".into(),
            passthrough: true,
            ..Default::default()
        };
        let text = toml::to_string(&h).unwrap();
        assert_eq!(toml::from_str::<Holder>(&text).unwrap(), h, "{text}");
        assert!(text.contains("loudness = \"live\""), "{text}");
        // Absent fields take their defaults.
        let partial: AudioFx = toml::from_str("enabled = true\n").unwrap();
        assert_eq!(partial, on());
    }

    fn stages(fx: &AudioFx) -> Vec<Stage> {
        chain(fx, &Caps::assumed(), &STEREO())
    }

    #[test]
    fn a_value_change_is_a_command() {
        let mut fx = on();
        fx.eq[2].db = 3.0;
        let a = stages(&fx);
        fx.eq[2].db = 4.5;
        fx.eq[5].db = -2.0;
        let p = plan(&a, &stages(&fx), true);
        assert_eq!(p.set_af, None);
        assert_eq!(
            p.commands,
            vec![
                ["gain", "volume", "-4.5dB", "volume"].map(String::from),
                ["eq", "g", "4.5", "equalizer@b2"].map(String::from),
                ["eq", "g", "-2", "equalizer@b5"].map(String::from),
            ]
        );
        assert_eq!(plan(&p.applied, &stages(&fx), true), Plan { applied: p.applied.clone(), ..Default::default() });
    }

    #[test]
    fn a_new_stage_sets_af_in_play_order() {
        let mut fx = on();
        fx.eq[2].db = 3.0;
        let a = stages(&fx);
        fx.night = Some(Night::default());
        let p = plan(&a, &stages(&fx), true);
        assert!(p.commands.is_empty());
        let af = p.set_af.unwrap();
        assert!(af.find("@eq:").unwrap() < af.find("@dyn:").unwrap(), "{af}");
    }

    #[test]
    fn an_effect_turned_off_is_bypassed_then_dropped() {
        let mut fx = on();
        fx.eq[2].db = 3.0;
        fx.night = Some(Night::default());
        let a = stages(&fx);
        fx.night = None;
        let p = plan(&a, &stages(&fx), true);
        assert_eq!(p.set_af, None, "no re-initialisation while listening");
        assert!(p.commands.contains(&["dyn", "mix", "0", "acompressor"].map(String::from)), "{:?}", p.commands);
        assert!(p.commands.contains(&["dyn", "limit", "1", "alimiter"].map(String::from)), "{:?}", p.commands);
        assert_eq!(p.applied.len(), 3, "still running");
        // It comes back with commands too.
        fx.night = Some(Night::default());
        let back = plan(&p.applied, &stages(&fx), true);
        assert_eq!(back.set_af, None);
        assert!(back.commands.contains(&["dyn", "mix", "1", "acompressor"].map(String::from)));
        // The next change of structure doesn't carry the bypassed stage along.
        fx.night = None;
        let off = plan(&back.applied, &stages(&fx), true);
        fx.crossfeed = Some(Crossfeed::default());
        let want = stages(&fx);
        let p = plan(&off.applied, &want, true);
        assert!(!p.set_af.as_ref().unwrap().contains("@dyn"), "{:?}", p.set_af);
        assert_eq!(p.applied, want);
    }

    #[test]
    fn everything_off_really_removes_the_filters() {
        let mut fx = AudioFx::default();
        fx.apply_preset("night");
        let a = stages(&fx);
        fx.flatten();
        let p = plan(&a, &stages(&fx), true);
        assert_eq!(p.set_af.as_deref(), Some(""));
        assert!(p.applied.is_empty());
        // And nothing to do when it was off already.
        assert_eq!(plan(&[], &[], true), Plan::default());
    }

    #[test]
    fn a_stage_with_nothing_to_bypass_goes_away() {
        let mut fx = AudioFx { loudness: Loudness::Live, night: Some(Night::default()), ..on() };
        let a = stages(&fx);
        fx.loudness = Loudness::Off;
        let p = plan(&a, &stages(&fx), true);
        let af = p.set_af.unwrap();
        assert!(!af.contains("@norm") && af.contains("@dyn"), "{af}");
    }

    #[test]
    fn without_a_target_every_change_sets_af() {
        let mut fx = on();
        fx.eq[2].db = 3.0;
        let a = stages(&fx);
        assert_eq!(plan(&a, &a, false).set_af, None, "nothing changed");
        fx.eq[2].db = 4.0;
        let p = plan(&a, &stages(&fx), false);
        assert!(p.commands.is_empty());
        assert!(p.set_af.unwrap().contains("equalizer@b2=t=q:f=125:w=1:g=4"));
    }
}
