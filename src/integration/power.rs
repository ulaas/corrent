//! Sleep / shut down the computer ("when downloads finish"). Only ever called after the user
//! chose it and a cancellable countdown ran out.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WhenDone {
    #[default]
    Nothing,
    Quit,
    Sleep,
    Shutdown,
}

impl WhenDone {
    pub const ALL: [WhenDone; 4] = [WhenDone::Nothing, WhenDone::Quit, WhenDone::Sleep, WhenDone::Shutdown];

    pub fn label(self) -> &'static str {
        match self {
            WhenDone::Nothing => t!("Do nothing"),
            WhenDone::Quit => t!("Quit Corrent"),
            WhenDone::Sleep => t!("Put the computer to sleep"),
            WhenDone::Shutdown => t!("Shut down the computer"),
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            WhenDone::Nothing => "",
            WhenDone::Quit => t!("Corrent will quit"),
            WhenDone::Sleep => t!("The computer will go to sleep"),
            WhenDone::Shutdown => t!("The computer will shut down"),
        }
    }
}

/// The OS command for sleep/shutdown: (program, args).
pub fn command(action: WhenDone) -> Option<(&'static str, Vec<&'static str>)> {
    match action {
        WhenDone::Nothing | WhenDone::Quit => None,
        WhenDone::Sleep => Some(if cfg!(windows) {
            // SetSuspendState(Suspend) sleeps even when hibernation is enabled (rundll32 would hibernate).
            (
                "powershell.exe",
                vec![
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Application]::SetSuspendState('Suspend', $false, $false)",
                ],
            )
        } else if cfg!(target_os = "macos") {
            ("pmset", vec!["sleepnow"])
        } else {
            ("systemctl", vec!["suspend"])
        }),
        WhenDone::Shutdown => Some(if cfg!(windows) {
            ("shutdown.exe", vec!["/s", "/t", "5", "/c", "Corrent: downloads finished"])
        } else if cfg!(target_os = "macos") {
            ("osascript", vec!["-e", "tell application \"System Events\" to shut down"])
        } else {
            ("systemctl", vec!["poweroff"])
        }),
    }
}

/// Run the OS action (detached).
pub fn perform(action: WhenDone) -> std::io::Result<()> {
    let Some((prog, args)) = command(action) else { return Ok(()) };
    tracing::info!("when done: {action:?}");
    let mut c = std::process::Command::new(prog);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_exist_for_power_actions_only() {
        assert!(command(WhenDone::Nothing).is_none());
        assert!(command(WhenDone::Quit).is_none());
        assert!(command(WhenDone::Sleep).is_some());
        let (_, args) = command(WhenDone::Shutdown).unwrap();
        assert!(!args.is_empty());
    }
}
