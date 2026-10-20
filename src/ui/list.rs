use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{body, header, put, width};
use crate::app::{App, Focus};
use crate::entry::Entry;
use crate::format::{compact_size, count, list_date};
use crate::icons;
use crate::theme::*;

pub fn render(app: &App, buf: &mut Buffer, area: Rect) {
    let focused = app.focus == Focus::Files;
    header(buf, area, &app.display_path(&app.dir), focused);
    let body = body(area);
    if body.height == 0 {
        return;
    }
    if app.visible.is_empty() {
        let style = Style::default().fg(OVERLAY0);
        let text = if app.filter.text.is_empty() {
            "empty folder".to_string()
        } else {
            format!("no match for “{}”", app.filter.text)
        };
        put(
            buf,
            body.x + 2,
            body.y,
            &text,
            body.width.saturating_sub(2),
            style,
        );
        return;
    }
    let rows = body.height as usize;
    let start = window_start(app.cursor, rows, app.visible.len());
    for (i, &ei) in app.visible.iter().enumerate().skip(start).take(rows) {
        let r = Rect {
            y: body.y + (i - start) as u16,
            height: 1,
            ..body
        };
        let e = &app.entries[ei];
        row(app, buf, r, e, i == app.cursor, focused, app.is_selected(e));
    }
    if !app.selection.is_empty() {
        summary_box(buf, body, &app.selection_summary());
    }
}

/// The selection summary in the bottom-right corner of the list.
fn summary_box(buf: &mut Buffer, body: Rect, text: &str) {
    let w = (width(text) + 2).min(body.width.saturating_sub(2));
    if w < 3 {
        return;
    }
    let r = Rect {
        x: body.right() - 1 - w,
        y: body.bottom() - 1,
        width: w,
        height: 1,
    };
    buf.set_style(r, Style::default().bg(SURFACE1));
    put(buf, r.x + 1, r.y, text, w - 2, Style::default().fg(TEXT));
}

/// Keeps the cursor row near the middle, like the prototype.
pub(super) fn window_start(cursor: usize, rows: usize, len: usize) -> usize {
    if len <= rows {
        0
    } else {
        cursor.saturating_sub(rows / 2).min(len - rows)
    }
}

pub(super) fn row(
    app: &App,
    buf: &mut Buffer,
    r: Rect,
    e: &Entry,
    is_cursor: bool,
    focused: bool,
    selected: bool,
) {
    let on_accent = is_cursor && focused;
    if is_cursor {
        let bg = if focused { ACCENT } else { SURFACE0 };
        buf.set_style(r, Style::default().bg(bg));
    }
    let ink = |c| Style::default().fg(if on_accent { ON_ACCENT } else { c });

    // Columns: the selection marker, the icon, the name, then metadata
    // right-aligned with a 2-column gap and 1 column of padding.
    let (name, color) = if e.is_link() {
        (e.name.clone(), YELLOW)
    } else if e.is_dir() {
        (format!("{}/", e.name), BLUE)
    } else {
        (e.name.clone(), TEXT)
    };
    let color = if selected { MAUVE } else { color };
    if selected {
        put(buf, r.x + 1, r.y, "●", 1, ink(MAUVE));
    }
    let mut icon = [0; 4];
    let icon = icons::for_entry(e).encode_utf8(&mut icon);
    put(buf, r.x + 3, r.y, icon, 1, ink(color));
    let name_x = r.x + 5;
    let right = r.right().saturating_sub(1);
    let meta = meta(app, e);
    let meta_w = width(&meta);
    let room = right.saturating_sub(name_x);
    // Metadata only when the name keeps a fair share of the row.
    if meta_w + 2 + width(&name).min(12) <= room {
        let meta_x = right - meta_w;
        put(buf, name_x, r.y, &name, meta_x - 2 - name_x, ink(color));
        put(buf, meta_x, r.y, &meta, meta_w, ink(OVERLAY0));
    } else {
        put(buf, name_x, r.y, &name, room, ink(color));
    }
}

fn meta(app: &App, e: &Entry) -> String {
    if let Some(target) = &e.link_target {
        let slash = if e.is_dir() { "/" } else { "" };
        format!("→ {}{slash}", app.display_path(target))
    } else if e.is_dir() {
        e.children
            .map(|n| count(n, "item", "items"))
            .unwrap_or_default()
    } else {
        let date = e
            .mtime
            .map(|t| list_date(t, &app.tz, app.this_year))
            .unwrap_or_default();
        format!("{}  {date}", compact_size(e.size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_keeps_cursor_centred() {
        assert_eq!(window_start(3, 10, 5), 0);
        assert_eq!(window_start(0, 10, 100), 0);
        assert_eq!(window_start(4, 10, 100), 0);
        assert_eq!(window_start(50, 10, 100), 45);
        assert_eq!(window_start(99, 10, 100), 90);
    }
}
