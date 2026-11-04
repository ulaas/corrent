//! Proves the mechanism real-time audio processing rests on: labelled lavfi stages surface frame
//! metadata through `af-metadata/<label>`, `af-command` changes a value without touching `af`, and
//! metering doesn't change the rate or channel layout of what is played.
//!
//! Measured on the pinned build (mpv v0.41.0-1224-g6a262d11f, FFmpeg N-127683-gc2bbf2b83), 2026-11-05:
//!
//! - `sine` is an eighth of full scale, not full scale: peak -18.06 dBFS, RMS -21.07, M -21.07 LUFS,
//!   and true peak 0.125 (linear, not dB; `lavfi.r128.true_peaks_chN`, N from 0; `astats` channels
//!   count from 1). Full scale (`aevalsrc`): peak 0.00, RMS -3.00, M -3.00 LUFS, true peak 1.000.
//! - `lavfi.r128.S` is -120.7 until 3 s of signal have passed.
//! - `af-command gain volume -6dB volume`: RMS -21.10 → -27.06, `af` unchanged. The default target
//!   `all` fails with -12 ("error running command") because mpv's graph holds filters that refuse
//!   the option, so a target is always passed: the filter's name or `name@instance`
//!   (`equalizer@b1` works: +6 dB on a band at the tone gave +6.04 dB; the `Parsed_…` form fails).
//! - `observe("af-metadata/out")` delivers maps at about 10 Hz (26 in 2.5 s).
//! - 44.1 kHz: `audio-out-params/samplerate` stays 44100 with `ebur128` active, whether the stage is
//!   there from the start or added while playing. M -21.07 LUFS, the same as at 48 kHz.
//! - 5.1: `audio-out-params/channel-count` stays 6 with `@out`.
//! - With `@out` there from the start the output format is `double` (`ebur128`'s) under `ao=null`;
//!   added while playing the running AO stays `s16`, so turning meters on doesn't reopen it.
//! - A whole-`af` set restarts `ebur128` even when `@out`'s text is the same (I jumps from -21.07 to
//!   -33.07 with -12 dB put in front of it, no averaging): a structure change resets I and LRA.
//! - `dynaudnorm` holds about g × f of audio, not (g-1)/2 × f: on a source limited to real time,
//!   loadfile→restart is 224 ms without it, 1237 ms with `f=200:g=5`, 1687 ms with `f=200:g=7`,
//!   4019 ms with `f=250:g=15`. From a file (decoded far faster than real time) the difference is
//!   under 3 ms. The first `af-metadata/out` sample arrives within 1 ms of `PlaybackRestart`, and
//!   `time-pos` at that moment is 0.021 s in every case, so the frames keep their timestamps and
//!   A/V sync doesn't shift.
//! - Probe: `@probe:nosuchfilter_xyz` is refused (-9); `@probe:lavfi=[nosuchfilter_xyz]` isn't, as
//!   a graph only parses when it is instantiated.

use std::time::{Duration, Instant};

use crate::{Event, Library, Mpv, Node};

const METER: &str = "@out:lavfi=[astats=metadata=1:reset=1:measure_overall=none:measure_perchannel=Peak_level+RMS_level,\
                     ebur128=metadata=1:peak=true]";

/// An instance with `af`, or `None` where libmpv is absent (those machines skip these tests).
fn start(af: &str) -> Option<Mpv> {
    if Library::get().is_err() {
        eprintln!("libmpv not found; skipping");
        return None;
    }
    Some(Mpv::headless(&[("ao", "null"), ("af", af)]).unwrap())
}

/// Older libmpvs (CI's distro build) are held to the mechanism, this build to its exact behaviour.
fn pinned(mpv: &Mpv) -> bool {
    mpv.get_string("mpv-version").is_ok_and(|v| v.starts_with("mpv v0.41"))
}

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

fn meta(mpv: &Mpv, key: &str) -> Option<f64> {
    mpv.get("af-metadata/out").ok()?.get(key)?.as_str()?.parse().ok()
}

/// Waits until playback reached `at` seconds, then reads `key`.
fn meta_at(mpv: &Mpv, at: f64, key: &str) -> f64 {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if mpv.get_f64("time-pos").ok().flatten().is_some_and(|t| t >= at)
            && let Some(v) = meta(mpv, key)
        {
            return v;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{key} never appeared");
}

fn out_param(mpv: &Mpv, key: &str) -> i64 {
    mpv.get("audio-out-params").unwrap().get(key).and_then(Node::as_i64).unwrap()
}

fn near(what: &str, got: f64, want: f64, by: f64) {
    println!("{what}: {got}");
    assert!((got - want).abs() <= by, "{what}: {got}, expected {want} ± {by}");
}

#[test]
fn metadata_keys_and_levels() {
    let Some(mpv) = start(METER) else { return };
    println!("{:?} {:?}", mpv.get("mpv-version"), mpv.get("ffmpeg-version"));
    play(&mpv, "av://lavfi:sine=f=1000:r=48000:d=4");
    near("sine M", meta_at(&mpv, 1.0, "lavfi.r128.M"), -21.07, 0.5);
    near("sine peak", meta_at(&mpv, 1.0, "lavfi.astats.1.Peak_level"), -18.06, 0.3);
    near("sine RMS", meta_at(&mpv, 1.0, "lavfi.astats.1.RMS_level"), -21.07, 0.5);
    near("sine true peak (linear)", meta_at(&mpv, 1.0, "lavfi.r128.true_peaks_ch0"), 0.125, 0.01);

    let Some(mpv) = start(METER) else { return };
    play(&mpv, "av://lavfi:aevalsrc=sin(2*PI*1000*t):s=48000:d=4");
    near("full scale M", meta_at(&mpv, 1.0, "lavfi.r128.M"), -3.0, 0.5);
    near("full scale peak", meta_at(&mpv, 1.0, "lavfi.astats.1.Peak_level"), 0.0, 0.3);
}

#[test]
fn af_command_changes_a_value_without_touching_af() {
    let Some(mpv) = start(&format!("@gain:lavfi=[volume=0dB],{METER}")) else { return };
    play(&mpv, "av://lavfi:sine=f=1000:r=48000:d=6");
    let before = meta_at(&mpv, 1.0, "lavfi.astats.1.RMS_level");
    let af = mpv.get("af").unwrap();
    let pos = mpv.get_f64("time-pos").unwrap().unwrap();
    mpv.command(&["af-command", "gain", "volume", "-6dB", "volume"]).unwrap();
    let after = meta_at(&mpv, pos + 1.0, "lavfi.astats.1.RMS_level");
    near("RMS change from af-command", after - before, -6.0, 0.5);
    assert_eq!(mpv.get("af").unwrap(), af, "af stays as it was");
    // Why a target is always passed.
    if pinned(&mpv) {
        assert!(mpv.command(&["af-command", "gain", "volume", "-6dB"]).is_err());
    }
}

#[test]
fn a_named_instance_inside_a_graph_can_be_addressed() {
    let af = format!("@eq:lavfi=[equalizer@b1=f=1000:t=q:w=1:g=0,equalizer@b2=f=8000:t=q:w=1:g=0],{METER}");
    let Some(mpv) = start(&af) else { return };
    play(&mpv, "av://lavfi:sine=f=1000:r=48000:d=6");
    let before = meta_at(&mpv, 1.0, "lavfi.astats.1.RMS_level");
    let pos = mpv.get_f64("time-pos").unwrap().unwrap();
    // The other band first: a tone at 1 kHz must not hear it.
    mpv.command(&["af-command", "eq", "g", "12", "equalizer@b2"]).unwrap();
    mpv.command(&["af-command", "eq", "g", "6", "equalizer@b1"]).unwrap();
    let after = meta_at(&mpv, pos + 1.0, "lavfi.astats.1.RMS_level");
    near("RMS change from one band", after - before, 6.0, 0.8);
}

#[test]
fn command_list_shows_the_target_argument() {
    let Some(mpv) = start("") else { return };
    let list = mpv.get("command-list").unwrap();
    let args = list
        .as_array()
        .iter()
        .find(|c| c.get("name").and_then(Node::as_str) == Some("af-command"))
        .and_then(|c| c.get("args"))
        .map(|a| a.as_array().iter().filter_map(|a| a.get("name")?.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    println!("af-command arguments: {args:?}");
    if pinned(&mpv) {
        assert_eq!(args, ["label", "command", "argument", "target"]);
    }
}

#[test]
fn observing_metadata_delivers_maps() {
    let Some(mpv) = start(METER) else { return };
    mpv.observe(1, "af-metadata/out").unwrap();
    mpv.command(&["loadfile", "av://lavfi:sine=f=1000:r=48000:d=4"]).unwrap();
    let (mut maps, mut with_keys) = (0, 0);
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && with_keys < 5 {
        if let Some(Event::PropertyChange { userdata: 1, value, .. }) = mpv.wait_event(0.1)
            && matches!(value, Node::Map(_))
        {
            maps += 1;
            with_keys += i32::from(value.get("lavfi.r128.M").is_some());
        }
    }
    println!("observed {maps} maps, {with_keys} with lavfi.r128.M");
    // Where this fails the player falls back to reading the property (see `Meters`).
    if pinned(&mpv) {
        assert!(with_keys >= 5);
    }
}

#[test]
fn metering_keeps_44100_hz() {
    let Some(mpv) = start(METER) else { return };
    play(&mpv, "av://lavfi:sine=f=1000:r=44100:d=4");
    near("44.1 kHz M", meta_at(&mpv, 1.0, "lavfi.r128.M"), -21.07, 0.5);
    let rate = out_param(&mpv, "samplerate");
    println!("44.1 kHz source, @out from the start: output {rate} Hz");
    assert_eq!(rate, 44100);

    // Added while playing: the AO is already open.
    let Some(mpv) = start("") else { return };
    play(&mpv, "av://lavfi:sine=f=1000:r=44100:d=4");
    let format = mpv.get("audio-out-params").unwrap().get("format").cloned();
    mpv.set_property("af", METER).unwrap();
    meta_at(&mpv, 1.0, "lavfi.r128.M");
    assert_eq!(out_param(&mpv, "samplerate"), 44100);
    assert_eq!(mpv.get("audio-out-params").unwrap().get("format").cloned(), format, "the AO isn't reopened");
}

#[test]
fn metering_keeps_5_1() {
    let Some(mpv) = start(METER) else { return };
    play(&mpv, "av://lavfi:sine=f=1000:r=48000:d=4,aformat=channel_layouts=5.1");
    meta_at(&mpv, 1.0, "lavfi.r128.M");
    let channels = out_param(&mpv, "channel-count");
    println!("5.1 source, @out: {channels} channels");
    assert_eq!(channels, 6);
    // `sine` upmixes to the centre: third channel, 1-based in astats and 0-based in r128.
    near("5.1 centre RMS", meta_at(&mpv, 1.0, "lavfi.astats.3.RMS_level"), -21.07, 0.5);
    near("5.1 centre true peak", meta_at(&mpv, 1.0, "lavfi.r128.true_peaks_ch2"), 0.125, 0.01);
}

#[test]
fn every_value_the_sheet_changes_is_a_command() {
    let af = format!(
        "@gain:lavfi=[volume=0dB],\
         @eq:lavfi=[equalizer@b0=f=1000:t=q:w=1:g=0],\
         @dyn:lavfi=[acompressor=threshold=-24dB:ratio=4:attack=20:release=400:knee=6dB:makeup=6dB:mix=1,alimiter=limit=0.891:level=false],\
         @spatial:lavfi=[crossfeed=strength=0.5:range=0.5],\
         @norm:lavfi=[dynaudnorm=f=200:g=5:p=0.9:m=10],\
         {METER}"
    );
    let Some(mpv) = start(&af) else { return };
    play(&mpv, "av://lavfi:sine=f=1000:r=48000:d=3,aformat=channel_layouts=stereo");
    meta_at(&mpv, 0.2, "lavfi.r128.M");
    if !pinned(&mpv) {
        return;
    }
    for [label, cmd, arg, target] in [
        ["gain", "volume", "-3dB", "volume"],
        ["eq", "g", "3", "equalizer@b0"],
        ["eq", "w", "1.5", "equalizer@b0"],
        ["eq", "f", "2000", "equalizer@b0"],
        ["dyn", "threshold", "-20dB", "acompressor"],
        ["dyn", "ratio", "3", "acompressor"],
        ["dyn", "makeup", "3dB", "acompressor"],
        ["dyn", "mix", "0", "acompressor"],
        ["dyn", "limit", "0.8", "alimiter"],
        ["spatial", "strength", "0.1", "crossfeed"],
        ["spatial", "range", "0.1", "crossfeed"],
    ] {
        let r = mpv.command(&["af-command", label, cmd, arg, target]);
        assert!(r.is_ok(), "af-command {label} {cmd} {arg} {target}: {r:?}");
    }
}

#[test]
fn a_filter_probe_tells_filters_apart() {
    let Some(mpv) = start("") else { return };
    assert!(
        matches!(mpv.set_property("af", "@probe:nosuchfilter_xyz"), Err(crate::Error::Mpv { .. })),
        "an unknown filter must be refused, or the probe says nothing"
    );
    for f in ["equalizer", "acompressor", "alimiter", "crossfeed", "dynaudnorm", "astats", "ebur128", "aphasemeter"] {
        let r = mpv.set_property("af", &format!("@probe:{f}"));
        assert!(r.is_ok(), "{f}: {r:?}");
    }
    mpv.set_property("af", "").unwrap();
}

/// A bench, not a check (CI's speed would decide the numbers): the extra start-up time of
/// `dynaudnorm` on a source limited to real time. Results are in this file's header.
#[test]
#[ignore]
fn dynaudnorm_lookahead() {
    for (name, af) in [
        ("none", METER.to_string()),
        ("f=200:g=5", format!("@norm:lavfi=[dynaudnorm=f=200:g=5:p=0.9:m=10],{METER}")),
        ("f=200:g=7", format!("@norm:lavfi=[dynaudnorm=f=200:g=7:p=0.9:m=10],{METER}")),
        ("f=250:g=15", format!("@norm:lavfi=[dynaudnorm=f=250:g=15:p=0.9:m=10],{METER}")),
    ] {
        let Some(mpv) = start(&af) else { return };
        let t = Instant::now();
        play(&mpv, "av://lavfi:sine=f=1000:r=48000:d=12,arealtime");
        let restart = t.elapsed();
        let t = Instant::now();
        while meta(&mpv, "lavfi.astats.1.RMS_level").is_none() {
            std::thread::sleep(Duration::from_millis(1));
        }
        println!("{name}: loadfile→restart {restart:?}, restart→first sample {:?}", t.elapsed());
    }
}

/// A queue through mpv's own playlist, which is what makes tracks gapless: the second entry goes in
/// when the first has loaded, and mpv moves on by itself. With `keep-open=yes` the last entry
/// doesn't end with `EndFile` but with `eof-reached` (the player's queue used to wait for `EndFile`,
/// so it never advanced at all).
#[test]
fn a_two_entry_queue_plays_through_mpvs_playlist() {
    if Library::get().is_err() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let opts = [("ao", "null"), ("keep-open", "yes"), ("gapless-audio", "weak"), ("prefetch-playlist", "yes")];
    let mpv = Mpv::headless(&opts).unwrap();
    mpv.observe(1, "playlist-playing-pos").unwrap();
    mpv.observe(2, "eof-reached").unwrap();
    let first = mpv.loadfile_mode_args("av://lavfi:sine=f=500:r=48000:d=1", "replace", "");
    mpv.command(&first.iter().map(String::as_str).collect::<Vec<_>>()).unwrap();
    let (mut appended, mut count, mut positions, mut eofs, mut ends) = (false, 0, Vec::new(), 0, 0);
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline && eofs == 0 {
        let Some(ev) = mpv.wait_event(0.1) else { continue };
        match ev {
            Event::FileLoaded if !appended => {
                appended = true;
                let next = mpv.loadfile_mode_args("av://lavfi:sine=f=700:r=48000:d=1", "append", "");
                mpv.command(&next.iter().map(String::as_str).collect::<Vec<_>>()).unwrap();
                count = mpv.get("playlist-count").unwrap().as_i64().unwrap();
            }
            Event::PropertyChange { userdata: 1, value, .. } => positions.push(value.as_i64().unwrap_or(-1)),
            Event::PropertyChange { userdata: 2, value, .. } => eofs += i32::from(value.as_bool() == Some(true)),
            Event::EndFile(crate::EndReason::Eof) => ends += 1,
            _ => {}
        }
    }
    println!("count {count}, positions {positions:?}, EndFile(Eof) {ends}, eof-reached {eofs}");
    assert_eq!(count, 2);
    assert_eq!(positions.last(), Some(&1), "{positions:?}");
    assert_eq!(ends, 1, "the first entry ends by EndFile");
    assert_eq!(eofs, 1, "the last by eof-reached");
}
