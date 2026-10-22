mod dialog;
mod help;
mod list;
mod messages;
mod picker;
mod preview;
mod props;
mod sidebar;
mod status;
mod terminal;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;

use crate::app::App;
use crate::text_input::TextInput;
use crate::theme::*;

pub const SIDEBAR_WIDTH: u16 = 18;

/// Where everything goes. Pane areas exclude the dividers between them.
pub struct Areas {
    pub side: Rect,
    pub list: Rect,
    pub preview: Option<Rect>,
    /// The terminal panel, header included, while it's open.
    pub terminal: Option<Rect>,
    pub status: Rect,
    pub messages: Rect,
}

impl Areas {
    pub fn new(area: Rect, app: &App) -> Areas {
        let (preview, terminal) = (app.preview, app.terminal_open);
        // A third of the height, within reason, when the terminal is open.
        let term_height = if terminal {
            (area.height / 3).clamp(7, 20)
        } else {
            0
        };
        let [panes, term, status, messages] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(term_height),
            Constraint::Length(1),
            Constraint::Length(if app.show_log { 2 } else { 0 }),
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
            terminal: terminal.then_some(term),
            status,
            messages,
        }
    }

    /// The terminal body as (rows, cols), for the pty.
    pub fn terminal_size(&self) -> Option<(u16, u16)> {
        self.terminal
            .map(terminal::body)
            .map(|r| (r.height, r.width))
            .filter(|&(h, w)| h > 0 && w > 0)
    }

    /// The size the preview worker builds content for.
    pub fn preview_size(&self) -> Option<(u16, u16)> {
        self.preview
            .map(preview_body)
            .map(|r| (r.width, r.height))
            .filter(|&(w, h)| w > 0 && h > 0)
    }
}

/// The size of the file picker's preview, for the preview worker.
pub fn picker_preview_size(area: Rect) -> Option<(u16, u16)> {
    picker::preview_size(area)
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let a = Areas::new(area, app);
    let buf = frame.buffer_mut();
    buf.set_style(area, Style::default().bg(BASE).fg(TEXT));

    divider(buf, a.side);
    sidebar::render(app, buf, a.side);
    list::render(app, buf, a.list);
    if let Some(preview) = a.preview {
        divider(buf, a.list);
        preview::render(app, buf, preview);
    }
    if let Some(term) = a.terminal {
        terminal::render(app, buf, term);
    }
    status::render(app, buf, a.status);
    messages::render(app, buf, a.messages);
    dialog::render(app, buf, area);
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

/// A one-line text input with a block caret, scrolled so the caret shows.
fn input_line(buf: &mut Buffer, x: u16, y: u16, w: u16, input: &TextInput, style: Style) {
    if w == 0 {
        return;
    }
    let mut before = input.before_cursor();
    while width(before) >= w {
        let mut chars = before.chars();
        chars.next();
        before = chars.as_str();
    }
    let start = input.before_cursor().len() - before.len();
    put(buf, x, y, &input.text[start..], w, style);
    let caret = x + width(before);
    buf[(caret, y)].set_style(Style::default().bg(TEXT).fg(BASE));
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
        let size = Areas::new(Rect::new(0, 0, w, h), app).preview_size();
        for _ in 0..500 {
            app.sync_preview(size, None);
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

    #[test]
    fn git_segment_and_marks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let r = tmp.path().to_path_buf();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(&r)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "trunk"]);
        fs::create_dir_all(r.join("src")).unwrap();
        fs::write(r.join("src/lib.rs"), "a").unwrap();
        fs::write(r.join(".gitignore"), "build/\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        fs::write(r.join("src/lib.rs"), "b").unwrap();
        fs::write(r.join("new.txt"), "").unwrap();
        fs::create_dir_all(r.join("build")).unwrap();

        let mut app = App::new(r.clone(), None, None).unwrap();
        for _ in 0..500 {
            app.drain_workers();
            if app.git.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        settle(&mut app, 100, 12);
        let lines = screen(&app, 100, 12);
        // The segment ends the list's header row, before the preview.
        let list_header = lines[0].split('│').nth(1).unwrap();
        assert!(
            list_header.trim_end().ends_with("\u{e0a0} trunk ~2"),
            "{list_header:?}"
        );
        // The list column of the row naming `name`.
        let row = |name: &str| {
            lines
                .iter()
                .filter_map(|l| l.split('│').nth(1))
                .find(|l| l.contains(name))
                .unwrap()
                .to_string()
        };
        assert!(row("src/").contains("●"));
        assert!(row("new.txt").trim_end().ends_with('U'));
        assert!(!row("build/").contains('●'));

        // Into a subfolder of the same repo: the status stays meanwhile.
        app.nav_enter();
        assert!(app.git.is_some());
    }

    #[test]
    fn draws_dialogs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let r = tmp.path().canonicalize().unwrap();
        for n in ["a.mkv", "b.mkv", "c.mkv", "d", "e", "f", "g"] {
            fs::write(r.join(n), "").unwrap();
        }
        let mut app = App::new(r.clone(), None, None).unwrap();
        app.home = Some(r.clone());

        app.start_remove(false);
        let s = screen(&app, 80, 16);
        println!("{}", s.join("\n"));
        let all = s.join("\n");
        assert!(all.contains("Move to trash") && all.contains("1 item"));
        assert!(all.contains("~/a.mkv") && all.contains("y trash · n or Esc cancel"));

        for _ in 0..7 {
            app.toggle_select();
            app.move_cursor(1);
        }
        app.start_remove(true);
        let all = screen(&app, 80, 16).join("\n");
        println!("{all}");
        assert!(all.contains("Delete permanently") && all.contains("7 items"));
        assert!(all.contains("…and 2 more"));
        assert!(all.contains("This skips the trash and can't be undone."));
        assert!(all.contains("y delete · n or Esc cancel"));

        app.modal = None;
        app.start_rename();
        if let Some(crate::modal::Modal::Rename { input, .. }) = &mut app.modal {
            *input = TextInput::new("x[01]");
        }
        let all = screen(&app, 90, 20).join("\n");
        println!("{all}");
        assert!(all.contains("Bulk rename") && all.contains("7 files"));
        assert!(all.contains("a.mkv") && all.contains("x01"));
        assert!(all.contains("[01] counter · {name} old name"));

        app.selection.clear();
        app.cursor_to_top();
        app.start_rename();
        let all = screen(&app, 80, 16).join("\n");
        println!("{all}");
        assert!(all.contains(" Rename ") && all.contains("Enter apply · Esc cancel"));

        // A filter replaces the path in the status line.
        app.modal = None;
        app.start_filter();
        app.filter = TextInput::new("zzz");
        app.refilter_here();
        let s = screen(&app, 80, 10);
        println!("{}", s.join("\n"));
        assert!(s[7].starts_with(" FILTER  zzz"));
        assert!(s[1].contains("no match for “zzz”"));
        // Small terminals don't panic.
        app.start_remove(true);
        screen(&app, 10, 5);
        screen(&app, 1, 1);
    }

    #[test]
    fn draws_help_and_picker() {
        let tmp = tempfile::TempDir::new().unwrap();
        let r = tmp.path().canonicalize().unwrap();
        fs::write(r.join("build.sh"), "#!/bin/sh\necho hi\n").unwrap();
        fs::create_dir(r.join("src")).unwrap();
        fs::write(r.join("src/main.rs"), "fn main() {}\n").unwrap();
        let mut app = App::new(r.clone(), None, None).unwrap();
        app.home = Some(r.clone());
        app.cursor = 1;

        app.open_help();
        let all = screen(&app, 90, 40).join("\n");
        println!("{all}");
        assert!(all.contains("Keys") && all.contains("most relevant first"));
        assert!(all.contains("On build.sh"));
        assert!(all.contains("x       Run script in terminal panel"));
        assert!(all.contains("Everywhere") && all.contains("?       Shortcuts for this context"));

        app.open_picker();
        if let Some(crate::modal::Modal::Picker(p)) = &mut app.modal {
            p.input = TextInput::new("mai");
        }
        let start = std::time::Instant::now();
        loop {
            app.sync_preview(None, picker_preview_size(Rect::new(0, 0, 90, 20)));
            app.drain_workers();
            let ready = app
                .preview_key
                .as_ref()
                .is_some_and(|k| app.previewer.get(k).is_some());
            if ready || start.elapsed() > std::time::Duration::from_secs(5) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let all = screen(&app, 90, 20).join("\n");
        println!("{all}");
        assert!(all.contains("Files · ~") && all.contains("1/2"));
        assert!(all.contains("› mai"));
        assert!(all.contains("src/main.rs") && all.contains("1  fn main() {}"));
        // Only the picker shows the file; the pane behind it doesn't.
        assert_eq!(all.matches("fn main()").count(), 1);
        screen(&app, 12, 6);
        screen(&app, 1, 1);
    }

    #[test]
    fn draws_search_and_terminal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let r = tmp.path().canonicalize().unwrap();
        fs::create_dir_all(r.join("stage")).unwrap();
        let yaml: String =
            (1..40).map(|i| format!("key{i}: x\n")).collect::<String>() + "name: foo\n";
        fs::write(r.join("stage/deployment.yaml"), yaml).unwrap();
        fs::write(r.join("other.yaml"), "name: foo\n").unwrap();
        let mut app = App::new(r.clone(), None, None).unwrap();
        app.home = Some(r.clone());

        app.open_search();
        let all = screen(&app, 100, 24).join("\n");
        assert!(all.contains("Search · ~") && all.contains("%p filters paths"));
        assert!(all.contains("Enter go to file · Shift-Enter edit in Helix"));
        if let Some(crate::modal::Modal::Search(s)) = &mut app.modal {
            s.input = TextInput::new("name: foo %p stdep");
        }
        let start = std::time::Instant::now();
        loop {
            app.sync_preview(None, picker_preview_size(Rect::new(0, 0, 100, 24)));
            app.drain_workers();
            let ready = app
                .preview_key
                .as_ref()
                .is_some_and(|k| app.previewer.get(k).is_some());
            if ready || start.elapsed() > std::time::Duration::from_secs(5) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let all = screen(&app, 100, 24).join("\n");
        println!("{all}");
        assert!(all.contains("stage/deployment.yaml:40 "));
        // The matched text shows in the query and the preview, not the results.
        assert_eq!(all.matches("name: foo").count(), 2);
        assert!(!all.contains("other.yaml"));
        // The preview opens at the matching line.
        assert!(all.contains("40  name: foo") && all.contains("37  key37: x"));
        assert_eq!(
            all.matches("key37").count(),
            1,
            "only the search previews it"
        );

        app.modal = None;
        app.shell = "/bin/sh".into();
        app.toggle_terminal();
        let areas = Areas::new(Rect::new(0, 0, 100, 30), &app);
        app.sync_terminal_size(areas.terminal_size());
        if let Some(t) = &mut app.terminal {
            t.write(b"echo panel-$((6*7))\r");
        }
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            app.drain_workers();
            if screen(&app, 100, 30).join("\n").contains("panel-42") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let s = screen(&app, 100, 30);
        println!("{}", s.join("\n"));
        let all = s.join("\n");
        assert!(all.contains("terminal · ~ · Ctrl-s hides") && all.contains("panel-42"));
        assert!(s[27].starts_with(" TERMINAL "));
        app.toggle_terminal();
        let all = screen(&app, 100, 30).join("\n");
        assert!(!all.contains("panel-42"));
    }

    #[test]
    fn popups_log_and_create() {
        let tmp = tempfile::TempDir::new().unwrap();
        let r = tmp.path().canonicalize().unwrap();
        for n in ["a.txt", "b.txt"] {
            fs::write(r.join(n), "").unwrap();
        }
        let mut app = App::new(r.clone(), None, None).unwrap();
        app.home = Some(r.clone());
        app.info("hello from the log".into());

        // Popups: white border on the app background, no darker fill.
        app.start_remove(false);
        let mut t = ratatui::Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|f| draw(f, &app)).unwrap();
        let buf = t.backend().buffer().clone();
        let corner = (0..80)
            .flat_map(|x| (0..24).map(move |y| (x, y)))
            .find(|&(x, y)| buf[(x, y)].symbol() == "┌")
            .unwrap();
        assert_eq!(buf[corner].fg, POPUP_BORDER);
        assert_eq!(buf[(corner.0 + 3, corner.1 + 2)].bg, BASE);

        app.modal = None;
        let s = screen(&app, 80, 12);
        assert!(s[10].contains("hello from the log"));
        app.show_log = false;
        let s = screen(&app, 80, 12);
        println!("{}", s.join("\n"));
        assert!(!s.join("\n").contains("hello from the log"));
        // The status line moves down to the bottom row.
        assert!(s[11].starts_with(" FILES "));

        app.start_create(true);
        let all = screen(&app, 80, 16).join("\n");
        println!("{all}");
        assert!(all.contains("New folder") && all.contains("Enter create · Esc cancel"));
    }
}
