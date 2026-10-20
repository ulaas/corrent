//! Media → AI agent: how to connect an AI agent (MCP) to Corrent, and the changes agents
//! propose, each approved or rejected here.

use std::time::{Duration, Instant};

use corrent_proto::agent::Proposal;
use egui::{RichText, Ui};
use egui_phosphor::regular as icons;

use super::client::{MediaClient, Slot, fetch, ready};
use crate::app::CorrentApp;
use crate::ui::theme::Palette;
use crate::ui::widgets::{card, primary_button, secondary_button};

#[derive(Default)]
pub struct AgentUi {
    list: Option<Slot<Vec<Proposal>>>,
    fetched: Option<Instant>,
    shown: Vec<Proposal>,
    deciding: Option<(i64, Slot<Proposal>)>,
}

impl AgentUi {
    /// Proposals waiting for a decision (the sidebar's count).
    pub fn pending(&self) -> usize {
        self.shown.iter().filter(|p| p.status == "pending").count()
    }
}

impl CorrentApp {
    /// Keeps the proposals fresh: every few seconds on the page, every half minute elsewhere.
    pub(super) fn agent_tick(&mut self, client: &MediaClient, on_page: bool) {
        let a = &mut self.media.agent;
        if let Some(r) = a.list.as_ref().and_then(ready) {
            a.list = None;
            if let Ok(list) = r {
                a.shown = list;
            }
        }
        let every = if on_page { Duration::from_secs(3) } else { Duration::from_secs(30) };
        if a.list.is_none() && a.fetched.is_none_or(|t| t.elapsed() > every) {
            a.fetched = Some(Instant::now());
            let c = client.clone();
            a.list = Some(fetch(&self.rt, &self.ctx, async move { c.get("/api/v1/agent/proposals").await }));
        }
        if let Some((_, slot)) = &a.deciding
            && let Some(r) = ready(slot)
        {
            a.deciding = None;
            a.fetched = None;
            match r {
                Ok(p) if p.status == "failed" => self.toasts.push(
                    crate::engine::ToastKind::Error,
                    t!("Couldn't do it: {why}", why = p.result.unwrap_or_default()),
                ),
                Ok(p) if p.status == "applied" => {
                    self.toasts.push(crate::engine::ToastKind::Success, t!("Done.").to_string());
                    self.media.invalidate();
                }
                Ok(_) => {}
                Err(e) => self.toasts.push(crate::engine::ToastKind::Error, e),
            }
        }
    }

    pub(super) fn agent_page(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient) {
        self.agent_tick(client, true);
        ui.add_space(4.0);
        let side = 20.0;
        ui.horizontal(|ui| {
            ui.add_space(side);
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - side).max(200.0));
                self.agent_connect_card(ui, p);
                ui.add_space(14.0);
                self.agent_proposals(ui, p, client);
            });
        });
    }

    fn agent_connect_card(&mut self, ui: &mut Ui, p: &Palette) {
        let exe = std::env::current_exe().map(|e| e.display().to_string()).unwrap_or_else(|_| "corrent".into());
        let portable = self.paths.portable;
        let mut args = vec!["--mcp".to_string()];
        if portable {
            args.insert(0, "--portable".into());
        }
        let claude_code = format!("claude mcp add corrent -- \"{exe}\" {}", args.join(" "));
        let desktop = serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": { "corrent": { "command": exe, "args": args } }
        }))
        .unwrap_or_default();
        card(p).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("{}  {}", icons::SPARKLE, t!("Connect an AI agent"))).strong().size(16.0));
            ui.label(
                RichText::new(t!(
                    "AI agents (Claude Code, Claude Desktop and other MCP clients) can look through your libraries and downloads and propose changes: identify movies, fix titles, put downloads into libraries, pick thumbnails. Nothing changes until you approve it below, and nothing is ever deleted."
                ))
                .color(p.text_dim),
            );
            if self.server.is_some() {
                ui.label(
                    RichText::new(t!("Agents connect to the Corrent on this computer, not to the server you are viewing."))
                        .small()
                        .color(p.warn),
                );
            }
            ui.add_space(8.0);
            let snippet = |ui: &mut Ui, title: &str, text: &str| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).strong().color(p.text));
                    if ui.small_button(format!("{}  {}", icons::COPY, t!("Copy"))).clicked() {
                        ui.ctx().copy_text(text.to_string());
                    }
                });
                egui::Frame::new().fill(p.bg1).corner_radius(6).inner_margin(8).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(RichText::new(text).monospace().small().color(p.text)).wrap());
                });
                ui.add_space(6.0);
            };
            snippet(ui, t!("Claude Code (a terminal)"), &claude_code);
            snippet(ui, t!("Claude Desktop and others (their MCP settings)"), &desktop);
            ui.label(RichText::new(t!("Corrent has to be running while the agent works.")).small().color(p.text_mute));
        });
    }

    fn agent_proposals(&mut self, ui: &mut Ui, p: &Palette, client: &MediaClient) {
        let shown = self.media.agent.shown.clone();
        let pending: Vec<&Proposal> = shown.iter().filter(|x| x.status == "pending").collect();
        let mut decide: Option<(i64, bool)> = None;
        ui.label(RichText::new(t!("Waiting for you")).size(17.0).strong().color(p.text));
        ui.add_space(6.0);
        if pending.is_empty() {
            ui.label(RichText::new(t!("Nothing to decide. Proposals from agents show up here.")).color(p.text_dim));
        }
        let busy = self.media.agent.deciding.as_ref().map(|(id, _)| *id);
        for pr in &pending {
            card(p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(RichText::new(&pr.summary).color(p.text)).wrap());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::util::fmt_date(pr.created_at)).small().color(p.text_mute));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if busy == Some(pr.id) {
                            ui.spinner();
                        } else if busy.is_none() {
                            if primary_button(ui, p, &format!("{}  {}", icons::CHECK, t!("Approve"))).clicked() {
                                decide = Some((pr.id, true));
                            }
                            if secondary_button(ui, t!("Reject")).clicked() {
                                decide = Some((pr.id, false));
                            }
                        }
                    });
                });
            });
            ui.add_space(6.0);
        }
        let decided: Vec<&Proposal> = shown.iter().filter(|x| x.status != "pending").take(50).collect();
        if !decided.is_empty() {
            ui.add_space(12.0);
            ui.label(RichText::new(t!("Earlier")).size(17.0).strong().color(p.text));
            ui.add_space(6.0);
            for pr in decided {
                ui.horizontal(|ui| {
                    let (label, color) = match pr.status.as_str() {
                        "applied" => (t!("Done"), p.success),
                        "failed" => (t!("Failed"), p.error),
                        _ => (t!("Rejected"), p.text_mute),
                    };
                    ui.label(RichText::new(label).small().color(color));
                    ui.add(egui::Label::new(RichText::new(&pr.summary).color(p.text_dim)).truncate()).on_hover_text(
                        pr.result.clone().filter(|r| !r.is_empty()).unwrap_or_else(|| pr.summary.clone()),
                    );
                });
            }
        }
        if let Some((id, approve)) = decide {
            let c = client.clone();
            let path = format!("/api/v1/agent/proposals/{id}/{}", if approve { "approve" } else { "reject" });
            self.media.agent.deciding = Some((
                id,
                fetch(&self.rt, &self.ctx, async move {
                    c.send(reqwest::Method::POST, &path, &serde_json::json!({})).await
                }),
            ));
        }
    }
}
