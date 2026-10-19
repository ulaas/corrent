//! Drag & drop of .torrent files (and text files containing magnet links).

use egui::{Align2, Color32, Context, FontId, Id, LayerId, Order, vec2};
use egui_phosphor::regular as icons;

use super::CorrentApp;
use crate::engine::types::AddSource;
use crate::ui::theme::Palette;
use crate::ui::widgets::dashed_rect;

impl CorrentApp {
    pub(super) fn drag_and_drop(&mut self, ctx: &Context, p: &Palette) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let t = crate::ui::motion::toggle(ctx, Id::new("dnd_overlay"), hovering, crate::ui::motion::MICRO);
        if t > 0.0 {
            let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("dnd_layer")));
            let screen = ctx.content_rect();
            painter.rect_filled(screen, 0.0, p.bg0.gamma_multiply(0.85 * t));
            let inner = screen.shrink(24.0);
            painter.rect_filled(inner, egui::CornerRadius::same(18), p.accent.gamma_multiply(0.06 * t));
            dashed_rect(&painter, inner, p.accent.gamma_multiply(t), 2.0);
            let c = inner.center();
            painter.text(
                c - vec2(0.0, 30.0),
                Align2::CENTER_CENTER,
                icons::DOWNLOAD_SIMPLE,
                FontId::proportional(56.0),
                p.accent.gamma_multiply(t),
            );
            painter.text(
                c + vec2(0.0, 22.0),
                Align2::CENTER_CENTER,
                t!("Drop to add torrents"),
                crate::ui::theme::bold(20.0),
                p.text.gamma_multiply(t),
            );
            painter.text(
                c + vec2(0.0, 48.0),
                Align2::CENTER_CENTER,
                t!(".torrent files or text files with magnet links"),
                FontId::proportional(13.0),
                p.text_dim.gamma_multiply(t),
            );
            let _ = Color32::TRANSPARENT;
        }

        let dropped: Vec<egui::DroppedFileHandle> = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        // Read off the UI thread (a file on a sleeping disk or a share); `drain_background` adds
        // what was found.
        let paths: Vec<std::path::PathBuf> = dropped.iter().map(|f| f.path().to_path_buf()).collect();
        let (tx, ctx) = (self.dropped_tx.clone(), ctx.clone());
        self.rt.spawn_blocking(move || {
            let _ = tx.send(read_dropped(&paths));
            ctx.request_repaint();
        });
    }
}

/// What to add from dropped files, and how many weren't torrents (blocking file reads).
fn read_dropped(paths: &[std::path::PathBuf]) -> (Vec<AddSource>, usize) {
    let mut add = Vec::new();
    let mut ignored = 0;
    for path in paths {
        let is_torrent = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent"));
        if is_torrent && path.is_file() {
            add.push(AddSource::File(path.clone()));
            continue;
        }
        // Text files with magnet links / URLs, one per line.
        let small = std::fs::metadata(path).map(|m| m.len() < 512 * 1024).unwrap_or(false);
        if small && let Ok(text) = std::fs::read_to_string(path) {
            let sources: Vec<AddSource> =
                text.lines().filter_map(AddSource::parse).filter(|s| !matches!(s, AddSource::File(_))).collect();
            if !sources.is_empty() {
                add.extend(sources);
                continue;
            }
        }
        ignored += 1;
    }
    (add, ignored)
}

#[cfg(test)]
mod tests {
    #[test]
    fn dropped_files_are_sorted_into_adds_and_ignored() {
        let dir = std::env::temp_dir().join(format!("corrent-dnd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t = dir.join("a.torrent");
        std::fs::write(&t, b"d4:infoe").unwrap();
        let links = dir.join("links.txt");
        std::fs::write(&links, "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567\nnot a link\n").unwrap();
        let other = dir.join("photo.jpg");
        std::fs::write(&other, b"\xff\xd8").unwrap();
        let (add, ignored) = super::read_dropped(&[t, links, other]);
        assert_eq!(add.len(), 2);
        assert_eq!(ignored, 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
