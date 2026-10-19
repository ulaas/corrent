//! Saving when the OS ends the session: Windows logoff/shutdown (`WM_QUERYENDSESSION`, then
//! `WM_ENDSESSION`) and `SIGTERM`/`SIGHUP` on Unix. Corrent often sits hidden in the tray then,
//! and without this the store's debounced changes and the engine's piece state are lost.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::engine::EngineHandle;

/// The engine the app uses right now (it changes when the app connects to a server).
static ENGINE: Mutex<Option<EngineHandle>> = Mutex::new(None);
static PATHS: OnceLock<crate::config::Paths> = OnceLock::new();

/// Remember the current engine; call again whenever the app switches engines.
pub fn track(engine: &EngineHandle, paths: &crate::config::Paths) {
    *ENGINE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(engine.clone());
    let _ = PATHS.set(paths.clone());
}

fn current() -> Option<EngineHandle> {
    ENGINE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// The session may end: save now, keep running (another app may still cancel the logoff).
#[cfg(windows)]
fn flush() {
    if let Some(e) = current()
        && !e.flush_blocking(Duration::from_secs(3))
    {
        tracing::warn!("the engine didn't save in time before the session ends");
    }
}

/// The session ends: stop the engine within the few seconds Windows allows, then leave.
fn end() -> ! {
    tracing::info!("the session is ending; stopping");
    // A settings save may still be queued on the writer thread.
    if let Some(p) = PATHS.get() {
        corrent_core::writer::global().flush(&p.settings_file(), Duration::from_secs(1));
    }
    if let Some(e) = current() {
        e.shutdown_blocking(Duration::from_secs(3));
    }
    if let Some(p) = PATHS.get() {
        crate::crash::finish(&p.log_dir());
    }
    corrent_core::logfile::flush(Duration::from_millis(500));
    std::process::exit(0)
}

#[cfg(any(windows, test))]
const WM_QUERYENDSESSION: u32 = 0x0011;
#[cfg(any(windows, test))]
const WM_ENDSESSION: u32 = 0x0016;

/// How a window message is answered: `Some(result)` when it is ours. `WM_QUERYENDSESSION` always
/// gets TRUE: Corrent saves, but never stands in the way of a logoff.
#[cfg(any(windows, test))]
fn on_message(msg: u32, wparam: usize, flush: impl FnOnce(), end: impl FnOnce()) -> Option<isize> {
    match msg {
        WM_QUERYENDSESSION => {
            flush();
            Some(1)
        }
        WM_ENDSESSION => {
            if wparam != 0 {
                end();
            }
            Some(0)
        }
        _ => None,
    }
}

/// Watch the main window's messages (subclassing keeps winit's own handling intact).
#[cfg(windows)]
pub fn install(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    type Hwnd = *mut core::ffi::c_void;
    type SubclassProc = unsafe extern "system" fn(Hwnd, u32, usize, isize, usize, usize) -> isize;

    #[link(name = "comctl32")]
    unsafe extern "system" {
        fn SetWindowSubclass(hwnd: Hwnd, proc: SubclassProc, id: usize, data: usize) -> i32;
        fn DefSubclassProc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShutdownBlockReasonCreate(hwnd: Hwnd, reason: *const u16) -> i32;
        fn ShutdownBlockReasonDestroy(hwnd: Hwnd) -> i32;
    }

    unsafe extern "system" fn proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize, _: usize, _: usize) -> isize {
        if msg == WM_QUERYENDSESSION {
            // Shown by Windows if the save takes long enough for it to list blocking apps.
            let reason: Vec<u16> = t!("Saving your torrents…").encode_utf16().chain([0]).collect();
            // SAFETY: our own window; the string is NUL-terminated and outlives the call.
            unsafe { ShutdownBlockReasonCreate(hwnd, reason.as_ptr()) };
        }
        // A panic must not cross into the window procedure.
        let handled = std::panic::catch_unwind(|| on_message(msg, wparam, flush, || end())).unwrap_or(None);
        if msg == WM_QUERYENDSESSION {
            // SAFETY: as above.
            unsafe { ShutdownBlockReasonDestroy(hwnd) };
        }
        match handled {
            Some(r) => r,
            // SAFETY: forwarding the message we were given to the next procedure in the chain.
            None => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
        }
    }

    let Ok(handle) = cc.window_handle() else { return };
    let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
    let hwnd = win.hwnd.get() as Hwnd;
    // SAFETY: a valid HWND created on this (the UI) thread; `proc` lives for the whole process.
    if unsafe { SetWindowSubclass(hwnd, proc, 1, 0) } == 0 {
        tracing::warn!("could not watch for the end of the Windows session");
    }
}

/// `SIGTERM` (logout, `systemctl` stop, `kill`) and `SIGHUP` (terminal closed): save and stop.
#[cfg(unix)]
pub fn install_signals(rt: &tokio::runtime::Handle) {
    use tokio::signal::unix::{SignalKind, signal};
    rt.spawn(async {
        let (Ok(mut term), Ok(mut hup)) = (signal(SignalKind::terminate()), signal(SignalKind::hangup())) else {
            tracing::warn!("could not watch for SIGTERM/SIGHUP");
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
        // The engine runs on this runtime: stop it from a blocking thread.
        let _ = tokio::task::spawn_blocking(|| end()).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn the_end_of_the_session_saves_first_and_never_says_no() {
        let calls = RefCell::new(Vec::new());
        let r =
            on_message(WM_QUERYENDSESSION, 0, || calls.borrow_mut().push("flush"), || calls.borrow_mut().push("end"));
        assert_eq!(r, Some(1), "TRUE: the logoff may go ahead");
        assert_eq!(*calls.borrow(), ["flush"]);
        // Someone else cancelled the logoff: keep running.
        let r = on_message(WM_ENDSESSION, 0, || calls.borrow_mut().push("flush"), || calls.borrow_mut().push("end"));
        assert_eq!(r, Some(0));
        assert_eq!(*calls.borrow(), ["flush"]);
        on_message(WM_ENDSESSION, 1, || calls.borrow_mut().push("flush"), || calls.borrow_mut().push("end"));
        assert_eq!(*calls.borrow(), ["flush", "end"]);
        assert_eq!(on_message(0x000F, 0, || panic!("not ours"), || panic!("not ours")), None, "WM_PAINT");
    }
}
