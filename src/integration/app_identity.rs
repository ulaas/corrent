//! Windows notification identity: an AppUserModelID registered per user so toasts show
//! "Corrent" with its icon instead of "Windows PowerShell". No admin rights or shortcut needed.

pub const APP_ID: &str = "Corrent.Corrent";

/// Register (idempotently) and return the app id to use for notifications, if available.
#[cfg(windows)]
pub fn ensure(paths: &crate::config::Paths) -> Option<&'static str> {
    use std::sync::OnceLock;
    static OK: OnceLock<bool> = OnceLock::new();
    // Portable/test profiles must not touch the per-user registry (and would point the icon
    // at a folder that may disappear); they use the plain notification identity.
    if paths.portable {
        return None;
    }
    let ok = *OK.get_or_init(|| {
        let icon = paths.data_dir.join("corrent-notify.png");
        if !icon.exists() {
            let png = crate::util::encode_png(&crate::app::icon::icon_rgba(64), 64, 64);
            if std::fs::write(&icon, png).is_err() {
                return false;
            }
        }
        let key = format!(r"Software\Classes\AppUserModelId\{APP_ID}");
        let result = windows_registry::CURRENT_USER.create(&key).and_then(|k| {
            k.set_string("DisplayName", "Corrent")?;
            k.set_string("IconUri", icon.to_string_lossy().as_ref())
        });
        result.is_ok()
    });
    ok.then_some(APP_ID)
}

#[cfg(windows)]
pub fn remove() {
    let _ = windows_registry::CURRENT_USER.remove_tree(format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
}

#[cfg(not(windows))]
pub fn ensure(_paths: &crate::config::Paths) -> Option<&'static str> {
    None
}

#[cfg(not(windows))]
pub fn remove() {}
