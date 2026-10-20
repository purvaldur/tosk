use crossterm::event::{KeyEvent, KeyEventKind};

use crate::app::App;
use crate::command;
use crate::keymap::{Code, Key};
use crate::modal::Modal;

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
    } else if let Some(cmd) = app.keymap.lookup(key) {
        command::dispatch(app, cmd);
    }
}

/// The key's code when no modifier is held.
fn plain(key: Key) -> Option<Code> {
    (!key.ctrl && !key.alt && !key.shift).then_some(key.code)
}

fn modal_key(app: &mut App, key: Key) {
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
        None => {}
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
        assert_eq!((app.dir.clone(), app.focus), (sub.clone(), crate::app::Focus::Files));
        press(&mut app, "j");
        handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        press(&mut app, "\n");
        assert_eq!(app.dir, sub);
        assert!(app.selection.is_empty());
    }
}
