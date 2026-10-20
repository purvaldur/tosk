use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use super::put;
use crate::app::App;
use crate::theme::*;

/// Newest message on top in full colour, the one before it dimmed.
pub fn render(app: &App, buf: &mut Buffer, area: Rect) {
    for (i, m) in app.messages.iter().take(area.height as usize).enumerate() {
        let fg = match (m.error, i) {
            (true, _) => RED,
            (false, 0) => TEXT,
            (false, _) => OVERLAY0,
        };
        put(
            buf,
            area.x + 1,
            area.y + i as u16,
            &m.text,
            area.width.saturating_sub(2),
            Style::default().fg(fg),
        );
    }
}
