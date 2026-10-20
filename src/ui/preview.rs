use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;
use ratatui_image::Image;

use super::{header, list, preview_body, put};
use crate::app::App;
use crate::format::{compact_size, count, list_date};
use crate::preview::Content;
use crate::theme::*;

/// Line numbers take this many columns, then a gap of two.
const LNO_WIDTH: u16 = 4;

pub fn render(app: &App, buf: &mut Buffer, area: Rect) {
    header(buf, area, &title(app), false);
    let body = preview_body(area);
    if body.height == 0 || body.width == 0 {
        return;
    }
    // Nothing yet means the worker is still on it.
    let Some(content) = app.preview_key.as_ref().and_then(|k| app.previewer.get(k)) else {
        return;
    };
    let dim = Style::default().fg(OVERLAY0);
    match content {
        Content::Folder(entries) => {
            let shown: Vec<_> = entries
                .iter()
                .filter(|e| app.show_hidden || !e.is_hidden())
                .take(body.height as usize)
                .collect();
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
                list::row(app, buf, r, e, false, false, false);
            }
        }
        Content::Text(lines) => {
            for (i, line) in lines.iter().take(body.height as usize).enumerate() {
                let y = body.y + i as u16;
                let lno = format!("{:>w$}", i + 1, w = LNO_WIDTH as usize);
                put(buf, body.x, y, &lno, LNO_WIDTH, dim);
                let x = body.x + LNO_WIDTH + 2;
                if x < body.right() {
                    buf.set_line(x, y, line, body.right() - x);
                }
            }
        }
        Content::Image(protocol) => Image::new(protocol).render(body, buf),
        Content::Info { label, size, mtime } => {
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
                ("no previewer".to_string(), Style::default().fg(SUBTEXT0)),
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
