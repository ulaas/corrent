//! Transcoding with real libmpv (skips when it isn't installed).

use std::path::PathBuf;
use std::time::Duration;

use crate::decide::{AudioTarget, VideoTarget};
use crate::hls::{HlsConfig, HlsManager, SessionSpec};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("corrent-tc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn source(dir: &std::path::Path, secs: u32) -> String {
    let out = dir.join("src.mkv");
    let job = corrent_mpv::encode::EncodeJob {
        input: format!("av://lavfi:testsrc=duration={secs}:size=320x240:rate=25"),
        audio_file: Some(format!("av://lavfi:sine=duration={secs}")),
        output: out.display().to_string(),
        options: vec![
            ("of".into(), "matroska".into()),
            ("ovc".into(), "libx264".into()),
            ("ovcopts".into(), "preset=ultrafast".into()),
            ("oac".into(), "flac".into()),
        ],
    };
    corrent_mpv::encode::run(&job, Duration::from_secs(60), &mut |_| true).unwrap();
    out.display().to_string()
}

fn start_time(path: &std::path::Path) -> f64 {
    let mpv = corrent_mpv::Mpv::headless(&[("vo", "null"), ("ao", "null"), ("pause", "yes")]).unwrap();
    mpv.command(&["loadfile", &path.display().to_string()]).unwrap();
    while !matches!(mpv.wait_event(5.0), Some(corrent_mpv::Event::FileLoaded) | None) {}
    mpv.get_f64("demuxer-start-time").unwrap().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hls_session_serves_and_seeks() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let dir = scratch("hls");
    let input = source(&dir, 30);
    let cfg = HlsConfig { root: dir.join("sessions"), worker: None, ..Default::default() };
    let m = HlsManager::new(cfg);
    let id = m
        .create(SessionSpec {
            input,
            duration_ms: 30_000,
            fps: Some(25.0),
            video: Some(VideoTarget { height: None, bitrate: 1_000_000 }),
            audio: Some(AudioTarget { track: None, channels: 2, bitrate: 128_000 }),
            burn_subtitle: None,
            burn_subtitle_file: None,
            user: "u".into(),
            title: "t".into(),
            item_id: None,
        })
        .unwrap();
    let pl = m.playlist(&id).unwrap();
    assert_eq!(pl.matches(".ts").count(), 8, "{pl}");
    assert!(pl.ends_with("#EXT-X-ENDLIST\n"));
    let t0 = std::time::Instant::now();
    let s0 = m.segment(&id, 0).await.unwrap();
    assert!(s0.metadata().unwrap().len() > 1000);
    eprintln!("first segment after {:?}", t0.elapsed());
    // A seek far ahead restarts the encoder there, on the original timeline.
    let t1 = std::time::Instant::now();
    let s6 = m.segment(&id, 6).await.unwrap();
    eprintln!("segment 6 after {:?}", t1.elapsed());
    let st = start_time(&s6);
    assert!((23.0..25.5).contains(&st), "segment 6 starts near 24 s, got {st}");
    assert!(m.segment(&id, 99).await.is_err());
    assert_eq!(m.sessions().len(), 1);
    m.stop(&id).await;
    assert!(m.playlist(&id).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn encoder_detection_and_trickplay() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let dir = scratch("trick");
    let enc = crate::hw::detect(None, None, &dir).await.expect("an H.264 encoder");
    assert!(crate::hw::CANDIDATES.contains(&enc.as_str()));
    let input = source(&dir, 25);
    let out = dir.join("trick");
    std::fs::create_dir_all(&out).unwrap();
    let n = crate::trickplay::generate(&input, &out, 5, 160, None).await.unwrap();
    assert!((4..=6).contains(&n), "{n} frames");
    // Through the worker too (the server's way).
    if let Some(w) = corrent_mpv::worker_path() {
        let out2 = dir.join("trick-worker");
        std::fs::create_dir_all(&out2).unwrap();
        let n2 = crate::trickplay::generate(&input, &out2, 5, 160, Some(w)).await.unwrap();
        assert_eq!(n2, n);
    }
    let vtt = std::fs::read_to_string(out.join("index.vtt")).unwrap();
    assert!(vtt.starts_with("WEBVTT") && vtt.contains("sheet-0.jpg#xywh=160,0,160,120"), "{vtt}");
    let (w, _) = image::image_dimensions(out.join("sheet-0.jpg")).unwrap();
    assert_eq!(w, 160 * n as u32);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_comes_out_as_a_png() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let dir = scratch("frame");
    let input = source(&dir, 12);
    let at = crate::frame::position_secs(Some(12_000), false);
    assert_eq!(at, 5.0);
    // In this process, and through the worker as the server does.
    let png = crate::frame::extract(&input, at, 160, None, &dir.join("tmp")).await.unwrap();
    assert_eq!(png.get(..4), Some(&[0x89, b'P', b'N', b'G'][..]), "a PNG");
    if let Some(w) = corrent_mpv::worker_path() {
        let again = crate::frame::extract(&input, at, 160, Some(w), &dir.join("tmpw")).await.unwrap();
        assert_eq!(image::load_from_memory(&again).unwrap().width(), 160);
    }
    let img = image::load_from_memory(&png).unwrap();
    assert_eq!((img.width(), img.height()), (160, 120));
    assert!(!dir.join("tmp").exists(), "the job's folder is cleaned up");
    // A file that isn't there: an error rather than a hang.
    let gone = dir.join("gone.mkv").display().to_string();
    assert!(crate::frame::extract(&gone, 5.0, 160, None, &dir.join("tmp2")).await.is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn thumbnail_positions() {
    use crate::frame::position_secs;
    assert_eq!(position_secs(None, false), 30.0);
    assert_eq!(position_secs(Some(100 * 60_000), false), 900.0);
    assert_eq!(position_secs(Some(40 * 60_000), true), 480.0);
    assert_eq!(position_secs(Some(3_000), false), 2.0, "a short clip: a second before the end");
}

/// A real x264 (with B-frames) + AAC Matroska file remuxed to HLS/fMP4 plays and seeks in mpv.
#[test]
fn remuxed_hls_plays_and_seeks() {
    if !corrent_mpv::Library::available() {
        eprintln!("libmpv not found; skipping");
        return;
    }
    let dir = scratch("remux");
    let src = dir.join("src.mkv");
    let job = corrent_mpv::encode::EncodeJob {
        input: "av://lavfi:testsrc=duration=20:size=320x240:rate=25".into(),
        audio_file: Some("av://lavfi:sine=duration=20".into()),
        output: src.display().to_string(),
        options: vec![
            ("of".into(), "matroska".into()),
            ("ovc".into(), "libx264".into()),
            ("ovcopts".into(), "preset=fast,bf=3,g=50".into()),
            ("oac".into(), "aac".into()),
        ],
    };
    corrent_mpv::encode::run(&job, Duration::from_secs(60), &mut |_| true).unwrap();
    let r = crate::remux::prepare(&src, Some(1)).unwrap();
    assert!(r.segments.len() >= 4, "{} segments", r.segments.len());
    assert!(r.codecs().starts_with("avc1.") && r.codecs().ends_with(",mp4a.40.2"), "{}", r.codecs());
    let out = dir.join("hls");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("index.m3u8"), r.playlist("")).unwrap();
    std::fs::write(out.join("init.mp4"), r.init()).unwrap();
    for i in 0..r.segments.len() {
        std::fs::write(out.join(format!("{i}.m4s")), r.segment(i).unwrap()).unwrap();
    }
    let mpv = corrent_mpv::Mpv::headless(&[("vo", "null"), ("ao", "null"), ("untimed", "yes")]).unwrap();
    mpv.request_log_messages("error").unwrap();
    mpv.command(&["loadfile", &out.join("index.m3u8").display().to_string()]).unwrap();
    let mut errors = Vec::new();
    let mut loaded = false;
    let mut seeked = false;
    let end = loop {
        match mpv.wait_event(10.0) {
            Some(corrent_mpv::Event::Log { prefix, text, .. }) => errors.push(format!("{prefix}: {text}")),
            Some(corrent_mpv::Event::FileLoaded) => {
                loaded = true;
                let d = mpv.get_f64("duration").unwrap().unwrap_or(0.0);
                assert!((d - 20.0).abs() < 1.0, "duration {d}");
                assert_eq!(mpv.get_string("current-tracks/video/codec").unwrap_or_default(), "h264");
                assert_eq!(mpv.get_string("current-tracks/audio/codec").unwrap_or_default(), "aac");
            }
            Some(corrent_mpv::Event::PlaybackRestart) if loaded && !seeked => {
                seeked = true;
                mpv.command(&["seek", "13", "absolute"]).unwrap();
            }
            Some(corrent_mpv::Event::PlaybackRestart) if seeked => {
                let pos = mpv.get_f64("time-pos").unwrap().unwrap_or(-1.0);
                assert!((12.0..14.5).contains(&pos), "after seeking: {pos}");
            }
            Some(corrent_mpv::Event::EndFile(reason)) => break reason,
            None => panic!("mpv stopped answering"),
            _ => {}
        }
    };
    assert!(matches!(end, corrent_mpv::EndReason::Eof), "ended with {end:?}");
    assert!(errors.is_empty(), "mpv errors: {errors:?}");
    let _ = std::fs::remove_dir_all(dir);
}
