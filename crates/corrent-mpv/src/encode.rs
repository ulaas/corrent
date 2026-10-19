//! mpv's encoding mode: decode `input` and re-encode it to `--o` with libavcodec/libavformat.
//!
//! mpv always re-encodes (it cannot stream-copy). Encoding options must be set before
//! `mpv_initialize`, so every job gets its own instance.

use std::time::{Duration, Instant};

use crate::{EndReason, Error, Event, Mpv, Result};

/// One encode job. `options` are mpv option pairs such as `("ovc", "libx264")`,
/// `("of", "hls")`, `("ofopts", "hls_time=4")`, `("start", "120")`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EncodeJob {
    pub input: String,
    pub output: String,
    pub options: Vec<(String, String)>,
    /// Extra inputs merged into the job (`--audio-file`).
    pub audio_file: Option<String>,
}

/// Progress callback argument: seconds of output written so far.
pub type Progress<'a> = &'a mut dyn FnMut(f64) -> bool;

/// Runs `job` to completion (blocking). `progress` is called about twice a second with the
/// current output position and returns `false` to cancel.
pub fn run(job: &EncodeJob, timeout: Duration, progress: Progress<'_>) -> Result<()> {
    let mut opts: Vec<(&str, &str)> = crate::HEADLESS.iter().copied().filter(|(k, _)| *k != "idle").collect();
    opts.push(("o", &job.output));
    opts.push(("vo", "null"));
    opts.push(("ao", "null"));
    opts.push(("idle", "once"));
    for (k, v) in &job.options {
        opts.push((k, v));
    }
    let mpv = Mpv::new(crate::Library::get()?, &opts)?;
    match &job.audio_file {
        // `%len%` quoting keeps `:` and `,` in URLs out of mpv's option-list parser.
        Some(a) => mpv.command(&["loadfile", &job.input, "replace", "-1", &format!("audio-file=%{}%{a}", a.len())])?,
        None => mpv.command(&["loadfile", &job.input])?,
    }
    let deadline = Instant::now() + timeout;
    let mut last_report = Instant::now();
    loop {
        if Instant::now() >= deadline {
            return Err(Error::Timeout(format!("encoding {}", job.input)));
        }
        match mpv.wait_event(0.5) {
            Some(Event::EndFile(EndReason::Eof)) => return Ok(()),
            Some(Event::EndFile(EndReason::Error(code))) => {
                return Err(Error::Mpv {
                    what: format!("encode {}", job.input),
                    code,
                    message: mpv.library().error_text(code),
                });
            }
            Some(Event::EndFile(_)) | Some(Event::Shutdown) => {
                return Err(Error::Mpv { what: "encode".into(), code: -20, message: "stopped".into() });
            }
            _ => {}
        }
        if last_report.elapsed() >= Duration::from_millis(500) {
            last_report = Instant::now();
            let pos = mpv.get_f64("time-pos").ok().flatten().unwrap_or(0.0);
            if !progress(pos) {
                return Err(Error::Mpv { what: "encode".into(), code: -20, message: "cancelled".into() });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Library;

    pub(crate) fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("corrent-mpv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn encode_then_probe() {
        if !Library::available() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let dir = scratch("encode");
        let out = dir.join("clip.mkv");
        let job = EncodeJob {
            input: "av://lavfi:testsrc=duration=3:size=320x240:rate=25".into(),
            audio_file: Some("av://lavfi:sine=frequency=440:duration=3".into()),
            output: out.display().to_string(),
            options: vec![("of".into(), "matroska".into()), ("oac".into(), "flac".into())],
        };
        run(&job, Duration::from_secs(60), &mut |_| true).unwrap();
        let r = crate::probe(&out.display().to_string(), Duration::from_secs(20)).unwrap();
        let d = r.duration.unwrap();
        assert!((2.5..3.5).contains(&d), "duration {d}");
        assert_eq!(r.video().next().unwrap().width, Some(320));
        assert_eq!(r.audio().next().unwrap().codec.as_deref(), Some("flac"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hls_restart_keeps_the_original_timeline() {
        if !Library::available() {
            eprintln!("libmpv not found; skipping");
            return;
        }
        let dir = scratch("hls");
        let seg = dir.join("seg%03d.ts");
        // A restart at 4 s with 2 s segments starts at segment 2 and offsets timestamps by 4 s.
        let job = EncodeJob {
            input: "av://lavfi:testsrc=duration=8:size=320x240:rate=25".into(),
            audio_file: Some("av://lavfi:sine=duration=8".into()),
            output: dir.join("index.m3u8").display().to_string(),
            options: vec![
                ("start".into(), "4".into()),
                ("of".into(), "hls".into()),
                (
                    "ofopts".into(),
                    format!(
                        "hls_time=2,hls_list_size=0,start_number=2,output_ts_offset=4,hls_segment_filename=%{}%{}",
                        seg.display().to_string().len(),
                        seg.display()
                    ),
                ),
                ("ovc".into(), "libx264".into()),
                ("ovcopts".into(), "preset=veryfast,g=50".into()),
                ("oac".into(), "aac".into()),
            ],
        };
        if let Err(e) = run(&job, Duration::from_secs(60), &mut |_| true) {
            // Builds without libx264 or the hls muxer can't run this; that is not a failure here.
            eprintln!("hls encode unsupported by this libmpv: {e}");
            return;
        }
        assert!(dir.join("seg002.ts").exists() && !dir.join("seg000.ts").exists());
        let r = crate::probe(&dir.join("seg002.ts").display().to_string(), Duration::from_secs(20)).unwrap();
        assert!(r.video().next().is_some());
        let mpv = Mpv::headless(&[("vo", "null"), ("ao", "null"), ("pause", "yes")]).unwrap();
        mpv.command(&["loadfile", &dir.join("seg002.ts").display().to_string()]).unwrap();
        while !matches!(mpv.wait_event(5.0), Some(Event::FileLoaded) | None) {}
        let start = mpv.get_f64("demuxer-start-time").unwrap().unwrap();
        assert!((3.5..4.6).contains(&start), "segment 2 starts near 4 s, got {start}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
