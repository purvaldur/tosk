use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;
use tui_term::widget::{Cursor, PseudoTerminal};

use super::{header, put};
use crate::app::{App, Focus};
use crate::command::Command;
use crate::theme::*;

/// The panel under its header row.
pub fn body(area: Rect) -> Rect {
    Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    }
}

pub fn render(app: &App, buf: &mut Buffer, area: Rect) {
    let focused = app.focus == Focus::Terminal;
    let head = Rect { height: 1, ..area };
    buf.set_style(head, Style::default().bg(MANTLE));
    let key = app
        .keymap
        .first_key(Command::TermToggle)
        .map(|k| k.pretty())
        .unwrap_or_else(|| "term.toggle".into());
    let path = app
        .terminal
        .as_ref()
        .map(|t| app.display_path(&t.cwd))
        .unwrap_or_default();
    // The panel only shows while it has focus, so it can only hide.
    header(
        buf,
        head,
        &format!("terminal · {path} · {key} hides"),
        focused,
    );

    let body = body(area);
    let Some(t) = &app.terminal else {
        let style = Style::default().fg(OVERLAY0);
        put(buf, body.x + 1, body.y, "starting…", body.width, style);
        return;
    };
    t.with_screen(|screen| {
        let mut cursor = Cursor::default();
        if !focused {
            cursor.hide();
        }
        PseudoTerminal::new(screen).cursor(cursor).render(body, buf);
    });
}
