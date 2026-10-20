mod list;
mod messages;
mod preview;
mod sidebar;
mod status;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;

use crate::app::App;
use crate::theme::*;

pub const SIDEBAR_WIDTH: u16 = 18;

/// Where everything goes. Pane areas exclude the dividers between them.
pub struct Areas {
    pub side: Rect,
    pub list: Rect,
    pub preview: Option<Rect>,
    pub status: Rect,
    pub messages: Rect,
}

impl Areas {
    pub fn new(area: Rect, preview: bool) -> Areas {
        let [panes, status, messages] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(2),
        ])
        .areas(area);
        let [side, rest] =
            Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Fill(1)])
                .areas(panes);
        let (list, preview) = if preview {
            // The list takes about 60% of what the sidebar leaves.
            let [list, preview] =
                Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)]).areas(rest);
            (without_divider(list), Some(preview))
        } else {
            (rest, None)
        };
        Areas {
            side: without_divider(side),
            list,
            preview,
            status,
            messages,
        }
    }

    /// The size the preview worker builds content for.
    pub fn preview_size(&self) -> Option<(u16, u16)> {
        self.preview
            .map(preview_body)
            .map(|r| (r.width, r.height))
            .filter(|&(w, h)| w > 0 && h > 0)
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let a = Areas::new(area, app.preview);
    let buf = frame.buffer_mut();
    buf.set_style(area, Style::default().bg(BASE).fg(TEXT));

    divider(buf, a.side);
    sidebar::render(app, buf, a.side);
    list::render(app, buf, a.list);
    if let Some(preview) = a.preview {
        divider(buf, a.list);
        preview::render(app, buf, preview);
    }
    status::render(app, buf, a.status);
    messages::render(app, buf, a.messages);
}

fn without_divider(area: Rect) -> Rect {
    Rect {
        width: area.width.saturating_sub(1),
        ..area
    }
}

/// Draws the divider just right of a pane.
fn divider(buf: &mut Buffer, pane: Rect) {
    let x = pane.right();
    if x >= buf.area.right() {
        return;
    }
    for y in pane.top()..pane.bottom() {
        buf[(x, y)]
            .set_symbol("│")
            .set_style(Style::default().fg(SURFACE0));
    }
}

/// The preview's content area: under the header, one column in from each side.
fn preview_body(area: Rect) -> Rect {
    let b = body(area);
    Rect {
        x: b.x + 1,
        width: b.width.saturating_sub(2),
        ..b
    }
}

/// The one-row pane header, in the accent colour when focused.
fn header(buf: &mut Buffer, area: Rect, text: &str, focused: bool) {
    let fg = if focused { ACCENT } else { OVERLAY0 };
    put(
        buf,
        area.x + 1,
        area.y,
        text,
        area.width.saturating_sub(2),
        Style::default().fg(fg),
    );
}

/// The part of a pane under its header.
fn body(area: Rect) -> Rect {
    Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    }
}

pub fn width(s: &str) -> u16 {
    Span::raw(s).width().try_into().unwrap_or(u16::MAX)
}

/// Writes `s` within `max` columns, ending in `…` when it doesn't fit.
/// Returns the columns used.
fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, max: u16, style: Style) -> u16 {
    if max == 0 {
        return 0;
    }
    if width(s) <= max {
        let (end, _) = buf.set_stringn(x, y, s, max as usize, style);
        return end - x;
    }
    let (end, _) = buf.set_stringn(x, y, s, (max - 1) as usize, style);
    buf.set_stringn(end, y, "…", 1, style);
    end + 1 - x
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn screen(app: &App, w: u16, h: u16) -> Vec<String> {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        let buf = t.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    /// Lets the workers finish and points the preview at the cursor, like
    /// the main loop does before drawing.
    fn settle(app: &mut App, w: u16, h: u16) {
        let size = Areas::new(Rect::new(0, 0, w, h), app.preview).preview_size();
        for _ in 0..500 {
            app.sync_preview(size);
            app.drain_workers();
            let counted = app
                .entries
                .iter()
                .all(|e| !e.is_dir() || e.children.is_some());
            let previewed = app
                .preview_key
                .as_ref()
                .is_none_or(|k| app.previewer.get(k).is_some());
            if counted && previewed {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("workers didn't finish");
    }

    #[test]
    fn draws_the_layout() {
        use crate::shortcuts::Shortcuts;
        let tmp = tempfile::TempDir::new().unwrap();
        let r = tmp.path().canonicalize().unwrap();
        fs::create_dir(r.join("src")).unwrap();
        fs::write(r.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(r.join("Cargo.toml"), "[package]\nname = \"tosk\"\n").unwrap();
        fs::write(r.join(".hidden"), "").unwrap();
        symlink("src", r.join("code")).unwrap();
        let mut app = App::new(r.clone(), None, None).unwrap();
        app.home = Some(r.clone());
        let pins = vec![r.join("src")];
        app.shortcuts = Shortcuts::build(Some(&r), [].into_iter(), pins, None);
        let (home, pin) = (crate::icons::HOME, crate::icons::PINNED);

        settle(&mut app, 80, 9);
        let s = screen(&app, 80, 9);
        println!("{}", s.join("\n"));
        assert!(s[0].contains("shortcuts") && s[0].contains("~ ") && s[0].contains("code → src"));
        // Home, a divider, then the pinned folder.
        assert!(s[1].contains(&format!("{home} Home")));
        assert!(s[2].starts_with("─────"));
        assert!(s[3].contains(&format!("{pin} src")));
        // Links to folders sort with the folders, and rows have icons.
        assert!(s[1].contains("\u{f482} code") && s[1].contains("→ src/"));
        assert!(s[2].contains("\u{f07b} src/") && s[2].contains("1 item"));
        assert!(s[3].contains("\u{e615} Cargo.toml") && s[3].contains("24B"));
        // The folder preview of the linked src.
        assert!(s[1].contains("\u{e7a8} main.rs"));
        assert!(s[6].contains(" FILES ") && s[6].ends_with("3 items · ? help "));

        // Text preview with line numbers.
        app.move_cursor(2);
        settle(&mut app, 80, 9);
        let s = screen(&app, 80, 9);
        println!("{}", s.join("\n"));
        assert!(s[0].contains("Cargo.toml · 24B"));
        assert!(s[1].contains("   1  [package]"));
        assert!(s[2].contains("   2  name = \"tosk\""));

        // Selection: marker, and the summary in the list's bottom-right corner.
        app.toggle_select();
        app.move_cursor(-2);
        app.toggle_select();
        let s = screen(&app, 80, 9);
        println!("{}", s.join("\n"));
        assert!(s[1].contains("● \u{f482} code"));
        assert!(s[3].contains("● \u{e615} Cargo.toml"));
        // code links to a folder, so this is the mixed wording, cut to fit.
        assert!(s[5].contains("1 folder selected (containing 1…  │"));

        // Sidebar focus: SHORTCUTS chip.
        app.cycle_focus(1);
        let s = screen(&app, 80, 9);
        assert!(s[6].contains(" SHORTCUTS "));

        app.clear_selection();
        app.toggle_hidden();
        app.toggle_preview();
        app.info("followed code → ~/src".into());
        let s = screen(&app, 80, 9);
        println!("{}", s.join("\n"));
        assert!(s[6].contains("4 items · hidden shown · ? help"));
        assert!(s[7].contains("followed code"));
        assert!(!s[1].contains("main.rs"));
        assert!(!s[5].contains("selected"));

        // Narrow terminals truncate instead of overlapping or panicking.
        app.toggle_preview();
        app.toggle_select();
        settle(&mut app, 24, 4);
        let s = screen(&app, 24, 4);
        println!("{}", s.join("\n"));
        screen(&app, 1, 1);
    }
}
