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
/// Space between a dialog's border and its contents.
const PAD_X: u16 = 2;
const PAD_Y: u16 = 1;

pub fn render(app: &App, buf: &mut Buffer, area: Rect) {
    match &app.modal {
        Some(Modal::Confirm { delete, paths }) => {
            let shown: Vec<String> = paths.iter().map(|p| app.display_path(p)).collect();
            confirm(buf, area, *delete, &shown);
        }
        Some(Modal::Rename { olds, input }) => {
            let inner = if olds.len() == 1 {
                let titles = ("Rename", olds[0].as_str());
                frame(
                    buf,
                    area,
                    60,
                    1,
                    POPUP_BORDER,
                    titles,
                    "Enter apply · Esc cancel",
                )
            } else {
                bulk(app, buf, area, olds.len())
            };
            if inner.height > 0 {
                input_line(
                    buf,
                    inner.x,
                    inner.y,
                    inner.width,
                    input,
                    Style::default().fg(TEXT),
                );
                if olds.len() > 1 && input.text.is_empty() {
                    let hint = "e.g. show.s01.e[01].mkv";
                    put(
                        buf,
                        inner.x + 2,
                        inner.y,
                        hint,
                        inner.width.saturating_sub(2),
                        Style::default().fg(POPUP_DIM),
                    );
                }
            }
        }
        Some(Modal::Create { folder, input }) => {
            let title = if *folder { "New folder" } else { "New file" };
            let here = app.display_path(&app.dir);
            let inner = frame(
                buf,
                area,
                60,
                1,
                POPUP_BORDER,
                (title, &here),
                "Enter create · Esc cancel",
            );
            if inner.height > 0 {
                let style = Style::default().fg(TEXT);
                input_line(buf, inner.x, inner.y, inner.width, input, style);
            }
        }
        Some(Modal::Help(h)) => super::help::render(app, buf, area, h),
        Some(Modal::Picker(p)) => super::picker::render(app, buf, area, p),
        Some(Modal::Search(s)) => super::picker::render_search(app, buf, area, s),
        Some(Modal::Props(p)) => super::props::render(buf, area, p),
        None => {}
    }
}

fn confirm(buf: &mut Buffer, area: Rect, delete: bool, paths: &[String]) {
    let (title, verb, border) = if delete {
        ("Delete permanently", "delete", RED)
    } else {
        ("Move to trash", "trash", POPUP_BORDER)
    };
    let mut lines: Vec<(String, Color)> = paths
        .iter()
        .take(MAX_LISTED)
        .map(|p| (p.clone(), TEXT))
        .collect();
    if paths.len() > MAX_LISTED {
        lines.push((format!("…and {} more", paths.len() - MAX_LISTED), TEXT));
    }
    if delete {
        lines.push((String::new(), TEXT));
        lines.push(("This skips the trash and can't be undone.".into(), RED));
    }
    let widest = lines.iter().map(|(l, _)| width(l)).max().unwrap_or(0);
    let right = count(paths.len(), "item", "items");
    let foot = format!("y {verb} · n or Esc cancel");
    let inner = frame(
        buf,
        area,
        widest,
        lines.len() as u16,
        border,
        (title, &right),
        &foot,
    );
    for (i, (text, fg)) in lines.iter().enumerate().take(inner.height as usize) {
        put(
            buf,
            inner.x,
            inner.y + i as u16,
            text,
            inner.width,
            Style::default().fg(*fg),
        );
    }
}

/// The bulk rename frame and its old → new preview; returns the inner area.
fn bulk(app: &App, buf: &mut Buffer, area: Rect, n: usize) -> Rect {
    let rows = app.bulk_rows();
    let listed = rows.len().min(MAX_ROWS);
    let more = rows.len() > MAX_ROWS;
    let height = 1 + 1 + listed as u16 + more as u16;
    let foot = "[01] counter · {name} old name · {ext} extension · Enter apply · Esc cancel";
    let files = count(n, "file", "files");
    let inner = frame(
        buf,
        area,
        76,
        height,
        POPUP_BORDER,
        ("Bulk rename", &files),
        foot,
    );
    let half = inner.width.saturating_sub(3) / 2;
    for (i, row) in rows.iter().take(listed).enumerate() {
        let y = inner.y + 2 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let x = inner.x;
        put(buf, x, y, &row.old, half, Style::default().fg(TEXT));
        put(buf, x + half + 1, y, "→", 1, Style::default().fg(POPUP_DIM));
        let (new, fg) = match row.problem {
            Some(_) => (format!("{} ✗", row.new), RED),
            None => (row.new.clone(), TEXT),
        };
        put(buf, x + half + 3, y, &new, half, Style::default().fg(fg));
    }
    if more && inner.y + 2 + listed as u16 <= inner.bottom() {
        let text = format!("…and {} more", rows.len() - MAX_ROWS);
        put(
            buf,
            inner.x,
            inner.y + 2 + listed as u16,
            &text,
            inner.width,
            Style::default().fg(TEXT),
        );
    }
    inner
}

/// Clears a centred box for `w` × `h` of content, draws its border, titles
/// (left, right) and footer, and returns the content area.
pub(super) fn frame(
    buf: &mut Buffer,
    area: Rect,
    w: u16,
    h: u16,
    border: Color,
    (title, right): (&str, &str),
    footer: &str,
) -> Rect {
    // Border, padding, content, a blank row, the footer, padding, border.
    let w = (w.max(width(footer)) + 2 * PAD_X + 2).min(area.width.saturating_sub(4));
    let h = (h + 2 * PAD_Y + 4).min(area.height);
    let r = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 3,
        width: w,
        height: h,
    };
    Clear.render(r, buf);
    let block = Block::bordered()
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(POPUP_BG).fg(TEXT))
        .title(Line::from(format!(" {title} ")).left_aligned())
        .title(
            Line::from(format!(" {right} "))
                .right_aligned()
                .style(Style::default().fg(POPUP_DIM)),
        );
    let inner = block.inner(r);
    block.render(r, buf);
    let padded = Rect {
        x: inner.x + PAD_X,
        y: inner.y + PAD_Y,
        width: inner.width.saturating_sub(2 * PAD_X),
        height: inner.height.saturating_sub(2 * PAD_Y),
    };
    if padded.height > 0 && padded.width > 0 {
        let style = Style::default().fg(POPUP_DIM);
        let y = padded.bottom() - 1;
        put(buf, padded.x, y, footer, padded.width, style);
    }
    Rect {
        height: padded.height.saturating_sub(2),
        ..padded
    }
}
