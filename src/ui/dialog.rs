//! Centred overlays: the trash/delete confirmation and rename.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Widget};

use super::{input_line, put, width};
use crate::app::App;
use crate::format::count;
use crate::modal::Modal;
use crate::theme::*;

/// Paths listed in a confirmation, and rows in the bulk rename preview.
const MAX_LISTED: usize = 5;
const MAX_ROWS: usize = 8;

pub fn render(app: &App, buf: &mut Buffer, area: Rect) {
    match &app.modal {
        Some(Modal::Confirm { delete, paths }) => {
            let shown: Vec<String> = paths.iter().map(|p| app.display_path(p)).collect();
            confirm(buf, area, *delete, &shown);
        }
        Some(Modal::Rename { olds, input }) => {
            let inner = if olds.len() == 1 {
                let b = frame(buf, area, 60, 3, SURFACE1, "Rename", &olds[0]);
                footer(buf, b, "Enter apply · Esc cancel");
                b
            } else {
                bulk(app, buf, area, olds.len())
            };
            if inner.height > 0 {
                input_line(buf, inner.x + 1, inner.y, inner.width.saturating_sub(2), input, Style::default().fg(TEXT));
                if olds.len() > 1 && input.text.is_empty() {
                    let hint = "e.g. show.s01.e[01].mkv";
                    put(buf, inner.x + 3, inner.y, hint, inner.width.saturating_sub(4), Style::default().fg(OVERLAY0));
                }
            }
        }
        None => {}
    }
}

fn confirm(buf: &mut Buffer, area: Rect, delete: bool, paths: &[String]) {
    let (title, verb, border) = if delete {
        ("Delete permanently", "delete", RED)
    } else {
        ("Move to trash", "trash", SURFACE1)
    };
    let mut lines: Vec<(String, Color)> = paths
        .iter()
        .take(MAX_LISTED)
        .map(|p| (p.clone(), TEXT))
        .collect();
    if paths.len() > MAX_LISTED {
        lines.push((format!("…and {} more", paths.len() - MAX_LISTED), SUBTEXT0));
    }
    if delete {
        lines.push(("This skips the trash and can't be undone.".into(), RED));
    }
    let widest = lines.iter().map(|(l, _)| width(l)).max().unwrap_or(0);
    let right = count(paths.len(), "item", "items");
    let inner = frame(buf, area, widest + 4, lines.len() as u16 + 2, border, title, &right);
    for (i, (text, fg)) in lines.iter().enumerate().take(inner.height as usize) {
        put(buf, inner.x + 1, inner.y + i as u16, text, inner.width.saturating_sub(2), Style::default().fg(*fg));
    }
    footer(buf, inner, &format!("y {verb} · n or Esc cancel"));
}

/// The bulk rename frame and its old → new preview; returns the inner area.
fn bulk(app: &App, buf: &mut Buffer, area: Rect, n: usize) -> Rect {
    let rows = app.bulk_rows();
    let listed = rows.len().min(MAX_ROWS);
    let more = rows.len() > MAX_ROWS;
    let height = 1 + 1 + listed as u16 + more as u16 + 2;
    let inner = frame(buf, area, 72, height, SURFACE1, "Bulk rename", &count(n, "file", "files"));
    let half = inner.width.saturating_sub(7) / 2;
    for (i, row) in rows.iter().take(listed).enumerate() {
        let y = inner.y + 2 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let x = inner.x + 1;
        put(buf, x, y, &row.old, half, Style::default().fg(SUBTEXT0));
        put(buf, x + half + 1, y, "→", 1, Style::default().fg(OVERLAY0));
        let (new, fg) = match row.problem {
            Some(_) => (format!("{} ✗", row.new), RED),
            None => (row.new.clone(), TEXT),
        };
        put(buf, x + half + 3, y, &new, half, Style::default().fg(fg));
    }
    if more && inner.y + 2 + listed as u16 <= inner.bottom() {
        let text = format!("…and {} more", rows.len() - MAX_ROWS);
        put(buf, inner.x + 1, inner.y + 2 + listed as u16, &text, inner.width, Style::default().fg(SUBTEXT0));
    }
    footer(buf, inner, "[01] counter · {name} old name · {ext} extension · Enter apply · Esc cancel");
    inner
}

/// Clears a centred box of `w` × `h` content rows plus a footer row, draws
/// its border and titles, and returns the content area above the footer.
fn frame(buf: &mut Buffer, area: Rect, w: u16, h: u16, border: Color, title: &str, right: &str) -> Rect {
    let w = (w + 2).min(area.width.saturating_sub(4)).max(20.min(area.width));
    let h = (h + 3).min(area.height);
    let r = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 3,
        width: w,
        height: h,
    };
    Clear.render(r, buf);
    let block = Block::bordered()
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(MANTLE).fg(TEXT))
        .title(Line::from(format!(" {title} ")).left_aligned())
        .title(Line::from(format!(" {right} ")).right_aligned().style(Style::default().fg(OVERLAY0)));
    let inner = block.inner(r);
    block.render(r, buf);
    Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    }
}

fn footer(buf: &mut Buffer, content: Rect, text: &str) {
    if content.width > 2 {
        put(buf, content.x + 1, content.bottom(), text, content.width - 2, Style::default().fg(OVERLAY0));
    }
}
