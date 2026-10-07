use egui::Context;

use super::CorrentApp;
use crate::config::Settings;
use crate::ui::theme::Palette;

pub struct SettingsDraft {
    pub settings: Settings,
}

impl CorrentApp {
    pub fn open_settings(&mut self) {
        self.settings_draft = Some(SettingsDraft { settings: self.settings.clone() });
    }

    pub(super) fn settings_window(&mut self, _ctx: &Context, _p: &Palette) {}
}
