use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::dialog::frame;
use super::{put, width};
use crate::props::Props;
use crate::theme::*;

/// Label column, then the value.
const LABEL_WIDTH: u16 = 13;
/// Wide enough that the box doesn't jump when a count finishes.
const MIN_WIDTH: u16 = 52;

pub fn render(buf: &mut Buffer, area: Rect, p: &Props) {
    let rows = p.rows();
    let mut names: Vec<String> = p.names.clone();
    if p.more > 0 {
        names.push(format!("…and {} more", p.more));
    }
    let widest = rows
        .iter()
        .map(|r| LABEL_WIDTH + width(&r.value))
        .chain(names.iter().map(|n| width(n)))
        .max()
        .unwrap_or(0)
        .max(MIN_WIDTH);
    // The names sit under the fields after a blank row.
    let height = rows.len() + if names.is_empty() { 0 } else { names.len() + 1 };
    let footer = if p.counting() {
        "Esc close · sizes are counted in the background"
    } else {
        "Esc close"
    };
    let inner = frame(
        buf,
        area,
        widest,
        height as u16,
        POPUP_BORDER,
        ("Properties", &p.title),
        footer,
    );
    let label = Style::default().fg(POPUP_DIM);
    let mut y = inner.y;
    for r in &rows {
        if y >= inner.bottom() {
            return;
        }
        put(buf, inner.x, y, r.label, LABEL_WIDTH, label);
        let fg = if r.pending { POPUP_DIM } else { TEXT };
        let room = inner.width.saturating_sub(LABEL_WIDTH);
        put(
            buf,
            inner.x + LABEL_WIDTH,
            y,
            &r.value,
            room,
            Style::default().fg(fg),
        );
        y += 1;
    }
    y += 1;
    for name in &names {
        if y >= inner.bottom() {
            return;
        }
        put(
            buf,
            inner.x,
            y,
            name,
            inner.width,
            Style::default().fg(TEXT),
        );
        y += 1;
    }
}
