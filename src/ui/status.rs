use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{put, width};
use crate::app::{App, Focus};
use crate::format::count;
use crate::theme::*;

pub fn render(app: &App, buf: &mut Buffer, area: Rect) {
    buf.set_style(area, Style::default().bg(MANTLE));

    let mut right = vec![count(app.visible.len(), "item", "items")];
    if app.show_hidden {
        right.push("hidden shown".to_string());
    }
    right.push("? help".to_string());
    let right = right.join(" · ");
    let right_w = width(&right).min(area.width);
    let right_x = area.right().saturating_sub(right_w + 1);
    put(
        buf,
        right_x,
        area.y,
        &right,
        right_w,
        Style::default().fg(OVERLAY0),
    );

    let chip = Style::default()
        .bg(ACCENT)
        .fg(ON_ACCENT)
        .add_modifier(Modifier::BOLD);
    let mode = match app.focus {
        Focus::Files => " FILES ",
        Focus::Sidebar => " SHORTCUTS ",
    };
    let mut x = area.x;
    let mut room = right_x.saturating_sub(area.x + 2);
    for (text, style) in [
        (mode.to_string(), chip),
        (app.display_path(&app.dir), Style::default().fg(TEXT)),
        (
            app.current().map(|e| e.name.clone()).unwrap_or_default(),
            Style::default().fg(OVERLAY0),
        ),
    ] {
        let used = put(buf, x, area.y, &text, room, style);
        let step = (used + 2).min(room);
        x += step;
        room -= step;
    }
}
