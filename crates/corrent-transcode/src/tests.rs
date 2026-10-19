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
    let vtt = std::fs::read_to_string(out.join("index.vtt")).unwrap();
    assert!(vtt.starts_with("WEBVTT") && vtt.contains("sheet-0.jpg#xywh=160,0,160,120"), "{vtt}");
    let (w, _) = image::image_dimensions(out.join("sheet-0.jpg")).unwrap();
    assert_eq!(w, 160 * n as u32);
    let _ = std::fs::remove_dir_all(dir);
}
