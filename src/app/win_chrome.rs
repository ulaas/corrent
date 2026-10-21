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
pub fn fullscreen(_window: &impl raw_window_handle::HasWindowHandle, _on: bool) {}

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
