//! The player's sound: what this libmpv can do, and what the player hands over to be saved.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

use corrent_core::audio_fx::{Caps, FILTERS};
use corrent_mpv::{Mpv, Node};

static PROBED: OnceLock<Caps> = OnceLock::new();
static PROBING: AtomicBool = AtomicBool::new(false);
static ASSUMED: LazyLock<Caps> = LazyLock::new(Caps::assumed);

/// What this libmpv can do, for use on the UI thread: the first call starts the probe in the
/// background, and until it has answered everything is assumed to be there.
pub fn caps() -> &'static Caps {
    if let Some(c) = PROBED.get() {
        return c;
    }
    if !PROBING.swap(true, Ordering::Relaxed) {
        let started = std::thread::Builder::new().name("mpv-filter-probe".into()).spawn(|| match probe() {
            Ok(c) => {
                tracing::debug!("sound filters: {:?}", c);
                let _ = PROBED.set(c);
            }
            Err(e) => tracing::debug!("sound filter probe: {e}"),
        });
        if let Err(e) = started {
            tracing::debug!("sound filter probe: {e}");
        }
    }
    &ASSUMED
}

/// Asks a headless mpv: the bridged form of a filter (`@probe:name`) is refused when the name is
/// unknown, while `lavfi=[graph]` text would pass, being read only when it runs. Blocking.
fn probe() -> Result<Caps, corrent_mpv::Error> {
    let mpv = Mpv::headless(&[("ao", "null")])?;
    // Without this answer the rest would say nothing.
    if mpv.set_property("af", "@probe:nosuchfilter_xyz").is_ok() {
        return Ok(Caps::assumed());
    }
    let filters = FILTERS
        .iter()
        .filter(|f| mpv.set_property("af", &format!("@probe:{f}")).is_ok())
        .map(|f| f.to_string())
        .collect();
    let list = mpv.get("command-list")?;
    let target = list.as_array().iter().any(|c| {
        c.get("name").and_then(Node::as_str) == Some("af-command")
            && c.get("args")
                .is_some_and(|a| a.as_array().iter().any(|a| a.get("name").and_then(Node::as_str) == Some("target")))
    });
    Ok(Caps { filters, target })
}

/// A closing player has no way to the settings, so what is left to save waits here for the next
/// frame.
static VOLUME: Mutex<Option<(f32, bool)>> = Mutex::new(None);

pub(super) fn post_volume(volume: f32, muted: bool) {
    *crate::util::lock(&VOLUME) = Some((volume, muted));
}

pub(super) fn take_volume() -> Option<(f32, bool)> {
    crate::util::lock(&VOLUME).take()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use corrent_core::audio_fx::{AudioFx, Crossfeed, Loudness, Night, Source, af_string, chain, plan};
    use corrent_mpv::{Event, Library};

    use super::*;

    fn play(mpv: &Mpv, url: &str) {
        mpv.command(&["loadfile", url]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Some(Event::PlaybackRestart) = mpv.wait_event(0.1) {
                return;
            }
        }
        panic!("{url} never started");
    }

    fn level(mpv: &Mpv, key: &str, after: f64) -> f64 {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            let at = mpv.get_f64("time-pos").ok().flatten().unwrap_or(0.0);
            if at >= after
                && let Ok(meta) = mpv.get("af-metadata/out")
                && let Some(v) = meta.get(key).and_then(Node::as_str).and_then(|v| v.parse().ok())
            {
                return v;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("{key} never appeared");
    }

    fn labels(mpv: &Mpv) -> Vec<String> {
        mpv.get("af")
            .unwrap()
            .as_array()
            .iter()
            .filter_map(|f| f.get("label").and_then(Node::as_str).map(str::to_string))
            .collect()
    }

    #[test]
    fn the_probe_finds_what_the_chain_uses() {
        if Library::get().is_err() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let caps = probe().unwrap();
        println!("{caps:?}");
        for f in ["equalizer", "acompressor", "alimiter", "crossfeed", "dynaudnorm", "astats", "ebur128", "aphasemeter"]
        {
            assert!(caps.has(f), "{f}");
        }
        assert!(caps.has("pan") && caps.has("volume"));
        assert!(caps.target);
    }

    fn with_everything(meters: bool) -> AudioFx {
        let mut fx = AudioFx {
            enabled: true,
            meters,
            night: Some(Night::default()),
            crossfeed: Some(Crossfeed::default()),
            dialogue: true,
            loudness: Loudness::Live,
            ..Default::default()
        };
        fx.eq[2].db = 3.0;
        fx
    }

    /// Everything the builder makes runs on a real libmpv, and every command a plan sends is taken.
    #[test]
    fn built_chains_run_and_their_plans_apply() {
        if Library::get().is_err() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let caps = Caps::assumed();
        let stereo = Source { channels: Some(2), layout: Some("stereo".into()) };
        let mut fx = with_everything(true);
        let first = chain(&fx, &caps, &stereo);
        let mpv = Mpv::headless(&[("ao", "null"), ("af", &af_string(&first))]).unwrap();
        play(&mpv, "av://lavfi:sine=f=1000:r=48000:d=8,aformat=channel_layouts=stereo");
        level(&mpv, "lavfi.r128.M", 0.2);
        assert_eq!(labels(&mpv), ["gain", "in", "eq", "dyn", "spatial", "norm", "out"]);
        assert_eq!(mpv.get("audio-out-params").unwrap().get("channel-count").and_then(Node::as_i64), Some(2));

        // Values go by command, and each is accepted.
        fx.eq[2].db = 5.0;
        fx.eq[7].db = -2.5;
        fx.night = Some(Night { threshold_db: -30.0, ratio: 2.5, makeup_db: 3.0 });
        fx.crossfeed = Some(Crossfeed { strength: 0.2, range: 0.8 });
        let p = plan(&first, &chain(&fx, &caps, &stereo), true);
        assert_eq!(p.set_af, None);
        assert!(p.commands.len() >= 8, "{:?}", p.commands);
        for [label, option, value, target] in &p.commands {
            let r = mpv.command(&["af-command", label, option, value, target]);
            assert!(r.is_ok(), "af-command {label} {option} {value} {target}: {r:?}");
        }
        // Bypassing too.
        fx.night = None;
        fx.crossfeed = None;
        let q = plan(&p.applied, &chain(&fx, &caps, &stereo), true);
        assert_eq!(q.set_af, None);
        for [label, option, value, target] in &q.commands {
            let r = mpv.command(&["af-command", label, option, value, target]);
            assert!(r.is_ok(), "af-command {label} {option} {value} {target}: {r:?}");
        }
        // And all off leaves no filter.
        let off = plan(&q.applied, &chain(&AudioFx::default(), &caps, &stereo), true);
        mpv.set_property("af", off.set_af.as_deref().unwrap()).unwrap();
        assert!(mpv.get("af").unwrap().as_array().is_empty());
    }

    /// The centre lift on 5.1: the layout stays, and only the centre gains on the others.
    #[test]
    fn dialogue_lifts_the_centre_of_5_1() {
        if Library::get().is_err() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let url = "av://lavfi:sine=f=1000:r=48000:d=6,aformat=channel_layouts=5.1";
        let source = Source { channels: Some(6), layout: Some("5.1".into()) };
        let mut fx = AudioFx { meters: true, ..Default::default() };
        let plain =
            Mpv::headless(&[("ao", "null"), ("af", &af_string(&chain(&fx, &Caps::assumed(), &source)))]).unwrap();
        play(&plain, url);
        let before = level(&plain, "lavfi.astats.3.RMS_level", 1.0);
        fx.apply_preset("dialogue");
        fx.meters = true;
        let af = af_string(&chain(&fx, &Caps::assumed(), &source));
        assert!(af.contains("pan=5.1|"), "{af}");
        let mpv = Mpv::headless(&[("ao", "null"), ("af", &af)]).unwrap();
        play(&mpv, url);
        let after = level(&mpv, "lavfi.astats.3.RMS_level", 1.0);
        let out = mpv.get("audio-out-params").unwrap();
        println!("centre {before} -> {after}, {out:?}");
        assert_eq!(out.get("channel-count").and_then(Node::as_i64), Some(6));
        assert_eq!(out.get("channels").and_then(Node::as_str), Some("5.1"));
        // -7 dB headroom, +4 dB centre, and a 1 kHz tone is on the skirt of +3 dB at 2.5 kHz.
        let change = after - before;
        assert!((-3.5..=0.0).contains(&change), "{change}");
    }

    /// Nothing on: nothing runs and no metadata arrives.
    #[test]
    fn off_means_no_filter() {
        if Library::get().is_err() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let af = corrent_core::config::Playback::default()
            .mpv_options()
            .into_iter()
            .find(|(k, _)| *k == "af")
            .map(|(_, v)| v)
            .unwrap();
        assert_eq!(af, "");
        let mpv = Mpv::headless(&[("ao", "null"), ("af", &af)]).unwrap();
        play(&mpv, "av://lavfi:sine=f=1000:r=48000:d=2");
        assert!(mpv.get("af").unwrap().as_array().is_empty());
        assert!(mpv.get("af-metadata/out").is_err());
        // The source's own format reaches the output untouched.
        assert_eq!(mpv.get("audio-out-params").unwrap().get("format").and_then(Node::as_str), Some("s16"));
    }
}
