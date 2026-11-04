# Spike MS0b/MS0c — libmpv video inside eframe/glow

Date: 2026-10-19. Machine: Fedora (Wayland session, XWayland available), mpv 0.41.0 /
libmpv client API 2.5, FFmpeg 9.0.1, eframe/egui 0.36.2 (glow). Code:
`crates/corrent-mpv/examples/egui_player.rs`.

## What was built

- **GL path:** `RenderContext::new_gl(mpv, cc.get_proc_address)` created in the app constructor
  (eframe's context is current there). An FBO with an RGBA8 texture attachment of the widget's
  pixel size. Each frame an `egui_glow::CallbackFn` runs `mpv_render_context_update()` and, when a
  frame is ready (or after a resize), renders into that FBO with `flip_y = 0`. The same texture is
  registered once with `Frame::register_native_glow_texture` and drawn with `painter.image`, so
  egui overlays (the control bar in the screenshot) compose on top.
- **SW path:** `RenderContext::new_sw` → `rgb0` into a reusable buffer (alpha forced to 255) →
  `TextureHandle::set` each new frame.
- mpv update callback → `egui::Context::request_repaint()`; an events thread forwards
  `time-pos`, `duration`, `media-title`, `hwdec-current` property changes.

## Results

| Path | Source | Renders in ~3 s | UI frame p99 | Orientation | Overlay |
|---|---|---|---|---|---|
| GL | testsrc2 1280×720@30 | 78 (≈ 28 fps, 30 fps source) | 16.0 ms | correct | composes |
| SW | same | 85 | 189 ms (includes the first frames; CPU copy of ~3.5 MB/frame) | correct | composes |

Findings:
- egui_glow restores its GL state after a paint callback (`prepare_painting`) **but not the
  framebuffer binding** — the callback must read `FRAMEBUFFER_BINDING` before rendering and
  rebind it afterwards. Scissor and blend are disabled before `mpv_render_context_render`.
- `flip_y = 0` is right for an FBO texture sampled by egui (uv (0,0) = top-left).
- With `ADVANCED_CONTROL=1`, `update()` has to run after every update callback; doing it in the
  paint callback is sufficient because each callback requests a repaint.
- On exit mpv reported `END_FILE` with error -15 (VO init failed) after the render context was
  freed during teardown — harmless; the player must stop playback (`stop`) before dropping the
  render context.
- hwdec was `no` for the lavfi source (expected: nothing to decode); real-file hwdec on VA-API is
  checked in MS8.

## Decision

**Go** for the GL path as the default native video path; SW stays the automatic fallback
(`CORRENT_SW_VIDEO=1` or GL context creation failure). Windows/macOS runs remain [O].

## Frame reports and dither (quality round, 2026-11-05)

`GlVideo::paint` now calls `report_swap()` after rendering, `render_gl` tells mpv the target is
`GL_RGBA8`, and the player sets `dither-depth=8`. Measured with `examples/egui_player.rs --simple`
(no advanced control, as the player runs; 120 Hz display, 1280×720@30 `testsrc2`, 8 s, two runs each):

| | frames | p50 | p99 | max (start-up) |
|---|---|---|---|---|
| without `--swap` | 934, 936 | 8.3 ms | 8.8, 8.8 ms | 128, 184 ms |
| with `--swap` | 934, 936, 936 | 8.3 ms | 9.0, 8.8, 8.9 ms | 202, 192, 191 ms |

No change, as expected: it only feeds statistics. But `display-fps`, `estimated-display-fps`,
`vsync-jitter` and `vsync-ratio` stay "property unavailable" on this libmpv with the report, with or
without advanced control (`vo=libmpv` gets no display timing from us). So the A/V page shows "—" for
them instead of made-up numbers; making them real needs `display-fps-override` from the monitor's
rate, or the advanced control trial, both deferred.

## Closing while a server stalls (A13: verified, doesn't reproduce)

`Drop for Player` sends `stop` and frees the render context on the UI thread; `render.h` says that
waits for the core to uninitialise the VO, so a stalled demuxer could hold the UI thread up.
`closing_while_the_server_stalls` in `render.rs` (ignored, `--ignored --nocapture`) does the same
against a local server that stops sending with the connection open:

| | frames rendered | render context freed in | `mpv_terminate_destroy` |
|---|---|---|---|
| stalled at opening | 0 | 46 µs | 1 ms |
| stalled mid playback (`paused-for-cache`) | 100 | 173 µs | 102 ms |

Far below the 10 s of the UI watchdog, so no `logs/hang-*.txt`, and no fix was made (the deferred
`vid no` and a closing state aren't needed). The stall is in the demuxer's thread, which the core
doesn't wait for. A slow disk or a blocked `sub-add` were not part of this.

