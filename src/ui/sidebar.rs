use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{body, header, list::window_start, put};
use crate::app::{App, Focus};
use crate::theme::*;

enum Row {
    Shortcut(usize),
    /// Between groups: XDG folders, pinned folders, network hosts.
    Divider,
}

pub fn render(app: &App, buf: &mut Buffer, area: Rect) {
    buf.set_style(area, Style::default().bg(MANTLE));
    let focused = app.focus == Focus::Sidebar;
    header(buf, area, "shortcuts", focused);
    let body = body(area);
    if body.height == 0 {
        return;
    }

    let items = &app.shortcuts.items;
    let mut rows = Vec::new();
    for (i, s) in items.iter().enumerate() {
        if i > 0 && items[i - 1].group != s.group {
            rows.push(Row::Divider);
        }
        rows.push(Row::Shortcut(i));
    }
    let cursor_row = rows
        .iter()
        .position(|r| matches!(r, Row::Shortcut(i) if *i == app.side_cursor))
        .unwrap_or(0);
    let start = window_start(cursor_row, body.height as usize, rows.len());
    let here = app.here();

    for (n, row) in rows
        .iter()
        .skip(start)
        .take(body.height as usize)
        .enumerate()
    {
        let r = Rect {
            y: body.y + n as u16,
            height: 1,
            ..body
        };
        let i = match *row {
            Row::Divider => {
                let line = "─".repeat(r.width as usize);
                put(buf, r.x, r.y, &line, r.width, Style::default().fg(SURFACE0));
                continue;
            }
            Row::Shortcut(i) => i,
        };
        let (icon, label) = if focused && i == app.side_cursor {
            buf.set_style(r, Style::default().bg(ACCENT));
            (ON_ACCENT, ON_ACCENT)
        } else if here == Some(i) {
            (ACCENT, ACCENT)
        } else if items[i].host.is_some() {
            (TEAL, TEAL)
        } else {
            (OVERLAY0, TEXT)
        };
        let s = &items[i];
        let label_x = if app.icons {
            let mut glyph = [0; 4];
            let glyph = s.icon.encode_utf8(&mut glyph);
            put(buf, r.x + 1, r.y, glyph, 1, Style::default().fg(icon));
            r.x + 3
        } else {
            r.x + 1
        };
        let w = r.right().saturating_sub(label_x + 1);
        put(buf, label_x, r.y, &s.label, w, Style::default().fg(label));
    }
}
