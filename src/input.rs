use crossterm::event::{KeyEvent, KeyEventKind};

use crate::app::App;
use crate::app::Focus;
use crate::command::Command;
use crate::keymap::{Code, Key};
use crate::modal::Modal;
use crate::picker::Pick;
use crate::{command, help};

/// Routes a key by mode: modal > filter > terminal > normal.
pub fn handle_key(app: &mut App, ev: KeyEvent) {
    if ev.kind == KeyEventKind::Release {
        return;
    }
    let Some(key) = Key::from_event(ev) else {
        return;
    };
    if app.modal.is_some() {
        modal_key(app, key);
    } else if app.filtering {
        filter_key(app, key);
    } else if app.focus == Focus::Terminal {
        // Every key goes to the shell, except the ones that hide it.
        match app.keymap.lookup(key) {
            Some(Command::TermToggle) => app.toggle_terminal(),
            _ => {
                if let Some(t) = &mut app.terminal {
                    t.key(ev);
                }
            }
        }
    } else if let Some(cmd) = app.keymap.lookup(key) {
        command::dispatch(app, cmd);
    }
}

/// The key's code when no modifier is held.
fn plain(key: Key) -> Option<Code> {
    (!key.ctrl && !key.alt && !key.shift).then_some(key.code)
}

fn modal_key(app: &mut App, key: Key) {
    if matches!(app.modal, Some(Modal::Help(_))) {
        return help_key(app, key);
    }
    match &mut app.modal {
        Some(Modal::Confirm { .. }) => match plain(key) {
            Some(Code::Char('y' | 'Y')) => app.confirm_remove(),
            Some(Code::Char('n' | 'N') | Code::Esc) => app.modal = None,
            _ => {}
        },
        Some(Modal::Rename { input, .. }) => match plain(key) {
            Some(Code::Enter) => app.apply_rename(),
            Some(Code::Esc) => app.modal = None,
            _ => {
                input.handle(key);
            }
        },
        Some(Modal::Create { input, .. }) => match plain(key) {
            Some(Code::Enter) => app.apply_create(),
            Some(Code::Esc) => app.modal = None,
            _ => {
                input.handle(key);
            }
        },
        Some(Modal::Picker(_)) => pick_key(app, key, |a| match &mut a.modal {
            Some(Modal::Picker(p)) => Some(p.as_mut()),
            _ => None,
        }),
        Some(Modal::Search(_)) => pick_key(app, key, |a| match &mut a.modal {
            Some(Modal::Search(s)) => Some(s.as_mut()),
            _ => None,
        }),
        Some(Modal::Props(_)) => {
            if let Some(Code::Esc | Code::Enter | Code::Char('p' | 'q')) = plain(key) {
                app.modal = None;
            }
        }
        Some(Modal::Help(_)) | None => {}
    }
}

/// Keys in the file picker and the content search. `Esc` closes, `Enter`
/// jumps to the file, `Shift-Enter` opens it in Helix, arrows and `Ctrl-p`/`Ctrl-n` move, the rest edits
/// the query.
fn pick_key(app: &mut App, key: Key, pick: impl Fn(&mut App) -> Option<&mut dyn Pick>) {
    let ctrl = |c| key.ctrl && !key.alt && key.code == Code::Char(c);
    if key.code == Code::Enter && key.shift && !key.ctrl && !key.alt {
        return app.picker_edit();
    }
    match plain(key) {
        Some(Code::Esc) => app.modal = None,
        Some(Code::Enter) => app.picker_jump(),
        _ => {
            let Some(p) = pick(app) else {
                return;
            };
            if plain(key) == Some(Code::Up) || ctrl('p') {
                p.move_cursor(-1);
            } else if plain(key) == Some(Code::Down) || ctrl('n') {
                p.move_cursor(1);
            } else {
                p.input().handle(key);
            }
        }
    }
}

/// The `?` menu. Not typing: `i`/`k` move, `Enter` runs, `/` filters,
/// `Esc` or `?` close, and any listed command's own key runs it.
fn help_key(app: &mut App, key: Key) {
    let Some(Modal::Help(h)) = &app.modal else {
        return;
    };
    let commands = help::commands(&help::rows(app, &h.filter.text));
    let typing = h.typing;
    let Some(Modal::Help(h)) = &mut app.modal else {
        return;
    };
    let last = commands.len().saturating_sub(1);
    let code = plain(key);
    let mut run = None;
    match code {
        Some(Code::Up) => h.cursor = h.cursor.saturating_sub(1),
        Some(Code::Down) => h.cursor = (h.cursor + 1).min(last),
        Some(Code::Enter) => run = commands.get(h.cursor).copied(),
        Some(Code::Esc) if typing => {
            h.typing = false;
            h.filter.clear();
            h.cursor = 0;
        }
        _ if typing => {
            if h.filter.handle(key) {
                h.cursor = 0;
            }
        }
        Some(Code::Esc | Code::Char('?')) => app.modal = None,
        Some(Code::Char('i')) => h.cursor = h.cursor.saturating_sub(1),
        Some(Code::Char('k')) => h.cursor = (h.cursor + 1).min(last),
        Some(Code::Char('/')) => h.typing = true,
        _ => run = app.keymap.lookup(key).filter(|c| commands.contains(c)),
    }
    if let Some(cmd) = run {
        app.modal = None;
        command::dispatch(app, cmd);
    }
}

fn filter_key(app: &mut App, key: Key) {
    match plain(key) {
        Some(Code::Enter) => app.keep_filter(),
        Some(Code::Esc) => app.cancel_filter(),
        Some(Code::Up) => app.move_cursor(-1),
        Some(Code::Down) => app.move_cursor(1),
        _ => {
            if app.filter.handle(key) {
                app.refilter_here();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::fs;
    use tempfile::TempDir;

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            let code = match c {
                '\n' => KeyCode::Enter,
                '\x1b' => KeyCode::Esc,
                '\x08' => KeyCode::Backspace,
                c => KeyCode::Char(c),
            };
            handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
        }
    }

    fn ctrl(app: &mut App, c: char) {
        handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    fn shown(app: &App) -> Vec<&str> {
        app.visible
            .iter()
            .map(|&i| app.entries[i].name.as_str())
            .collect()
    }

    fn setup(names: &[&str]) -> (TempDir, App) {
        let tmp = TempDir::new().unwrap();
        for n in names {
            fs::write(tmp.path().join(n), n).unwrap();
        }
        let app = App::new(tmp.path().canonicalize().unwrap(), None, None).unwrap();
        (tmp, app)
    }

    #[test]
    fn filter_types_keeps_and_clears() {
        let (_tmp, mut app) = setup(&["alpha.txt", "Beta.md", "gamma.txt", "zeta"]);
        ctrl(&mut app, 'f');
        press(&mut app, "ET");
        assert_eq!(shown(&app), ["Beta.md", "zeta"]);
        // While typing, letters go to the filter, not to commands.
        assert!(!app.quit);
        press(&mut app, "\n");
        assert!(!app.filtering);
        press(&mut app, "k");
        assert_eq!(app.current().unwrap().name, "zeta");
        // Esc in normal mode clears selection and filter.
        press(&mut app, " \x1b");
        assert!(app.selection.is_empty());
        assert_eq!(shown(&app).len(), 4);
        assert_eq!(app.current().unwrap().name, "zeta");

        ctrl(&mut app, 'f');
        press(&mut app, "nothing\x1b");
        assert_eq!(shown(&app).len(), 4);
        assert_eq!(app.filter.text, "");
    }

    #[test]
    fn filter_drops_hidden_selections() {
        let (_tmp, mut app) = setup(&["a.txt", "b.md"]);
        press(&mut app, " k ");
        assert_eq!(app.selection.len(), 2);
        ctrl(&mut app, 'f');
        press(&mut app, ".md\n");
        // a.txt is filtered away, so it can't stay selected.
        assert_eq!(app.selection, ["b.md"]);
    }

    #[test]
    fn trash_and_delete_confirm() {
        let (tmp, mut app) = setup(&["a", "b", "c"]);
        // n and Esc cancel; anything else is ignored.
        press(&mut app, "dqn");
        assert!(app.modal.is_none() && !app.quit);
        press(&mut app, "D\x1b");
        assert!(app.modal.is_none());
        assert_eq!(shown(&app), ["a", "b", "c"]);

        press(&mut app, " k Dy");
        assert_eq!(shown(&app), ["c"]);
        assert_eq!(app.messages[0].text, "2 deleted for good");
        assert!(app.selection.is_empty());
        assert!(!tmp.path().join("a").exists());
    }

    #[test]
    fn single_rename() {
        let (tmp, mut app) = setup(&["a", "b"]);
        // Clash: refused, dialog stays open.
        press(&mut app, "r\x08b\n");
        assert!(app.modal.is_some());
        assert_eq!(app.messages[0].text, "“b” already exists here");
        press(&mut app, "\x08new\n");
        assert!(app.modal.is_none());
        assert_eq!(shown(&app), ["b", "new"]);
        assert_eq!(app.current().unwrap().name, "new");
        assert_eq!(fs::read_to_string(tmp.path().join("new")).unwrap(), "a");
        // Esc leaves the file alone.
        press(&mut app, "rxyz\x1b");
        assert_eq!(shown(&app), ["b", "new"]);
    }

    #[test]
    fn bulk_rename_in_selection_order() {
        let (tmp, mut app) = setup(&["a.mkv", "b.mkv", "c.mkv", "keep"]);
        // Select c, a, b in that order.
        press(&mut app, "kk ii k ");
        assert_eq!(app.selection, ["c.mkv", "a.mkv", "b.mkv"]);
        press(&mut app, "rkeep\n");
        assert!(app.modal.is_some(), "clash must refuse");
        assert!(app.messages[0].text.starts_with("fix the names marked ✗"));
        ctrl(&mut app, 'u');
        press(&mut app, "ep[01].{ext}\n");
        assert_eq!(shown(&app), ["ep01.mkv", "ep02.mkv", "ep03.mkv", "keep"]);
        assert_eq!(app.messages[0].text, "renamed 3 files");
        let read = |n: &str| fs::read_to_string(tmp.path().join(n)).unwrap();
        assert_eq!(
            (read("ep01.mkv"), read("ep02.mkv"), read("ep03.mkv")),
            ("c.mkv".into(), "a.mkv".into(), "b.mkv".into())
        );
    }

    #[test]
    fn sidebar_space_and_enter_open_the_shortcut() {
        let (tmp, mut app) = setup(&["a"]);
        let sub = tmp.path().canonicalize().unwrap().join("sub");
        fs::create_dir(&sub).unwrap();
        app.shortcuts =
            crate::shortcuts::Shortcuts::build(None, [].into_iter(), vec![sub.clone()], None);
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        press(&mut app, " ");
        assert_eq!(
            (app.dir.clone(), app.focus),
            (sub.clone(), crate::app::Focus::Files)
        );
        press(&mut app, "j");
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        press(&mut app, "\n");
        assert_eq!(app.dir, sub);
        assert!(app.selection.is_empty());
    }

    fn help_open(app: &App) -> bool {
        matches!(app.modal, Some(Modal::Help(_)))
    }

    #[test]
    fn help_runs_by_key_by_enter_and_filters() {
        let (_tmp, mut app) = setup(&["a", "b", ".dot"]);
        press(&mut app, "?");
        assert!(help_open(&app));
        // `h` is listed (Toggle hidden files), so pressing it runs it.
        press(&mut app, "h");
        assert!(!help_open(&app) && app.show_hidden);

        // i/k move; `?` closes without running anything.
        press(&mut app, "?kki?");
        assert!(!help_open(&app));

        // `/` filters titles; letters then go to the filter, not commands.
        press(&mut app, "?/quit");
        assert!(help_open(&app) && !app.quit);
        press(&mut app, "\n");
        assert!(app.quit);
    }

    #[test]
    fn help_escape_leaves_filter_then_menu() {
        let (_tmp, mut app) = setup(&["a"]);
        press(&mut app, "?/zz\x1b");
        let Some(Modal::Help(h)) = &app.modal else {
            panic!("help closed too early");
        };
        assert!(!h.typing && h.filter.text.is_empty());
        press(&mut app, "\x1b");
        assert!(app.modal.is_none());
    }

    #[test]
    fn picker_jumps_to_the_file() {
        let (tmp, mut app) = setup(&["top.txt"]);
        let root = tmp.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("deep/er")).unwrap();
        fs::write(root.join("deep/er/needle.rs"), "").unwrap();
        fs::write(root.join("deep/er/.secret"), "").unwrap();
        press(&mut app, "f");
        assert!(matches!(app.modal, Some(Modal::Picker(_))));
        press(&mut app, "needle");
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            app.drain_workers();
            if let Some(Modal::Picker(p)) = &app.modal
                && p.selected() == Some("deep/er/needle.rs")
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        press(&mut app, "\n");
        assert!(app.modal.is_none());
        assert_eq!(app.dir, root.join("deep/er"));
        assert_eq!(app.current().unwrap().name, "needle.rs");
    }

    fn screen_has(app: &mut App, needle: &str) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            app.drain_workers();
            let found = app
                .terminal
                .as_ref()
                .is_some_and(|t| t.with_screen(|s| s.contents().contains(needle)));
            if found {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn terminal_focus_rules_and_keys() {
        use crate::app::Focus;
        let (_tmp, mut app) = setup(&["a"]);
        app.shell = "/bin/sh".into();

        // From the sidebar: open and focus, then hide and go back there.
        app.cycle_focus(1);
        ctrl(&mut app, 's');
        assert!(app.terminal_open && app.focus == Focus::Terminal);
        // Letters, Tab and Esc go to the shell, not to commands.
        press(&mut app, "q\x1b");
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(!app.quit && app.focus == Focus::Terminal);
        // Clear the junk with Ctrl-u, then run something.
        ctrl(&mut app, 'u');
        press(&mut app, "echo tosk-$((40+2))\n");
        assert!(screen_has(&mut app, "tosk-42"));

        ctrl(&mut app, 's');
        assert!(!app.terminal_open && app.focus == Focus::Sidebar);
        // The shell lives on while hidden.
        assert!(app.terminal.is_some());
        ctrl(&mut app, 's');
        assert!(screen_has(&mut app, "tosk-42"));

        // exit: the panel closes and focus goes back.
        press(&mut app, "exit\n");
        let start = std::time::Instant::now();
        while app.terminal.is_some() && start.elapsed() < std::time::Duration::from_secs(5) {
            app.drain_workers();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.terminal.is_none() && !app.terminal_open);
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn x_runs_scripts_in_the_panel() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        fs::write(
            dir.join("my script.sh"),
            "echo ran > ran.txt\necho script-done\n",
        )
        .unwrap();
        fs::write(dir.join("notes.txt"), "").unwrap();
        let mut app = App::new(dir.clone(), None, None).unwrap();
        app.shell = "/bin/sh".into();

        // Sorted: "my script.sh", then "notes.txt".
        press(&mut app, "k");
        press(&mut app, "x");
        assert_eq!(app.messages[0].text, "x runs .sh files");
        assert!(app.terminal.is_none());

        press(&mut app, "i");
        press(&mut app, "x");
        assert_eq!(app.focus, crate::app::Focus::Terminal);
        assert!(screen_has(&mut app, "script-done"));
        assert!(dir.join("ran.txt").exists());
    }

    #[test]
    fn n_and_shift_n_create() {
        let (tmp, mut app) = setup(&["b"]);
        let dir = tmp.path().canonicalize().unwrap();
        press(&mut app, "nnotes.md\n");
        assert!(dir.join("notes.md").is_file());
        assert_eq!(app.current().unwrap().name, "notes.md");
        press(&mut app, "Nsrc\n");
        assert!(dir.join("src").is_dir());
        assert_eq!(app.current().unwrap().name, "src");

        // Taken names and slashes are refused, and the dialog stays open.
        press(&mut app, "nb\n");
        assert_eq!(app.messages[0].text, "“b” already exists here");
        assert!(app.modal.is_some());
        press(&mut app, "\x08a/c\n");
        assert!(app.messages[0].text.contains("contain /"));
        press(&mut app, "\x1b");
        assert!(app.modal.is_none());
        assert_eq!(fs::read_to_string(dir.join("b")).unwrap(), "b");

        // A dotfile turns hidden files on so the cursor can land on it.
        press(&mut app, "n.env\n");
        assert!(app.show_hidden);
        assert_eq!(app.current().unwrap().name, ".env");
    }

    #[test]
    fn s_cycles_sort_and_keeps_the_cursor() {
        let (tmp, mut app) = setup(&["small.txt", "big.rs"]);
        fs::write(tmp.path().join("big.rs"), vec![b'x'; 5000]).unwrap();
        // Pick up the new size.
        press(&mut app, "nfresh\n");
        let names = |app: &App| -> Vec<String> {
            app.visible
                .iter()
                .map(|&i| app.entries[i].name.clone())
                .collect()
        };
        assert_eq!(names(&app), ["big.rs", "fresh", "small.txt"]);
        press(&mut app, "s");
        assert_eq!(names(&app), ["fresh", "big.rs", "small.txt"]);
        assert_eq!(app.current().unwrap().name, "fresh");
        press(&mut app, "s");
        assert_eq!(names(&app), ["big.rs", "small.txt", "fresh"]);
        assert_eq!(app.messages[0].text, "sorted by size, largest first");
        // S flips it; s moves on and starts the next one unflipped.
        press(&mut app, "S");
        assert_eq!(names(&app), ["fresh", "small.txt", "big.rs"]);
        assert_eq!(app.messages[0].text, "sorted by size, smallest first");
        press(&mut app, "s");
        assert!(!app.sort.reverse);
        press(&mut app, "s");
        assert_eq!(app.sort, crate::sort::Sort::default());
    }

    #[test]
    fn e_refuses_what_isnt_text() {
        let (tmp, mut app) = setup(&["a"]);
        let dir = tmp.path().canonicalize().unwrap();
        fs::write(dir.join("blob"), b"\0\x01\x02").unwrap();
        fs::write(dir.join("photo.png"), b"").unwrap();
        press(&mut app, "nz\n");
        let at = |app: &mut App, name: &str| {
            app.cursor = app
                .visible
                .iter()
                .position(|&i| app.entries[i].name == name)
                .unwrap();
        };
        at(&mut app, "blob");
        press(&mut app, "e");
        assert_eq!(
            app.messages[0].text,
            "e edits text files; blob is a binary file"
        );
        at(&mut app, "photo.png");
        press(&mut app, "e");
        assert_eq!(
            app.messages[0].text,
            "e edits text files; photo.png is an image"
        );
        // A folder among several selected items is refused too.
        fs::create_dir(dir.join("sub")).unwrap();
        press(&mut app, "nzz\n");
        at(&mut app, "sub");
        press(&mut app, " ");
        at(&mut app, "a");
        press(&mut app, " e");
        assert_eq!(app.messages[0].text, "e edits text files; sub is a folder");
    }

    #[test]
    fn shift_enter_in_pickers_refuses_non_text() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        fs::write(dir.join("blob.bin"), b"\0\0").unwrap();
        let mut app = App::new(dir, None, None).unwrap();
        press(&mut app, "fblob");
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            app.drain_workers();
            if let Some(Modal::Picker(p)) = &app.modal
                && p.selected().is_some()
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(
            app.messages[0].text,
            "Shift-Enter edits text files; blob.bin is a binary file"
        );
        assert!(app.modal.is_some(), "the picker stays open");
    }
}
