use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::dialog::frame;
use super::{input_line, list::window_start, put};
use crate::app::App;
use crate::help::{HelpState, Row, rows};
use crate::theme::*;

const WIDTH: u16 = 56;
/// Key column, then the title.
const KEY_WIDTH: u16 = 9;
const FOOTER: &str = "i/k move · Enter run · or press the key itself · / filter · Esc close";

pub fn render(app: &App, buf: &mut Buffer, area: Rect, h: &HelpState) {
    let rows = rows(app, &h.filter.text);
    let filtering = h.typing || !h.filter.text.is_empty();
    // A filter line on top while there's a filter, and one row when empty.
    let top = filtering as u16;
    let want = top + rows.len().max(1) as u16;
    let max = area.height.saturating_sub(8).max(3);
    let inner = frame(
        buf,
        area,
        WIDTH,
        want.min(max),
        POPUP_BORDER,
        ("Keys", "most relevant first"),
        FOOTER,
    );
    if inner.height == 0 {
        return;
    }
    if filtering {
        put(
            buf,
            inner.x,
            inner.y,
            "/",
            1,
            Style::default().fg(POPUP_DIM),
        );
        let style = Style::default().fg(TEXT);
        if h.typing {
            input_line(
                buf,
                inner.x + 2,
                inner.y,
                inner.width.saturating_sub(2),
                &h.filter,
                style,
            );
        } else {
            put(
                buf,
                inner.x + 2,
                inner.y,
                &h.filter.text,
                inner.width.saturating_sub(2),
                style,
            );
        }
    }
    let list = Rect {
        y: inner.y + top,
        height: inner.height.saturating_sub(top),
        ..inner
    };
    if rows.is_empty() {
        put(
            buf,
            list.x,
            list.y,
            "nothing matches",
            list.width,
            Style::default().fg(POPUP_DIM),
        );
        return;
    }

    // The row index of each command, to keep the cursor in view.
    let cursor_row = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, Row::Command(_)))
        .nth(h.cursor)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let start = window_start(cursor_row, list.height as usize, rows.len());
    for (n, row) in rows
        .iter()
        .enumerate()
        .skip(start)
        .take(list.height as usize)
    {
        let r = Rect {
            y: list.y + (n - start) as u16,
            height: 1,
            ..list
        };
        match row {
            Row::Section(title) => {
                let style = Style::default().fg(PEACH).add_modifier(Modifier::BOLD);
                put(buf, r.x, r.y, title, r.width, style);
            }
            Row::Command(cmd) => {
                let on = n == cursor_row;
                let (key_fg, title_fg) = if on {
                    let bar = Rect {
                        x: r.x.saturating_sub(1),
                        width: r.width + 2,
                        ..r
                    };
                    buf.set_style(bar, Style::default().bg(ACCENT));
                    (ON_ACCENT, ON_ACCENT)
                } else {
                    (ACCENT, TEXT)
                };
                let key = app
                    .keymap
                    .first_key(*cmd)
                    .map(|k| k.to_string())
                    .unwrap_or_default();
                put(
                    buf,
                    r.x + 1,
                    r.y,
                    &key,
                    KEY_WIDTH - 2,
                    Style::default().fg(key_fg),
                );
                let title_x = r.x + KEY_WIDTH;
                put(
                    buf,
                    title_x,
                    r.y,
                    &app.action_title(*cmd),
                    r.width.saturating_sub(KEY_WIDTH),
                    Style::default().fg(title_fg),
                );
            }
        }
    }
}
