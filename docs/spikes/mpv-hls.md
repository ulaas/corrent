# Spike MS0d — libmpv encode mode → HLS

Date: 2026-10-19. mpv 0.41.0, FFmpeg 9.0.1 (Fedora build). Tests:
`crates/corrent-mpv/src/encode.rs` (`encode_then_probe`, `hls_restart_keeps_the_original_timeline`).

## Capabilities of this libmpv build

- Muxers (`--of=help`): `hls`, `segment`, `dash`, `mpegts`, `mp4`, `matroska`, `webm`.
- Video encoders (`--ovc=help`): `libx264`, `libx265`, `libopenh264`, `libvpx-vp9`, `libsvtav1`,
  `libaom-av1`, `h264_vaapi`/`hevc_vaapi`/`av1_vaapi`, `*_nvenc`, `*_qsv`, `*_amf`, `*_vulkan`.
- Audio encoders: `aac`, `libfdk_aac`, `opus`/`libopus`, `flac`, `libmp3lame`.
- `av://lavfi:` inputs work in encode mode (test media without files).

## Results

- `--of=hls --ofopts=hls_time=2,hls_list_size=0,hls_segment_filename=…/seg%03d.ts
  --ovc=libx264 --ovcopts=preset=veryfast,g=60 --oac=aac` on a 12 s 720p source: 4 segments of
  2.0 s and a VOD playlist in **0.7 s wall** (≈ 17× realtime on this CPU).
- Keyframe interval via `--ovcopts=g=<n>` works (segments split exactly at 2.0 s).
- mpv rebases output timestamps to 0 at `--start`. Adding `start_number=<n>,output_ts_offset=<t>`
  to `--ofopts` makes a restarted encode write `seg<n>.ts…` whose PTS starts at ≈ t, i.e. a
  seek-restart stays on the original timeline (test asserts segment 2 starts within 3.5–4.6 s).
- Through libmpv the same options work when set before `mpv_initialize`; a per-file extra audio
  input goes through `loadfile <url> replace -1 audio-file=%len%<url>` (the `audio-files` list
  option splits on `:`).
- Not yet measured: VA-API/NVENC in encode mode (need `vf=format=nv12,hwupload` for VA-API),
  hls.js playback of the restarted playlist, fMP4 segments. These are MS9 acceptance items.

## Decision

**Go**: the server-owned virtual playlist + restart-at-segment design works with mpv's own `hls`
muxer; no custom TS segmenter is needed.
