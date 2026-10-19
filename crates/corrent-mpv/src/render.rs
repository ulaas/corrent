//! mpv's render API: video frames drawn into the host's OpenGL framebuffer or into a CPU buffer.
//!
//! The mpv instance must be created with `vo=libmpv`. A render context must be used from one
//! thread (the one with the GL context current, for OpenGL) and dropped before the [`Mpv`].

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::Arc;

use crate::{Error, Mpv, Result, WakeBox, ffi};

type ProcBox = Box<dyn Fn(&CStr) -> *const c_void + Send>;

pub struct RenderContext {
    mpv: Arc<Mpv>,
    ctx: *mut ffi::mpv_render_context,
    _proc: Option<Box<ProcBox>>,
    update: Option<Box<WakeBox>>,
    sw: bool,
}

// SAFETY: a render context may move between threads as long as it is used from one at a time
// (and, for OpenGL, with the same GL context current), which `&mut self` methods guarantee.
unsafe impl Send for RenderContext {}

unsafe extern "C" fn proc_trampoline(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: `ctx` is the boxed resolver owned by the RenderContext; `name` is a C string.
    unsafe {
        let f = &*(ctx as *const ProcBox);
        f(CStr::from_ptr(name)) as *mut c_void
    }
}

unsafe extern "C" fn update_trampoline(d: *mut c_void) {
    // SAFETY: `d` is the boxed callback owned by the RenderContext.
    let f = unsafe { &*(d as *const WakeBox) };
    f();
}

impl RenderContext {
    /// An OpenGL render context. Call with the host's GL context current; `get_proc_address`
    /// resolves GL function names (eframe: `CreationContext::get_proc_address`).
    pub fn new_gl(mpv: Arc<Mpv>, get_proc_address: impl Fn(&CStr) -> *const c_void + Send + 'static) -> Result<Self> {
        let proc: Box<ProcBox> = Box::new(Box::new(get_proc_address));
        let mut init = ffi::mpv_opengl_init_params {
            get_proc_address: Some(proc_trampoline),
            get_proc_address_ctx: &*proc as *const ProcBox as *mut c_void,
        };
        let mut advanced: c_int = 1;
        let mut params = [
            param(ffi::MPV_RENDER_PARAM_API_TYPE, c"opengl".as_ptr() as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, &mut init as *mut _ as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_ADVANCED_CONTROL, &mut advanced as *mut c_int as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_INVALID, std::ptr::null_mut()),
        ];
        Self::create(mpv, &mut params, Some(proc), false)
    }

    /// A software render context: frames are written into a caller-provided RGBA buffer.
    pub fn new_sw(mpv: Arc<Mpv>) -> Result<Self> {
        let mut params = [
            param(ffi::MPV_RENDER_PARAM_API_TYPE, c"sw".as_ptr() as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_INVALID, std::ptr::null_mut()),
        ];
        Self::create(mpv, &mut params, None, true)
    }

    fn create(
        mpv: Arc<Mpv>,
        params: &mut [ffi::mpv_render_param],
        proc: Option<Box<ProcBox>>,
        sw: bool,
    ) -> Result<Self> {
        let mut ctx = std::ptr::null_mut();
        // SAFETY: params is INVALID-terminated and its pointers live for the call; mpv copies them.
        let rc = unsafe { (mpv.api().render_context_create)(&mut ctx, mpv.raw(), params.as_mut_ptr()) };
        if rc < 0 {
            return Err(Error::Mpv {
                what: "mpv_render_context_create".into(),
                code: rc,
                message: mpv.library().error_text(rc),
            });
        }
        Ok(RenderContext { mpv, ctx, _proc: proc, update: None, sw })
    }

    pub fn is_software(&self) -> bool {
        self.sw
    }

    /// `f` runs on an mpv thread when a new frame should be rendered; it must only signal
    /// (e.g. `egui::Context::request_repaint`), never call back into mpv.
    pub fn set_update_callback(&mut self, f: impl Fn() + Send + Sync + 'static) {
        let boxed: Box<WakeBox> = Box::new(Box::new(f));
        let ptr = &*boxed as *const WakeBox as *mut c_void;
        // SAFETY: valid context; the closure stays alive in `self.update`.
        unsafe { (self.mpv.api().render_context_set_update_callback)(self.ctx, Some(update_trampoline), ptr) };
        self.update = Some(boxed);
    }

    /// Must be called after every update callback; returns true when a new frame is ready.
    pub fn update(&mut self) -> bool {
        // SAFETY: valid context, used from its thread.
        let flags = unsafe { (self.mpv.api().render_context_update)(self.ctx) };
        flags & ffi::MPV_RENDER_UPDATE_FRAME != 0
    }

    /// Renders the current frame into framebuffer `fbo` (0 = default) of `w`×`h` pixels.
    pub fn render_gl(&mut self, fbo: i32, w: i32, h: i32, flip_y: bool) -> Result<()> {
        let mut target = ffi::mpv_opengl_fbo { fbo, w, h, internal_format: 0 };
        let mut flip: c_int = flip_y.into();
        let mut block: c_int = 0;
        let mut params = [
            param(ffi::MPV_RENDER_PARAM_OPENGL_FBO, &mut target as *mut _ as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_FLIP_Y, &mut flip as *mut c_int as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME, &mut block as *mut c_int as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_INVALID, std::ptr::null_mut()),
        ];
        self.render(&mut params)
    }

    /// Renders the current frame as RGBA (alpha forced to 255) into `buf` (`w*h*4` bytes).
    pub fn render_sw(&mut self, w: i32, h: i32, buf: &mut [u8]) -> Result<()> {
        let need = (w.max(0) as usize) * (h.max(0) as usize) * 4;
        if buf.len() < need || w <= 0 || h <= 0 {
            return Err(Error::Mpv { what: "render_sw".into(), code: -4, message: "buffer too small".into() });
        }
        let mut size = [w as c_int, h as c_int];
        let mut stride: usize = w as usize * 4;
        let mut params = [
            param(ffi::MPV_RENDER_PARAM_SW_SIZE, size.as_mut_ptr() as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_SW_FORMAT, c"rgb0".as_ptr() as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_SW_STRIDE, &mut stride as *mut usize as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_SW_POINTER, buf.as_mut_ptr() as *mut c_void),
            param(ffi::MPV_RENDER_PARAM_INVALID, std::ptr::null_mut()),
        ];
        self.render(&mut params)?;
        for px in buf[..need].as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Ok(())
    }

    fn render(&mut self, params: &mut [ffi::mpv_render_param]) -> Result<()> {
        // SAFETY: valid context; params INVALID-terminated, pointers live for the call.
        let rc = unsafe { (self.mpv.api().render_context_render)(self.ctx, params.as_mut_ptr()) };
        if rc < 0 {
            return Err(Error::Mpv { what: "render".into(), code: rc, message: self.mpv.library().error_text(rc) });
        }
        Ok(())
    }

    /// Tells mpv a frame was presented (only improves its timing statistics).
    pub fn report_swap(&mut self) {
        // SAFETY: valid context.
        unsafe { (self.mpv.api().render_context_report_swap)(self.ctx) }
    }

    pub fn mpv(&self) -> &Arc<Mpv> {
        &self.mpv
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        // SAFETY: valid context; freeing it unregisters the update callback before it drops.
        unsafe { (self.mpv.api().render_context_free)(self.ctx) }
    }
}

fn param(type_: c_int, data: *mut c_void) -> ffi::mpv_render_param {
    ffi::mpv_render_param { type_, data }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Event, Library};

    #[test]
    fn software_render_produces_pixels() {
        let Ok(lib) = Library::get() else {
            eprintln!("libmpv not found; skipping");
            return;
        };
        let mut opts: Vec<(&str, &str)> = crate::HEADLESS.to_vec();
        opts.extend([("vo", "libmpv"), ("ao", "null"), ("pause", "yes")]);
        let mpv = Arc::new(Mpv::new(lib, &opts).unwrap());
        let mut rc = RenderContext::new_sw(mpv.clone()).unwrap();
        mpv.command(&["loadfile", "av://lavfi:color=c=red:size=64x48:duration=5"]).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if let Some(Event::PlaybackRestart) = mpv.wait_event(0.1) {
                break;
            }
        }
        let mut buf = vec![0u8; 64 * 48 * 4];
        let mut got = false;
        for _ in 0..100 {
            if rc.update() {
                rc.render_sw(64, 48, &mut buf).unwrap();
                got = true;
                break;
            }
            mpv.wait_event(0.05);
        }
        assert!(got, "a frame was rendered");
        let mid = &buf[(24 * 64 + 32) * 4..][..4];
        assert!(mid[0] > 200 && mid[1] < 60 && mid[2] < 60 && mid[3] == 255, "red pixel, got {mid:?}");
    }
}
