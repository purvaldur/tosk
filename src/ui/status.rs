use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::{input_line, put, width};
use crate::app::{App, Focus};
use crate::format::count;
use crate::sort::Sort;
use crate::theme::*;

pub fn render(app: &App, buf: &mut Buffer, area: Rect) {
    buf.set_style(area, Style::default().bg(MANTLE));

    let mut right = vec![count(app.visible.len(), "item", "items")];
    if app.sort != Sort::default() {
        right.push(format!("by {}", app.sort.label()));
    }
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

    let chip = |bg| {
        Style::default()
            .bg(bg)
            .fg(ON_ACCENT)
            .add_modifier(Modifier::BOLD)
    };
    let room = right_x.saturating_sub(area.x + 2);

    // While a filter is set, it replaces the path: chip, then the query.
    if app.filtering || !app.filter.text.is_empty() {
        let used = put(buf, area.x, area.y, " FILTER ", room, chip(MAUVE));
        let x = area.x + used + 1;
        let w = room.saturating_sub(used + 1);
        let style = Style::default().fg(TEXT);
        if app.filtering {
            input_line(buf, x, area.y, w, &app.filter, style);
        } else {
            put(buf, x, area.y, &app.filter.text, w, style);
        }
        return;
    }

    let (mode, chip_bg) = match app.focus {
        Focus::Files => (" FILES ", ACCENT),
        Focus::Sidebar => (" SHORTCUTS ", ACCENT),
        Focus::Terminal => (" TERMINAL ", GREEN),
    };
    let mut x = area.x;
    let mut room = room;
    for (text, style) in [
        (mode.to_string(), chip(chip_bg)),
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
