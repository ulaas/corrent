//! An OpenGL context without a window, for tests that need mpv's GPU renderer (user shaders run
//! only there, never in the software renderer).
//!
//! Linux only: EGL's device platform, surfaceless or on a pbuffer, which Mesa also offers
//! without a display server. Elsewhere [`Offscreen::new`] is `None` and the tests skip.

use std::ffi::{CStr, c_void};
use std::sync::Arc;

use glow::HasContext as _;

use crate::render::RenderContext;

pub struct Offscreen {
    pub gl: Arc<glow::Context>,
    #[cfg(target_os = "linux")]
    egl: linux::Egl,
}

impl Offscreen {
    /// A current context on this thread, which must stay the one that uses it.
    pub fn new() -> Option<Offscreen> {
        Self::make()
    }

    #[cfg(target_os = "linux")]
    fn make() -> Option<Offscreen> {
        let egl = linux::Egl::new()?;
        // SAFETY: the context is current on this thread and the loader resolves its functions.
        let gl = unsafe { glow::Context::from_loader_function_cstr(|n| egl.proc_address(n)) };
        Some(Offscreen { gl: Arc::new(gl), egl })
    }

    #[cfg(not(target_os = "linux"))]
    fn make() -> Option<Offscreen> {
        None
    }

    /// Resolves a GL function, for [`RenderContext::new_gl`].
    pub fn resolver(&self) -> impl Fn(&CStr) -> *const c_void + Send + 'static {
        #[cfg(target_os = "linux")]
        {
            self.egl.resolver()
        }
        #[cfg(not(target_os = "linux"))]
        {
            |_: &CStr| std::ptr::null()
        }
    }

    /// Renders mpv's current frame into a `w`×`h` RGBA8 texture and reads it back, top row first.
    pub fn render_rgba(&self, rc: &mut RenderContext, w: i32, h: i32) -> crate::Result<Vec<u8>> {
        let gl = &self.gl;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        // SAFETY: plain GL calls on the current context; the objects are deleted before returning.
        unsafe {
            let tex = gl.create_texture().expect("texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w,
                h,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            let fbo = gl.create_framebuffer().expect("framebuffer");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            // With flip_y the first row mpv writes is the picture's top, which is where
            // `read_pixels` starts.
            let done = rc.render_gl(fbo.0.get() as i32, w, h, true);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut buf)));
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.delete_framebuffer(fbo);
            gl.delete_texture(tex);
            done?;
        }
        Ok(buf)
    }

    /// `GL_VERSION` and `GL_RENDERER`, for a test's output.
    pub fn describe(&self) -> String {
        // SAFETY: plain GL queries on the current context.
        unsafe {
            format!(
                "{} / {}",
                self.gl.get_parameter_string(glow::VERSION),
                self.gl.get_parameter_string(glow::RENDERER)
            )
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::{CStr, c_void};

    use glutin::api::egl::{context::PossiblyCurrentContext, device::Device, display::Display, surface::Surface};
    use glutin::config::{ConfigSurfaceTypes, ConfigTemplateBuilder};
    use glutin::context::{ContextApi, ContextAttributesBuilder, GlProfile, Version};
    use glutin::prelude::*;
    use glutin::surface::{PbufferSurface, SurfaceAttributesBuilder};

    pub struct Egl {
        display: Display,
        // Dropped before the display (field order).
        _surface: Option<Surface<PbufferSurface>>,
        _context: PossiblyCurrentContext,
    }

    impl Egl {
        pub fn new() -> Option<Egl> {
            // Software devices last: a real GPU when there is one.
            let mut devices: Vec<Device> = Device::query_devices().ok()?.collect();
            devices.sort_by_key(|d| d.name().is_some_and(|n| n.contains("software") || n.contains("llvmpipe")));
            devices.into_iter().find_map(|d| Self::on(&d))
        }

        fn on(device: &Device) -> Option<Egl> {
            // SAFETY: no native display handle is passed.
            let display = unsafe { Display::with_device(device, None) }.ok()?;
            let template =
                ConfigTemplateBuilder::new().with_alpha_size(8).with_surface_type(ConfigSurfaceTypes::PBUFFER).build();
            // SAFETY: the template holds no native handles.
            let config = unsafe { display.find_configs(template) }.ok()?.next()?;
            let apis = [
                (ContextApi::OpenGl(Some(Version::new(3, 3))), Some(GlProfile::Core)),
                (ContextApi::Gles(Some(Version::new(3, 0))), None),
            ];
            for (api, profile) in apis {
                let mut attrs = ContextAttributesBuilder::new().with_context_api(api);
                if let Some(p) = profile {
                    attrs = attrs.with_profile(p);
                }
                // SAFETY: no window handle is passed.
                let Ok(context) = (unsafe { display.create_context(&config, &attrs.build(None)) }) else { continue };
                let size = std::num::NonZeroU32::new(16)?;
                let pbuffer = SurfaceAttributesBuilder::<PbufferSurface>::new().build(size, size);
                // SAFETY: the attributes hold no native handles.
                let surface = unsafe { display.create_pbuffer_surface(&config, &pbuffer) }.ok();
                let current = match &surface {
                    Some(s) => context.make_current(s),
                    None => context.make_current_surfaceless(),
                };
                if let Ok(context) = current {
                    return Some(Egl { display, _surface: surface, _context: context });
                }
            }
            None
        }

        pub fn proc_address(&self, name: &CStr) -> *const c_void {
            self.display.get_proc_address(name)
        }

        pub fn resolver(&self) -> impl Fn(&CStr) -> *const c_void + Send + 'static {
            let display = self.display.clone();
            move |name| display.get_proc_address(name)
        }
    }
}
