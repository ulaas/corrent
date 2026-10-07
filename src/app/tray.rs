//! System tray icon with a quick-actions menu.

use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum TrayAction {
    ToggleWindow,
    ShowWindow,
    PauseAll,
    ResumeAll,
    AltSpeed,
    Settings,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    show: MenuItem,
    pause_all: MenuItem,
    resume_all: MenuItem,
    alt: CheckMenuItem,
    settings: MenuItem,
    quit: MenuItem,
    menu_rx: Receiver<MenuEvent>,
    icon_rx: Receiver<TrayIconEvent>,
    last_tooltip: Instant,
}

impl Tray {
    /// Create the tray. Must run on the event-loop thread (Windows/macOS); failures are non-fatal.
    pub fn new(ctx: &egui::Context) -> Option<Tray> {
        let menu = Menu::new();
        let show = MenuItem::new("Show Corrent", true, None);
        let pause_all = MenuItem::new("Pause all", true, None);
        let resume_all = MenuItem::new("Resume all", true, None);
        let alt = CheckMenuItem::new("Alternative speed limits", true, false, None);
        let settings = MenuItem::new("Settings…", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let sep = PredefinedMenuItem::separator();
        let sep2 = PredefinedMenuItem::separator();
        if let Err(e) = menu.append_items(&[&show, &sep, &resume_all, &pause_all, &alt, &sep2, &settings, &quit]) {
            tracing::warn!("tray menu: {e}");
            return None;
        }
        let rgba = super::icon::icon_rgba(64);
        let icon = Icon::from_rgba(rgba, 64, 64).ok()?;

        // Forward events through our own channels and wake the UI (it may be hidden).
        let (menu_tx, menu_rx) = channel();
        let (icon_tx, icon_rx) = channel();
        let c1 = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = menu_tx.send(e);
            c1.request_repaint();
        }));
        let c2 = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            let _ = icon_tx.send(e);
            c2.request_repaint();
        }));

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Corrent")
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build();
        match tray {
            Ok(icon) => Some(Tray {
                icon,
                show,
                pause_all,
                resume_all,
                alt,
                settings,
                quit,
                menu_rx,
                icon_rx,
                last_tooltip: Instant::now(),
            }),
            Err(e) => {
                tracing::warn!("could not create tray icon: {e}");
                None
            }
        }
    }

    pub fn poll(&self) -> Vec<TrayAction> {
        let mut out = Vec::new();
        while let Ok(e) = self.menu_rx.try_recv() {
            let id = e.id();
            if id == self.show.id() {
                out.push(TrayAction::ShowWindow);
            } else if id == self.pause_all.id() {
                out.push(TrayAction::PauseAll);
            } else if id == self.resume_all.id() {
                out.push(TrayAction::ResumeAll);
            } else if id == self.alt.id() {
                out.push(TrayAction::AltSpeed);
            } else if id == self.settings.id() {
                out.push(TrayAction::Settings);
            } else if id == self.quit.id() {
                out.push(TrayAction::Quit);
            }
        }
        while let Ok(e) = self.icon_rx.try_recv() {
            match e {
                TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => {
                    out.push(TrayAction::ToggleWindow)
                }
                TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => out.push(TrayAction::ShowWindow),
                _ => {}
            }
        }
        out
    }

    pub fn update(&mut self, down: &str, up: &str, active: usize, alt: bool) {
        if self.alt.is_checked() != alt {
            self.alt.set_checked(alt);
        }
        if self.last_tooltip.elapsed().as_secs_f32() < 2.0 {
            return;
        }
        self.last_tooltip = Instant::now();
        let tip = format!("Corrent\n↓ {down}   ↑ {up}\n{active} active");
        let _ = self.icon.set_tooltip(Some(tip));
    }
}
