//! Looking metadata up again: one item (its page) or a whole library (Settings → Libraries),
//! with what came of it — matches, items without one, and the providers' own errors.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use corrent_proto::library::{MetadataRun, RefreshMode, RefreshOutcome};
use egui::{RichText, Ui};
use egui_phosphor::regular as icons;

use super::client::{MediaClient, Slot, fetch, ready};
use crate::app::CorrentApp;
use crate::engine::ToastKind;
use crate::ui::theme::Palette;
use crate::ui::widgets::menu_button;

#[derive(Default)]
pub struct RefreshUi {
    /// An item's refresh on its way.
    item: Option<Slot<RefreshOutcome>>,
    /// Each library's running or last run, as last heard.
    runs: HashMap<i64, MetadataRun>,
    /// Status requests on their way (a start, or a poll while it runs).
    asking: HashMap<i64, Slot<MetadataRun>>,
    /// When each library's run was last asked for (Settings → Libraries asks every few seconds:
    /// runs also start on their own after scans).
    loaded: HashMap<i64, Instant>,
    polled: Option<Instant>,
}

/// The "Refresh metadata" menu: `Some(mode)` when one of its entries was chosen.
pub fn refresh_menu(ui: &mut Ui) -> Option<RefreshMode> {
    let mut chosen = None;
    menu_button(ui, format!("{}  {}", icons::ARROWS_CLOCKWISE, t!("Refresh metadata")), |ui| {
        if ui
            .button(t!("Fill in missing details"))
            .on_hover_text(t!("Looks up what has no details yet, including what no provider matched before."))
            .clicked()
        {
            chosen = Some(RefreshMode::Missing);
            ui.close();
        }
        if ui
            .button(t!("Replace all details"))
            .on_hover_text(t!("Fetches everything from the providers again. Fields you edited and locked stay."))
            .clicked()
        {
            chosen = Some(RefreshMode::All);
            ui.close();
        }
    });
    chosen
}

impl CorrentApp {
    pub(super) fn refresh_item(&mut self, client: &MediaClient, item: i64, mode: RefreshMode) {
        let c = client.clone();
        let body = corrent_proto::library::RefreshRequest { mode };
        let path = format!("/api/v1/items/{item}/refresh-metadata");
        self.media.refresh.item =
            Some(fetch(&self.rt, &self.ctx, async move { c.send(reqwest::Method::POST, &path, &body).await }));
    }

    pub(super) fn refresh_library(&mut self, client: &MediaClient, library: i64, mode: RefreshMode) {
        let c = client.clone();
        let body = corrent_proto::library::RefreshRequest { mode };
        let path = format!("/api/v1/libraries/{library}/refresh-metadata");
        let slot = fetch(&self.rt, &self.ctx, async move { c.send(reqwest::Method::POST, &path, &body).await });
        self.media.refresh.asking.insert(library, slot);
        self.media.refresh.loaded.insert(library, Instant::now());
    }

    /// Answers that arrived, and polls of running library refreshes (once a second).
    pub(super) fn refresh_tick(&mut self, client: &MediaClient) {
        let r = &mut self.media.refresh;
        if let Some(o) = r.item.as_ref().and_then(ready) {
            r.item = None;
            match o {
                Ok(o) => {
                    let (kind, text) = item_outcome(&o);
                    self.toasts.push(kind, text);
                    if o.provider.is_some() {
                        self.media.invalidate();
                    }
                }
                Err(e) => self.toasts.push(ToastKind::Error, e),
            }
        }
        let r = &mut self.media.refresh;
        let done: Vec<i64> = r.asking.iter().filter(|(_, s)| ready(s).is_some()).map(|(id, _)| *id).collect();
        let mut finished = false;
        for id in done {
            if let Some(Some(Ok(run))) = r.asking.remove(&id).map(|s| ready(&s)) {
                let was_running = r.runs.get(&id).is_some_and(|x| x.running);
                finished |= was_running && !run.running && run.matched > 0;
                r.runs.insert(id, run);
            }
        }
        if finished {
            self.media.invalidate();
        }
        let r = &mut self.media.refresh;
        if r.runs.values().any(|x| x.running) {
            self.ctx.request_repaint_after(Duration::from_secs(1));
            if r.polled.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                r.polled = Some(Instant::now());
                let running: Vec<i64> = r.runs.values().filter(|x| x.running).map(|x| x.library_id).collect();
                for id in running {
                    if let std::collections::hash_map::Entry::Vacant(slot) = r.asking.entry(id) {
                        let c = client.clone();
                        let path = format!("/api/v1/libraries/{id}/refresh-metadata");
                        slot.insert(fetch(&self.rt, &self.ctx, async move { c.get(&path).await }));
                    }
                }
            }
        }
    }

    /// Under a library in Settings → Libraries: the running or last refresh.
    pub(super) fn library_refresh_status(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient, library: i64) {
        let r = &mut self.media.refresh;
        let stale = r.loaded.get(&library).is_none_or(|t| t.elapsed() >= Duration::from_secs(5));
        if stale && !r.asking.contains_key(&library) {
            r.loaded.insert(library, Instant::now());
            let c = client.clone();
            let path = format!("/api/v1/libraries/{library}/refresh-metadata");
            r.asking.insert(library, fetch(&self.rt, &self.ctx, async move { c.get(&path).await }));
        }
        self.ctx.request_repaint_after(Duration::from_secs(5));
        let r = &mut self.media.refresh;
        let Some(run) = r.runs.get(&library) else { return };
        for (color, text) in run_lines(run, p) {
            ui.label(RichText::new(text).small().color(color));
        }
    }
}

/// What a refresh of one item came to, as a toast.
fn item_outcome(o: &RefreshOutcome) -> (ToastKind, String) {
    if let Some(provider) = &o.provider {
        return (ToastKind::Success, t!("Details updated from {provider}.", provider = provider.as_str()));
    }
    if o.no_providers {
        return (
            ToastKind::Warning,
            t!("No metadata provider is set up. Add an API key in Settings → Metadata.").to_string(),
        );
    }
    if let Some(e) = o.errors.first() {
        return (
            ToastKind::Error,
            t!("{provider} failed: {error}", provider = e.provider.as_str(), error = e.error.as_str()),
        );
    }
    (ToastKind::Info, t!("No provider found a clear match. Use Identify… to choose one.").to_string())
}

/// A run's status lines (color, text), shortest first.
fn run_lines(run: &MetadataRun, p: &Palette) -> Vec<(egui::Color32, String)> {
    let mut out = Vec::new();
    if run.running {
        let text = if run.total == 0 {
            t!("Looking up metadata…").to_string()
        } else {
            t!("Looking up metadata… {done} of {total}", done = run.done, total = run.total)
        };
        out.push((p.text_dim, text));
    } else if run.finished_at.is_some() {
        let text = if run.total == 0 {
            t!("Metadata: nothing to look up.").to_string()
        } else {
            t!(
                "Metadata: {matched} identified, {unmatched} without a clear match.",
                matched = run.matched,
                unmatched = run.unmatched
            )
        };
        out.push((p.text_dim, text));
    }
    if run.no_providers {
        out.push((p.warn, t!("No metadata provider is set up. Add an API key in Settings → Metadata.").to_string()));
    }
    for e in &run.errors {
        out.push((p.error, t!("{provider} failed: {error}", provider = e.provider.as_str(), error = e.error.as_str())));
    }
    out
}
