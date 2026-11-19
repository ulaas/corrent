//! Windows 11 window chrome for the borderless window: rounded corners (and the native border
//! that comes with them). No-ops elsewhere and on Windows 10, where the call simply fails.

/// Ask DWM for rounded corners. Returns true when Windows accepted it (Windows 11+).
#[cfg(windows)]
pub fn round_corners(cc: &eframe::CreationContext<'_>) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut core::ffi::c_void,
            attr: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;

    let Ok(handle) = cc.window_handle() else { return false };
    let RawWindowHandle::Win32(win) = handle.as_raw() else { return false };
    let hwnd = win.hwnd.get() as *mut core::ffi::c_void;
    // SAFETY: valid HWND owned by this process; the value is a u32 as documented by DWM.
    let hr =
        unsafe { DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, (&DWMWCP_ROUND as *const u32).cast(), 4) };
    hr >= 0
}

/// Fullscreen drops the rounded corners and the native border DWM keeps drawing for a window
/// that asked for them (a playing video showed a hairline and cut corners); leaving restores them.
#[cfg(windows)]
pub fn fullscreen(window: &impl raw_window_handle::HasWindowHandle, on: bool) {
    use raw_window_handle::RawWindowHandle;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut core::ffi::c_void,
            attr: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWCP_DONOTROUND: u32 = 1;
    const DWMWCP_ROUND: u32 = 2;
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    const DWMWA_COLOR_DEFAULT: u32 = 0xFFFF_FFFF;

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
    let hwnd = win.hwnd.get() as *mut core::ffi::c_void;
    let (corners, border) =
        if on { (DWMWCP_DONOTROUND, DWMWA_COLOR_NONE) } else { (DWMWCP_ROUND, DWMWA_COLOR_DEFAULT) };
    // SAFETY: valid HWND owned by this process; both values are u32s as documented by DWM.
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, (&corners as *const u32).cast(), 4);
        DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, (&border as *const u32).cast(), 4);
    }
}

#[cfg(not(windows))]
pub fn fullscreen<W>(_window: &W, _on: bool) {}

/// What Windows paints where a resize uncovers the window, until the next frame is there: the
/// window class comes without a background brush, and the unpainted part shows white. Cheap to
/// call every frame (it does something only when the colour changes). The class is shared by all
/// of the app's windows.
#[cfg(windows)]
pub fn background(window: &impl raw_window_handle::HasWindowHandle, color: egui::Color32) {
    use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

    use raw_window_handle::RawWindowHandle;

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateSolidBrush(color: u32) -> *mut core::ffi::c_void;
        fn DeleteObject(object: *mut core::ffi::c_void) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetClassLongPtrW(hwnd: *mut core::ffi::c_void, index: i32, value: isize) -> usize;
    }
    const GCLP_HBRBACKGROUND: i32 = -10;
    /// The colour the class has now (`u32::MAX`: none set), and its brush.
    static COLOR: AtomicU32 = AtomicU32::new(u32::MAX);
    static BRUSH: AtomicIsize = AtomicIsize::new(0);

    // COLORREF is 0x00BBGGRR.
    let colorref = u32::from(color.r()) | u32::from(color.g()) << 8 | u32::from(color.b()) << 16;
    if COLOR.load(Ordering::Relaxed) == colorref {
        return;
    }
    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
    let hwnd = win.hwnd.get() as *mut core::ffi::c_void;
    // SAFETY: valid HWND owned by this process; the brush belongs to the class from here on,
    // and the one it replaces is ours and no longer used.
    unsafe {
        let brush = CreateSolidBrush(colorref);
        if brush.is_null() {
            return;
        }
        SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, brush as isize);
        let old = BRUSH.swap(brush as isize, Ordering::Relaxed);
        if old != 0 {
            DeleteObject(old as *mut core::ffi::c_void);
        }
    }
    COLOR.store(colorref, Ordering::Relaxed);
}

#[cfg(not(windows))]
pub fn background<W>(_window: &W, _color: egui::Color32) {}

/// Whether an erase request should paint the background: only when the window's size changed
/// since the last request. Windows also asks when the window is activated or deactivated (the
/// frame changes colour), and painting then covered the whole picture until the next frame.
#[cfg(any(windows, test))]
fn erase_paints(last: &std::sync::atomic::AtomicU64, width: i32, height: i32) -> bool {
    let size = (u64::from(width.cast_unsigned()) << 32) | u64::from(height.cast_unsigned());
    last.swap(size, std::sync::atomic::Ordering::Relaxed) != size
}

/// Keeps the class's background brush (see `background`) to what it is for: the main window
/// answers erase requests that come without a resize itself, without painting.
#[cfg(windows)]
pub fn keep_picture(cc: &eframe::CreationContext<'_>) {
    use std::sync::atomic::AtomicU64;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    type Hwnd = *mut core::ffi::c_void;
    type SubclassProc = unsafe extern "system" fn(Hwnd, u32, usize, isize, usize, usize) -> isize;

    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[link(name = "comctl32")]
    unsafe extern "system" {
        fn SetWindowSubclass(hwnd: Hwnd, proc: SubclassProc, id: usize, data: usize) -> i32;
        fn DefSubclassProc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetClientRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
    }
    const WM_ERASEBKGND: u32 = 0x0014;
    /// Not the id of `integration::end_session`'s subclass.
    const SUBCLASS_ID: usize = 2;
    static LAST_SIZE: AtomicU64 = AtomicU64::new(u64::MAX);

    unsafe extern "system" fn proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize, _: usize, _: usize) -> isize {
        if msg == WM_ERASEBKGND {
            let mut rect = Rect { left: 0, top: 0, right: 0, bottom: 0 };
            // SAFETY: our own window, and a rectangle that outlives the call.
            let known = unsafe { GetClientRect(hwnd, &mut rect) } != 0;
            let paints = !known || erase_paints(&LAST_SIZE, rect.right - rect.left, rect.bottom - rect.top);
            if !paints {
                // "Erased": nothing more is asked for, and the picture stays.
                return 1;
            }
        }
        // SAFETY: forwarding the message we were given to the next procedure in the chain.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    let Ok(handle) = cc.window_handle() else { return };
    let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
    // SAFETY: a valid HWND created on this (the UI) thread; `proc` lives for the whole process.
    unsafe { SetWindowSubclass(win.hwnd.get() as Hwnd, proc, SUBCLASS_ID, 0) };
}

#[cfg(not(windows))]
pub fn keep_picture(_cc: &eframe::CreationContext<'_>) {}

/// Mica spike (roadmap B5b, debug builds only, `CORRENT_DEV_MICA=1`): ask DWM for the Mica
/// backdrop and extend it into the client area. Needs a transparent window and clear color.
#[cfg(all(windows, debug_assertions))]
pub fn mica(cc: &eframe::CreationContext<'_>) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[repr(C)]
    struct Margins {
        left: i32,
        right: i32,
        top: i32,
        bottom: i32,
    }
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut core::ffi::c_void,
            attr: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
        fn DwmExtendFrameIntoClientArea(hwnd: *mut core::ffi::c_void, margins: *const Margins) -> i32;
    }
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMSBT_MAINWINDOW: u32 = 2; // Mica

    let Ok(handle) = cc.window_handle() else { return false };
    let RawWindowHandle::Win32(win) = handle.as_raw() else { return false };
    let hwnd = win.hwnd.get() as *mut core::ffi::c_void;
    let dark: u32 = 1;
    let margins = Margins { left: -1, right: -1, top: -1, bottom: -1 };
    // SAFETY: valid HWND owned by this process; values match the documented DWM types.
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, (&dark as *const u32).cast(), 4);
        let a = DwmExtendFrameIntoClientArea(hwnd, &margins);
        let b = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, (&DWMSBT_MAINWINDOW as *const u32).cast(), 4);
        tracing::info!("mica spike: extend frame hr={a:#x}, backdrop hr={b:#x}");
        a >= 0 && b >= 0
    }
}

#[cfg(not(windows))]
pub fn round_corners(_cc: &eframe::CreationContext<'_>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use super::erase_paints;

    #[test]
    fn only_a_resize_paints_the_background() {
        let last = AtomicU64::new(u64::MAX);
        assert!(erase_paints(&last, 1280, 800), "the first request: nothing is drawn yet");
        assert!(!erase_paints(&last, 1280, 800), "the window lost the focus: the picture stays");
        assert!(erase_paints(&last, 1300, 800), "wider");
        assert!(erase_paints(&last, 1300, 820), "taller");
        assert!(!erase_paints(&last, 1300, 820));
        assert!(erase_paints(&last, 800, 1300), "the sides swapped");
    }
}
