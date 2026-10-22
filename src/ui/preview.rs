use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;
use ratatui_image::Image;

use super::{header, list, preview_body, put};
use crate::app::App;
use crate::format::{compact_size, count, list_date};
use crate::modal::Modal;
use crate::preview::{Content, facts_rows};
use crate::sort::{Sort, compare};
use crate::theme::*;

/// Line numbers take this many columns, then a gap of two.
const LNO_WIDTH: u16 = 4;
/// Label column for the facts under a media preview.
const FACT_LABEL: u16 = 10;

pub fn render(app: &App, buf: &mut Buffer, area: Rect) {
    header(buf, area, &title(app), false);
    // An open picker or search borrows the preview for its own result.
    if !matches!(app.modal, Some(Modal::Picker(_) | Modal::Search(_))) {
        body(app, buf, preview_body(area));
    }
}

/// Whatever the worker built for `app.preview_key`, drawn into `body`.
pub(super) fn body(app: &App, buf: &mut Buffer, body: Rect) {
    if body.height == 0 || body.width == 0 {
        return;
    }
    // Nothing yet means the worker is still on it.
    let Some(key) = &app.preview_key else { return };
    let Some(content) = app.previewer.get(key) else {
        return;
    };
    let dim = Style::default().fg(OVERLAY0);
    match content {
        Content::Folder(entries) => {
            let mut shown: Vec<_> = entries
                .iter()
                .filter(|e| app.show_hidden || !e.is_hidden())
                .collect();
            // The worker sorts by name; follow the list's order instead.
            if app.sort != Sort::default() {
                shown.sort_by(|a, b| compare(a, b, app.sort));
            }
            shown.truncate(body.height as usize);
            if shown.is_empty() {
                put(buf, body.x + 1, body.y, "empty folder", body.width - 1, dim);
            }
            // List rows bring their own selection-marker gutter, which a
            // preview doesn't need.
            let rows = Rect {
                x: body.x - 2,
                width: body.width + 2,
                ..body
            };
            for (i, e) in shown.into_iter().enumerate() {
                let r = Rect {
                    y: rows.y + i as u16,
                    height: 1,
                    ..rows
                };
                list::row(app, buf, r, &key.path, e, false, false, false);
            }
        }
        Content::Text {
            first,
            focus,
            lines,
        } => {
            // Wide enough for the last number shown.
            let digits = (first + lines.len() as u64).to_string().len() as u16;
            let lno_width = digits.max(LNO_WIDTH);
            for (i, line) in lines.iter().take(body.height as usize).enumerate() {
                let y = body.y + i as u16;
                let n = first + i as u64;
                if Some(n) == *focus {
                    let row = Rect {
                        y,
                        height: 1,
                        ..body
                    };
                    buf.set_style(row, Style::default().bg(SURFACE0));
                }
                let lno = format!("{:>w$}", n, w = lno_width as usize);
                put(buf, body.x, y, &lno, lno_width, dim);
                let x = body.x + lno_width + 2;
                if x < body.right() {
                    buf.set_line(x, y, line, body.right() - x);
                }
            }
        }
        Content::Image(protocol) => Image::new(protocol).render(body, buf),
        Content::Media { image, facts } => {
            let facts_h = facts_rows(facts.len()).min(body.height);
            let top = Rect {
                height: body.height - facts_h,
                ..body
            };
            if let Some(protocol) = image {
                Image::new(protocol).render(top, buf);
            }
            // Under the picture, or at the top when there's none.
            let first = if image.is_some() {
                top.bottom() + 1
            } else {
                body.y
            };
            for (i, (label, value)) in facts.iter().enumerate() {
                let y = first + i as u16;
                if y >= body.bottom() {
                    break;
                }
                put(buf, body.x + 1, y, label, FACT_LABEL - 1, dim);
                let x = body.x + FACT_LABEL;
                let w = body.right().saturating_sub(x);
                put(buf, x, y, value, w, Style::default().fg(TEXT));
            }
        }
        Content::Archive { entries, more } => {
            let rows = body.height as usize - usize::from(*more);
            for (i, e) in entries.iter().take(rows).enumerate() {
                let fg = if e.ends_with('/') { BLUE } else { TEXT };
                let y = body.y + i as u16;
                put(
                    buf,
                    body.x + 1,
                    y,
                    e,
                    body.width - 1,
                    Style::default().fg(fg),
                );
            }
            if *more {
                let y = body.y + entries.len().min(rows) as u16;
                put(buf, body.x + 1, y, "…and more", body.width - 1, dim);
            }
        }
        Content::Info {
            label,
            size,
            mtime,
            note,
        } => {
            let date = mtime
                .map(|t| list_date(t, &app.tz, app.this_year))
                .unwrap_or_default();
            let lines = [
                (
                    label.to_string(),
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
                (
                    format!("{} · modified {date}", compact_size(*size)),
                    Style::default().fg(SUBTEXT0),
                ),
                (String::new(), dim),
                (note.clone(), Style::default().fg(SUBTEXT0)),
            ];
            for (i, (text, style)) in lines.iter().take(body.height as usize).enumerate() {
                put(
                    buf,
                    body.x + 1,
                    body.y + i as u16,
                    text,
                    body.width - 1,
                    *style,
                );
            }
        }
        Content::Error(e) => {
            put(
                buf,
                body.x + 1,
                body.y,
                e,
                body.width - 1,
                Style::default().fg(RED),
            );
        }
    }
}

fn title(app: &App) -> String {
    match app.current() {
        None => "preview".to_string(),
        Some(e) => match &e.link_target {
            Some(target) => format!("{} → {}", e.name, app.display_path(target)),
            None if e.is_dir() => {
                let items = e.children.map(|n| count(n, "item", "items"));
                format!("{}/ · {}", e.name, items.unwrap_or_default())
            }
            None => format!("{} · {}", e.name, compact_size(e.size)),
        },
    }
}
