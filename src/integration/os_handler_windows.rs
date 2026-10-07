//! Windows: per-user registration under HKCU (no administrator rights needed).
//!
//! Writes `Corrent.Magnet` / `Corrent.Torrent` ProgIDs, a Capabilities entry so Corrent
//! appears in Settings → Default apps, and the direct `magnet` / `.torrent` classes that
//! Windows uses when the user has no explicit UserChoice. Another app's previous handler is
//! remembered and restored on unregister. The hash-protected UserChoice is never written.

use std::sync::Mutex;

use windows_registry::CURRENT_USER;

use super::{HandlerState, HandlerStatus, exe_path};

const MAGNET_PROGID: &str = "Corrent.Magnet";
const TORRENT_PROGID: &str = "Corrent.Torrent";

/// Registry root under HKCU ("Software" normally; a scratch key in tests).
static ROOT: Mutex<&'static str> = Mutex::new("Software");

fn root() -> &'static str {
    *ROOT.lock().unwrap_or_else(|e| e.into_inner())
}

fn k(path: &str) -> String {
    format!(r"{}\{path}", root())
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHChangeNotify(event: i32, flags: u32, item1: *const core::ffi::c_void, item2: *const core::ffi::c_void);
}

fn notify_shell() {
    // SHCNE_ASSOCCHANGED, SHCNF_IDLIST
    unsafe { SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null()) };
}

fn command() -> String {
    format!("\"{}\" \"%1\"", exe_path())
}

fn icon() -> String {
    format!("\"{}\",0", exe_path())
}

fn get(path: &str, value: &str) -> Option<String> {
    CURRENT_USER.open(k(path)).ok()?.get_string(value).ok()
}

fn is_corrent_cmd(cmd: &str) -> bool {
    cmd.to_lowercase().contains("corrent")
}

/// Friendly app name from a shell command like `"C:\Program Files\tixati\tixati.exe" "%1"`.
fn app_from_cmd(cmd: &str) -> String {
    let trimmed = cmd.trim();
    let exe = match trimmed.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => trimmed.split(' ').next().unwrap_or(trimmed),
    };
    let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file);
    let mut c = stem.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn write_progid(progid: &str, desc: &str, url_protocol: bool) -> windows_registry::Result<()> {
    let key = CURRENT_USER.create(k(&format!(r"Classes\{progid}")))?;
    key.set_string("", desc)?;
    if url_protocol {
        key.set_string("URL Protocol", "")?;
    }
    CURRENT_USER.create(k(&format!(r"Classes\{progid}\DefaultIcon")))?.set_string("", icon())?;
    CURRENT_USER.create(k(&format!(r"Classes\{progid}\shell\open\command")))?.set_string("", command())?;
    Ok(())
}

fn write_capabilities() -> windows_registry::Result<()> {
    let caps_path = k(r"Corrent\Capabilities");
    let caps = CURRENT_USER.create(&caps_path)?;
    caps.set_string("ApplicationName", "Corrent")?;
    caps.set_string("ApplicationDescription", "A beautiful, stable BitTorrent client")?;
    caps.set_string("ApplicationIcon", icon())?;
    CURRENT_USER.create(k(r"Corrent\Capabilities\URLAssociations"))?.set_string("magnet", MAGNET_PROGID)?;
    CURRENT_USER.create(k(r"Corrent\Capabilities\FileAssociations"))?.set_string(".torrent", TORRENT_PROGID)?;
    CURRENT_USER.create(k("RegisteredApplications"))?.set_string("Corrent", &caps_path)?;
    CURRENT_USER.create(k(r"Classes\Applications\corrent.exe\shell\open\command"))?.set_string("", command())?;
    Ok(())
}

pub fn register(magnet: bool, torrent: bool) -> anyhow::Result<()> {
    write_capabilities()?;
    if magnet {
        write_progid(MAGNET_PROGID, "URL:Magnet link", true)?;
        // Remember another app's handler so unregistering can give it back.
        if let Some(prev) = get(r"Classes\magnet\shell\open\command", "")
            && !is_corrent_cmd(&prev)
        {
            CURRENT_USER.create(k(r"Corrent\Previous"))?.set_string("magnet", &prev)?;
        }
        write_progid("magnet", "URL:Magnet link", true)?;
    }
    if torrent {
        write_progid(TORRENT_PROGID, "BitTorrent file", false)?;
        let ext = CURRENT_USER.create(k(r"Classes\.torrent"))?;
        if let Ok(prev) = ext.get_string("")
            && !prev.is_empty()
            && prev != TORRENT_PROGID
        {
            CURRENT_USER.create(k(r"Corrent\Previous"))?.set_string(".torrent", &prev)?;
        }
        ext.set_string("", TORRENT_PROGID)?;
        ext.set_string("Content Type", "application/x-bittorrent")?;
        CURRENT_USER.create(k(r"Classes\.torrent\OpenWithProgids"))?.set_string(TORRENT_PROGID, "")?;
    }
    notify_shell();
    Ok(())
}

pub fn unregister(magnet: bool, torrent: bool) -> anyhow::Result<()> {
    let previous = |name: &str| get(r"Corrent\Previous", name);
    if magnet {
        let _ = CURRENT_USER.remove_tree(k(&format!(r"Classes\{MAGNET_PROGID}")));
        let ours = get(r"Classes\magnet\shell\open\command", "").is_some_and(|c| is_corrent_cmd(&c));
        if ours {
            match previous("magnet") {
                Some(prev) => {
                    CURRENT_USER.create(k(r"Classes\magnet\shell\open\command"))?.set_string("", &prev)?;
                    let _ = CURRENT_USER.remove_tree(k(r"Classes\magnet\DefaultIcon"));
                }
                None => {
                    let _ = CURRENT_USER.remove_tree(k(r"Classes\magnet"));
                }
            }
        }
        if let Ok(key) = CURRENT_USER.open(k(r"Corrent\Capabilities\URLAssociations")) {
            let _ = key.remove_value("magnet");
        }
    }
    if torrent {
        let _ = CURRENT_USER.remove_tree(k(&format!(r"Classes\{TORRENT_PROGID}")));
        if let Ok(key) = CURRENT_USER.open(k(r"Classes\.torrent"))
            && key.get_string("").ok().as_deref() == Some(TORRENT_PROGID)
        {
            match previous(".torrent") {
                Some(prev) => key.set_string("", &prev)?,
                None => {
                    let _ = key.remove_value("");
                }
            }
        }
        if let Ok(key) = CURRENT_USER.open(k(r"Classes\.torrent\OpenWithProgids")) {
            let _ = key.remove_value(TORRENT_PROGID);
        }
        if let Ok(key) = CURRENT_USER.open(k(r"Corrent\Capabilities\FileAssociations")) {
            let _ = key.remove_value(".torrent");
        }
    }
    if magnet && torrent {
        let _ = CURRENT_USER.remove_tree(k("Corrent"));
        if let Ok(key) = CURRENT_USER.open(k("RegisteredApplications")) {
            let _ = key.remove_value("Corrent");
        }
        let _ = CURRENT_USER.remove_tree(k(r"Classes\Applications\corrent.exe"));
    }
    notify_shell();
    Ok(())
}

/// Classify one association. `class_cmd` is what Windows runs when there is no UserChoice.
fn classify(user_choice: Option<String>, our_progid: &str, our_cmd: Option<String>, class_cmd: Option<String>) -> HandlerState {
    let exe = exe_path().to_lowercase();
    let ours_here = |c: &Option<String>| c.as_ref().is_some_and(|c| c.to_lowercase().contains(&exe));
    match user_choice {
        Some(choice) if choice == our_progid => {
            if ours_here(&our_cmd) { HandlerState::Ours } else { HandlerState::OtherPath }
        }
        Some(choice) => HandlerState::OtherDefault(choice),
        None => match &class_cmd {
            Some(_) if ours_here(&class_cmd) => HandlerState::Ours,
            Some(c) if is_corrent_cmd(c) => HandlerState::OtherPath,
            Some(c) => HandlerState::OtherDefault(app_from_cmd(c)),
            None => HandlerState::NotRegistered,
        },
    }
}

pub fn status() -> HandlerStatus {
    let magnet = classify(
        get(r"Microsoft\Windows\Shell\Associations\UrlAssociations\magnet\UserChoice", "ProgId"),
        MAGNET_PROGID,
        get(&format!(r"Classes\{MAGNET_PROGID}\shell\open\command"), ""),
        get(r"Classes\magnet\shell\open\command", ""),
    );
    let ext_default = get(r"Classes\.torrent", "").filter(|s| !s.is_empty());
    let torrent_class_cmd =
        ext_default.as_ref().and_then(|progid| get(&format!(r"Classes\{progid}\shell\open\command"), ""));
    let torrent = classify(
        get(r"Microsoft\Windows\CurrentVersion\Explorer\FileExts\.torrent\UserChoice", "ProgId"),
        TORRENT_PROGID,
        get(&format!(r"Classes\{TORRENT_PROGID}\shell\open\command"), ""),
        torrent_class_cmd,
    );
    HandlerStatus { magnet, torrent }
}

pub fn open_system_default_settings() {
    let _ = open::that_detached("ms-settings:defaultapps?registeredAppUser=Corrent");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_roundtrip_restores_previous_handler() {
        *ROOT.lock().unwrap() = r"Software\CorrentTest";
        let _ = CURRENT_USER.remove_tree(r"Software\CorrentTest");
        // Simulate another client owning magnet links.
        let tixati = r#""C:\Program Files\tixati\tixati.exe" "%1""#;
        CURRENT_USER.create(k(r"Classes\magnet\shell\open\command")).unwrap().set_string("", tixati).unwrap();
        // Note: UserChoice is read from the scratch root too, so it is absent here.
        assert_eq!(status().magnet, HandlerState::OtherDefault("Tixati".into()));
        assert_eq!(status().torrent, HandlerState::NotRegistered);

        register(true, true).unwrap();
        let st = status();
        assert_eq!(st.magnet, HandlerState::Ours);
        assert_eq!(st.torrent, HandlerState::Ours);
        assert_eq!(get(r"Classes\Corrent.Magnet", "URL Protocol").as_deref(), Some(""));
        assert_eq!(get("RegisteredApplications", "Corrent").as_deref(), Some(r"Software\CorrentTest\Corrent\Capabilities"));

        unregister(true, true).unwrap();
        assert_eq!(get(r"Classes\magnet\shell\open\command", "").as_deref(), Some(tixati));
        assert!(CURRENT_USER.open(k("Corrent")).is_err());
        assert_eq!(status().magnet, HandlerState::OtherDefault("Tixati".into()));

        let _ = CURRENT_USER.remove_tree(r"Software\CorrentTest");
        *ROOT.lock().unwrap() = "Software";
    }

    #[test]
    fn app_names() {
        assert_eq!(app_from_cmd(r#""C:\Program Files\qBittorrent\qbittorrent.exe" "%1""#), "Qbittorrent");
        assert_eq!(app_from_cmd(r"C:\x\tixati.exe %1"), "Tixati");
    }
}
