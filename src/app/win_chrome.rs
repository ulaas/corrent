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

#[cfg(not(windows))]
pub fn round_corners(_cc: &eframe::CreationContext<'_>) -> bool {
    false
}
