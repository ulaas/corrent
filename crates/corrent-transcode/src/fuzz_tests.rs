//! Robustness: subtitle conversion takes any file (wrong encodings, broken timestamps, huge
//! numbers) and always produces WebVTT without panicking.

use corrent_fuzz::{iterations, run};

const SRT: &[&str] = &[
    "1\n00:00:01,000 --> 00:00:02,500\nHello\n\n2\n00:00:03,000 --> 00:00:04,000\n<i>World</i>\n",
    "1\r\n00:00:01,000 --> 00:00:02,500\r\nLine\r\n\r\n",
    "99999999999999999999\n99:99:99,999 --> 00:00:00,000\nx\n",
];
const ASS: &[&str] = &[
    "[Script Info]\nTitle: x\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\i1}Hi{\\i0}\\Nthere\n",
    "[Events]\nFormat: Start, End, Text\nDialogue: 0:00:01.00,0:00:02.00,a,b,c\n",
];

#[test]
fn subtitle_conversion_never_panics() {
    run("srt", iterations(10_000), |r| {
        let s = r.case(SRT, 200);
        let out = crate::vtt::srt_to_vtt(&s);
        assert!(out.starts_with("WEBVTT"), "{out:?}");
    });
    run("ass", iterations(10_000), |r| {
        let s = r.case(ASS, 200);
        let out = crate::vtt::ass_to_vtt(&s);
        assert!(out.starts_with("WEBVTT"), "{out:?}");
    });
    run("to_vtt", iterations(5_000), |r| {
        let ext = *r.pick(&["srt", "ASS", "ssa", "vtt", "sup", ""]);
        let bytes = if r.chance(50) { r.bytes(300) } else { r.case(SRT, 100).into_bytes() };
        let _ = crate::vtt::to_vtt(ext, &bytes);
    });
}
