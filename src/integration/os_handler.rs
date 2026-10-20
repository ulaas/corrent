//! Registering Corrent as the handler for `magnet:` links and `.torrent` files.
//!
//! Everything is per-user and needs no administrator rights.

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // each platform constructs only some of these
pub enum HandlerState {
    /// Corrent is the effective handler.
    Ours,
    /// Corrent is registered, but another app is the user's chosen default.
    OtherDefault(String),
    /// Registered for a different Corrent executable path (portable copy moved).
    OtherPath,
    NotRegistered,
    /// Managed by the package (Flatpak/Snap/app bundle) — nothing to do at runtime.
    Managed,
    Unsupported,
}

impl HandlerState {
    pub fn label(&self) -> String {
        match self {
            HandlerState::Ours => "Corrent (default)".into(),
            HandlerState::OtherDefault(a) => format!("Another app is the default ({a})"),
            HandlerState::OtherPath => "Registered to a different Corrent location".into(),
            HandlerState::NotRegistered => "Not registered".into(),
            HandlerState::Managed => "Managed by the installed package".into(),
            HandlerState::Unsupported => "Not supported on this platform".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandlerStatus {
    pub magnet: HandlerState,
    pub torrent: HandlerState,
}

pub fn exe_path() -> String {
    // Inside an AppImage the binary runs from a temporary mount; register the AppImage itself.
    if cfg!(target_os = "linux")
        && let Some(appimage) = std::env::var_os("APPIMAGE").filter(|p| !p.is_empty())
    {
        return appimage.to_string_lossy().into_owned();
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(&p).ok().or(Some(p)))
        .map(|p| {
            let s = p.to_string_lossy().into_owned();
            s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
        })
        .unwrap_or_else(|| "corrent".into())
}

pub use imp::{open_system_default_settings, register, status, unregister};

// ------------------------------------------------------------------------------------ Windows
#[cfg(windows)]
#[path = "os_handler_windows.rs"]
mod imp;

// ------------------------------------------------------------------------------------ Linux / BSD
#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::{HandlerState, HandlerStatus, exe_path};
    use std::path::PathBuf;
    use std::process::Command;

    const DESKTOP: &str = "corrent.desktop";

    /// Quote a path for a Desktop Entry `Exec=` key: wrap in double quotes and escape `"`, `` ` ``,
    /// `$` and `\` (backslashes are doubled again because the value is also a string).
    fn desktop_exec_quote(path: &str) -> String {
        let mut out = String::from("\"");
        for c in path.chars() {
            match c {
                '"' | '`' | '$' => {
                    out.push_str("\\\\");
                    out.push(c);
                }
                '\\' => out.push_str("\\\\\\\\"),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    fn sandboxed() -> bool {
        std::env::var_os("FLATPAK_ID").is_some() || std::env::var_os("SNAP").is_some()
    }

    fn data_home() -> PathBuf {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from(".local/share"))
    }

    fn desktop_file() -> PathBuf {
        data_home().join("applications").join(DESKTOP)
    }

    fn write_desktop_file() -> anyhow::Result<()> {
        let apps = data_home().join("applications");
        std::fs::create_dir_all(&apps)?;
        let icon_dir = data_home().join("icons/hicolor/256x256/apps");
        std::fs::create_dir_all(&icon_dir)?;
        let png = crate::util::encode_png(&crate::app::icon::icon_rgba(256), 256, 256);
        std::fs::write(icon_dir.join("corrent.png"), png)?;
        let exe = desktop_exec_quote(&exe_path());
        let content = format!(
            "[Desktop Entry]\nType=Application\nName=Corrent\nGenericName=BitTorrent Client\n\
             Comment=Download and share files over BitTorrent\nExec={exe} %U\nIcon=corrent\nTerminal=false\n\
             Categories=Network;FileTransfer;P2P;\nMimeType=x-scheme-handler/magnet;application/x-bittorrent;\n\
             StartupWMClass=corrent\nKeywords=torrent;magnet;download;p2p;\n"
        );
        std::fs::write(desktop_file(), content)?;
        // NO_WINDOW_OK: Linux only (no console windows there).
        let _ = Command::new("update-desktop-database").arg(&apps).status();
        Ok(())
    }

    fn xdg_default(mime: &str) -> anyhow::Result<()> {
        // NO_WINDOW_OK: Linux only.
        let st = Command::new("xdg-mime").args(["default", DESKTOP, mime]).status()?;
        anyhow::ensure!(st.success(), "xdg-mime failed for {mime}");
        Ok(())
    }

    pub fn register(magnet: bool, torrent: bool) -> anyhow::Result<()> {
        anyhow::ensure!(!sandboxed(), "registration is managed by the Flatpak/Snap package");
        write_desktop_file()?;
        if magnet {
            xdg_default("x-scheme-handler/magnet")?;
        }
        if torrent {
            xdg_default("application/x-bittorrent")?;
        }
        Ok(())
    }

    pub fn unregister(magnet: bool, torrent: bool) -> anyhow::Result<()> {
        if magnet && torrent {
            let _ = std::fs::remove_file(desktop_file());
            // NO_WINDOW_OK: Linux only.
            let _ = Command::new("update-desktop-database").arg(data_home().join("applications")).status();
        }
        Ok(())
    }

    fn query(mime: &str) -> HandlerState {
        if sandboxed() {
            return HandlerState::Managed;
        }
        // NO_WINDOW_OK: Linux only.
        let out = Command::new("xdg-mime").args(["query", "default", mime]).output();
        let current = out.ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let ours_exists = desktop_file().exists();
        if current == DESKTOP {
            let exec_ok = std::fs::read_to_string(desktop_file())
                .map(|c| c.contains(&desktop_exec_quote(&exe_path())))
                .unwrap_or(false);
            if exec_ok { HandlerState::Ours } else { HandlerState::OtherPath }
        } else if current.is_empty() {
            HandlerState::NotRegistered
        } else if ours_exists {
            HandlerState::OtherDefault(current.trim_end_matches(".desktop").to_string())
        } else {
            HandlerState::NotRegistered
        }
    }

    pub fn status() -> HandlerStatus {
        HandlerStatus { magnet: query("x-scheme-handler/magnet"), torrent: query("application/x-bittorrent") }
    }

    pub fn open_system_default_settings() {}
}

// ------------------------------------------------------------------------------------ macOS
#[cfg(target_os = "macos")]
mod imp {
    //! On macOS the URL scheme and document types are declared in the app bundle's Info.plist
    //! (see `[package.metadata.bundle]`). Launch Services registers the bundle on first launch;
    //! the user picks the default browser-style in Finder or via `duti`.
    use super::{HandlerState, HandlerStatus};

    fn in_bundle() -> bool {
        std::env::current_exe().map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/")).unwrap_or(false)
    }

    pub fn register(_magnet: bool, _torrent: bool) -> anyhow::Result<()> {
        anyhow::ensure!(in_bundle(), "run Corrent from its .app bundle to register it with Launch Services");
        let exe = std::env::current_exe()?;
        let bundle = exe.ancestors().nth(3).map(|p| p.to_path_buf()).ok_or_else(|| anyhow::anyhow!("no bundle"))?;
        let lsregister =
            "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
        // NO_WINDOW_OK: macOS only.
        let _ = std::process::Command::new(lsregister).arg("-f").arg(&bundle).status();
        Ok(())
    }

    pub fn unregister(_magnet: bool, _torrent: bool) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn status() -> HandlerStatus {
        let s = if in_bundle() { HandlerState::Managed } else { HandlerState::Unsupported };
        HandlerStatus { magnet: s.clone(), torrent: s }
    }

    pub fn open_system_default_settings() {}
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use super::{HandlerState, HandlerStatus};
    pub fn register(_: bool, _: bool) -> anyhow::Result<()> {
        anyhow::bail!("not supported")
    }
    pub fn unregister(_: bool, _: bool) -> anyhow::Result<()> {
        Ok(())
    }
    pub fn status() -> HandlerStatus {
        HandlerStatus { magnet: HandlerState::Unsupported, torrent: HandlerState::Unsupported }
    }
    pub fn open_system_default_settings() {}
}
