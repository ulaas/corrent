# Spike — user shaders through mpv's render API

Date: 2026-11-13. Machine: Fedora, mpv 0.41.0 / libmpv 2.5, OpenGL 3.3 core on an offscreen EGL
context (NVIDIA; `crates/corrent-mpv/src/test_gl.rs`). Code: `crates/corrent-mpv/src/video_tests.rs`
(`cargo test -p corrent-mpv video_tests -- --include-ignored --nocapture --test-threads=1`).

**Go:** video effects are mpv hook shaders (`glsl-shaders`), with values baked into the text.

## Results

| Question | Answer |
|---|---|
| Does `glsl-shaders`, set while a file plays, act under `vo=libmpv`? | Yes, also while paused: `mpv_render_context_update` reports a frame about 90 ms after the set (5 ms after `brightness`). |
| How is the list set? | As a node array of strings (`Mpv::set_string_list`); the string form's separator differs between mpv versions. The asynchronous set answers with `MPV_EVENT_SET_PROPERTY_REPLY`. |
| `memory://` as a shader? | **No**: 0.41 opens shaders from the local file system only (`[file] Cannot open file 'memory://…'`). Shaders are files, named by a hash of their text. |
| A shader that doesn't compile | The picture turns blue. `libmpv_render` logs at `error`: `fragment shader compile log (status=0):`, `shader link log (status=0):` and the driver's lines. An empty list brings the picture back at once, and a good shader works after it. |
| A bad `//!` line, an unknown hook, a file that isn't a shader | No effect on the picture; `Unrecognized command '…'!` for the first, nothing for the others. |
| What a changed constant costs | Set 0.3 ms; the first render with a new text 2.5 ms median, 2.8 ms p95, 10 ms the very first (unchanged frame: 0.1 ms). A text seen before: 1.6 ms. |
| Do the variants pile up? | Yes, about 60 kB each (500 texts: +30 MB) until the renderer goes. So sliders move in steps and a drag is debounced. |
| `gpu-shader-cache` | On by default and written to `~/.cache/mpv`; the player turns it off (not an option of older mpv: set after init, error ignored). |
| Hooks | `MAIN` (source size), `MAINPRESUB`, `SCALED` and `OUTPUT` (target size) run. `LINEAR` doesn't with the default scalers. |
| On `OUTPUT`: `//!SAVE`, `//!WIDTH`, `//!HEIGHT`, a texture saved on `MAIN` | All work. |
| Uniforms | `HOOKED_pos/pt/size`, `HOOKED_tex`, `HOOKED_texOff`, `random`, `frame`, `input_size`, `target_size` compile and run. |
| `//!PARAM` with `glsl-shader-opts` | No: `Unrecognized command 'PARAM amount'!` (gpu-next only). |
| mpv's `brightness`, `contrast`, `saturation`, `gamma`, `hue`, `deband` | Work with OpenGL. |
| The software renderer | Neither the shaders nor those properties do anything. |

Not answered here: older libmpv (CI's Ubuntu) and the owner's Windows GPU; the tests that aren't
`#[ignore]` run there.
