//! "Create torrent" dialog.

use crate::ui::widgets::Labeled;
use std::path::PathBuf;

use egui::{Align, Context, Id, Layout, RichText, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use super::pickers::{PickKind, PickTarget};
use crate::engine::EngineCommand;
use crate::engine::creator::{CreateRequest, auto_piece_length, total_size};
use crate::ui::theme::{self, Palette};
use crate::ui::widgets::{primary_button, secondary_button};
use crate::util::fmt_bytes;

pub struct CreatorState {
    pub source: Option<PathBuf>,
    pub source_is_file: bool,
    pub size: u64,
    /// Folder size being computed off the UI thread.
    size_rx: Option<std::sync::mpsc::Receiver<u64>>,
    /// Id of the create request in flight.
    pub request: Option<u64>,
    pub name: String,
    pub trackers: String,
    /// 0 = automatic.
    pub piece_length: u32,
    pub private: bool,
    pub comment: String,
    pub seed: bool,
    pub output: Option<PathBuf>,
    pub busy: bool,
}

impl CreatorState {
    pub fn new(default_trackers: &[String]) -> Self {
        CreatorState {
            source: None,
            source_is_file: false,
            size: 0,
            size_rx: None,
            request: None,
            name: String::new(),
            trackers: default_trackers.join("\n"),
            piece_length: 0,
            private: false,
            comment: String::new(),
            seed: true,
            output: None,
            busy: false,
        }
    }

    pub fn set_source(&mut self, p: PathBuf, rt: &tokio::runtime::Handle, ctx: &egui::Context) {
        // Walking a big folder takes a while: do it off the UI thread.
        let (tx, rx) = std::sync::mpsc::channel();
        let (path, ctx) = (p.clone(), ctx.clone());
        rt.spawn_blocking(move || {
            let _ = tx.send(total_size(&path));
            ctx.request_repaint();
        });
        self.size = 0;
        self.size_rx = Some(rx);
        self.source_is_file = p.is_file();
        self.name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parent = p.parent().map(PathBuf::from).unwrap_or_default();
        self.output = Some(parent.join(format!("{}.torrent", self.name)));
        self.source = Some(p);
    }
}

const PIECE_SIZES: [u32; 9] = [0, 64, 128, 256, 512, 1024, 2048, 4096, 8192];

impl CorrentApp {
    pub fn open_creator(&mut self) {
        if self.creator.is_none() {
            self.creator = Some(CreatorState::new(&self.settings.bittorrent.default_trackers));
        }
    }

    pub(super) fn creator_dialog(&mut self, ctx: &Context, p: &Palette) {
        let Some(mut c) = self.creator.take() else { return };
        if let Some(size) = c.size_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            c.size = size;
            c.size_rx = None;
        }
        let mut close = false;
        let mut pick: Option<(PickTarget, PickKind)> = None;
        let busy_picker = self.picker_busy();
        let resp = egui::Modal::new(Id::new("creator")).show(ctx, |ui| {
            ui.set_width(600.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::FILE_PLUS).size(20.0).color(p.accent));
                ui.label(RichText::new(t!("Create torrent")).font(theme::title()).color(p.text));
            });
            ui.add_space(10.0);
            egui::Grid::new("creator_grid").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(t!("Source"));
                ui.horizontal(|ui| {
                    let label = c
                        .source
                        .as_ref()
                        .map(|s| s.display().to_string())
                        .unwrap_or_else(|| t!("Choose a file or folder…").into());
                    ui.add(egui::Label::new(RichText::new(label).color(p.text_dim)).truncate());
                });
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!busy_picker && !c.busy, |ui| {
                        if ui.button(format!("{}  {}", icons::FILE, t!("File…"))).clicked() {
                            pick = Some((PickTarget::CreatorSource, PickKind::AnyFile));
                        }
                        if ui.button(format!("{}  {}", icons::FOLDER, t!("Folder…"))).clicked() {
                            pick = Some((PickTarget::CreatorSource, PickKind::Folder));
                        }
                    });
                    if c.size_rx.is_some() {
                        ui.add(egui::Spinner::new().size(12.0));
                    } else if c.size > 0 {
                        ui.label(RichText::new(fmt_bytes(c.size)).small().color(p.text_dim));
                    }
                });
                ui.end_row();
                ui.label(t!("Name"));
                let is_file = c.source_is_file;
                ui.add_enabled(!is_file, egui::TextEdit::singleline(&mut c.name).desired_width(420.0))
                    .on_disabled_hover_text(t!("A single-file torrent is named after its file"));
                ui.end_row();
                ui.label(t!("Trackers"));
                ui.add(
                    egui::TextEdit::multiline(&mut c.trackers)
                        .desired_rows(3)
                        .desired_width(420.0)
                        .hint_text(t!("One per line (optional; DHT works without)"))
                        .font(egui::TextStyle::Monospace),
                );
                ui.end_row();
                ui.label(t!("Piece size"));
                let auto = auto_piece_length(c.size.max(1));
                egui::ComboBox::from_id_salt("piece_size")
                    .selected_text(if c.piece_length == 0 {
                        format!("Automatic ({})", fmt_bytes(auto as u64))
                    } else {
                        fmt_bytes(c.piece_length as u64)
                    })
                    .show_ui(ui, |ui| {
                        for kib in PIECE_SIZES {
                            let bytes = kib * 1024;
                            let label = if kib == 0 {
                                format!("Automatic ({})", fmt_bytes(auto as u64))
                            } else {
                                fmt_bytes(bytes as u64)
                            };
                            ui.selectable_value(&mut c.piece_length, bytes, label);
                        }
                    });
                ui.end_row();
                ui.label(t!("Comment"));
                ui.add(egui::TextEdit::singleline(&mut c.comment).desired_width(420.0));
                ui.end_row();
                ui.label(t!("Save .torrent as"));
                ui.horizontal(|ui| {
                    let label = c.output.as_ref().map(|s| s.display().to_string()).unwrap_or_default();
                    ui.add(egui::Label::new(RichText::new(label).color(p.text_dim)).truncate());
                    if ui
                        .add_enabled(!busy_picker && c.source.is_some(), egui::Button::new(icons::FOLDER_OPEN))
                        .labeled(t!("Choose where to save the .torrent file"))
                        .clicked()
                    {
                        pick = Some((PickTarget::CreatorOutput, PickKind::SaveTorrent(format!("{}.torrent", c.name))));
                    }
                });
                ui.end_row();
            });
            ui.add_space(6.0);
            ui.checkbox(&mut c.private, t!("Private torrent (only trackers, no DHT/peer exchange)"));
            ui.checkbox(&mut c.seed, t!("Start seeding right away"));
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ready = c.source.is_some() && c.output.is_some() && !c.name.trim().is_empty() && !c.busy;
                let label =
                    if c.busy { t!("Hashing…").to_string() } else { format!("{}  {}", icons::CHECK, t!("Create")) };
                if ui.add_enabled_ui(ready, |ui| primary_button(ui, p, &label)).inner.clicked()
                    && let (Some(source), Some(save_to)) = (c.source.clone(), c.output.clone())
                {
                    c.busy = true;
                    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    c.request = Some(id);
                    let req = CreateRequest {
                        id,
                        source,
                        name: Some(c.name.trim().to_string()),
                        trackers: c
                            .trackers
                            .lines()
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                            .map(String::from)
                            .collect(),
                        // The size shown as "Automatic" is the one used.
                        piece_length: Some(if c.piece_length > 0 {
                            c.piece_length
                        } else {
                            auto_piece_length(c.size.max(1))
                        }),
                        private: c.private,
                        comment: Some(c.comment.clone()),
                        save_to,
                        seed: c.seed,
                    };
                    self.engine.send(EngineCommand::CreateTorrent(Box::new(req)));
                }
                if c.busy {
                    ui.add(egui::Spinner::new().color(p.accent));
                }
                if secondary_button(ui, t!("Close")).clicked() {
                    close = true;
                }
            });
        });
        if resp.should_close() && !c.busy {
            close = true;
        }
        if let Some((target, kind)) = pick {
            let start = c.source.as_ref().and_then(|s| s.parent().map(PathBuf::from));
            self.pick(target, kind, start);
        }
        if !close {
            self.creator = Some(c);
        }
    }
}
