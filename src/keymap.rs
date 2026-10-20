use std::fmt;

use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::command::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    /// Case-sensitive; `Space` is `Char(' ')`.
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Home,
    End,
    Up,
    Down,
    Left,
    Right,
}

const NAMED: &[(&str, Code)] = &[
    ("Enter", Code::Enter),
    ("Esc", Code::Esc),
    ("Tab", Code::Tab),
    ("Space", Code::Char(' ')),
    ("Backspace", Code::Backspace),
    ("Home", Code::Home),
    ("End", Code::End),
    ("Up", Code::Up),
    ("Down", Code::Down),
    ("Left", Code::Left),
    ("Right", Code::Right),
];

/// A key as written in the keymap: `q`, `C`, `C-t`, `S-Tab`, `Space`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: Code,
    pub ctrl: bool,
    pub alt: bool,
    /// Only for named keys; for characters, shift is part of the character.
    pub shift: bool,
}

impl Key {
    pub fn parse(s: &str) -> Result<Key> {
        let (mut ctrl, mut alt, mut shift) = (false, false, false);
        let mut rest = s;
        // A bare "C" or "-" is a key of its own, so a prefix needs something after it.
        while rest.len() > 2 {
            match rest.get(..2) {
                Some("C-") => ctrl = true,
                Some("A-") => alt = true,
                Some("S-") => shift = true,
                _ => break,
            }
            rest = &rest[2..];
        }
        let code = match NAMED.iter().find(|(name, _)| *name == rest) {
            Some(&(_, code)) => code,
            None => {
                let mut chars = rest.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Code::Char(c),
                    _ => bail!("unknown key name `{s}`"),
                }
            }
        };
        if shift && matches!(code, Code::Char(_)) {
            bail!("`{s}`: S- is only for named keys; write the shifted character instead");
        }
        Ok(Key {
            code,
            ctrl,
            alt,
            shift,
        })
    }

    pub fn from_event(ev: KeyEvent) -> Option<Key> {
        let m = ev.modifiers;
        let mut shift = m.contains(KeyModifiers::SHIFT);
        let code = match ev.code {
            KeyCode::Char(c) => {
                // Shift is carried by the character: `C`, `?`.
                let c = if shift {
                    c.to_uppercase().next().unwrap_or(c)
                } else {
                    c
                };
                shift = false;
                Code::Char(c)
            }
            KeyCode::Enter => Code::Enter,
            KeyCode::Esc => Code::Esc,
            KeyCode::Tab => Code::Tab,
            KeyCode::BackTab => {
                shift = true;
                Code::Tab
            }
            KeyCode::Backspace => Code::Backspace,
            KeyCode::Home => Code::Home,
            KeyCode::End => Code::End,
            KeyCode::Up => Code::Up,
            KeyCode::Down => Code::Down,
            KeyCode::Left => Code::Left,
            KeyCode::Right => Code::Right,
            _ => return None,
        };
        Some(Key {
            code,
            ctrl: m.contains(KeyModifiers::CONTROL),
            alt: m.contains(KeyModifiers::ALT),
            shift,
        })
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("C-")?;
        }
        if self.alt {
            f.write_str("A-")?;
        }
        if self.shift {
            f.write_str("S-")?;
        }
        match NAMED.iter().find(|(_, code)| *code == self.code) {
            Some((name, _)) => f.write_str(name),
            None => match self.code {
                Code::Char(c) => write!(f, "{c}"),
                _ => unreachable!("every non-char code is named"),
            },
        }
    }
}

/// Default keys from section 7 of the spec, in display order.
const DEFAULTS: &[(&str, &str)] = &[
    ("i", "cursor.up"),
    ("Up", "cursor.up"),
    ("k", "cursor.down"),
    ("Down", "cursor.down"),
    ("j", "nav.parent"),
    ("Left", "nav.parent"),
    ("l", "nav.enter"),
    ("Right", "nav.enter"),
    ("Home", "cursor.top"),
    ("End", "cursor.bottom"),
    ("Enter", "open.default"),
    ("o", "open.default"),
    ("e", "open.editor"),
    ("c", "file.copy"),
    ("C", "path.copy"),
    ("r", "file.rename"),
    ("d", "file.trash"),
    ("D", "file.delete"),
    ("Space", "select.toggle"),
    ("Esc", "select.clear"),
    ("x", "script.run"),
    ("f", "picker.root"),
    ("F", "picker.here"),
    ("C-f", "filter.start"),
    ("C-t", "term.toggle"),
    ("Tab", "focus.next"),
    ("S-Tab", "focus.prev"),
    ("p", "file.props"),
    ("b", "shortcut.pin"),
    ("P", "view.preview"),
    ("h", "view.hidden"),
    ("?", "help.open"),
    ("q", "app.quit"),
];

pub struct Keymap {
    bindings: Vec<(Key, Command)>,
}

impl Keymap {
    pub fn defaults() -> Keymap {
        let bindings = DEFAULTS
            .iter()
            .map(|&(key, id)| {
                let key = Key::parse(key).expect("default key parses");
                (key, Command::from_id(id).expect("default command exists"))
            })
            .collect();
        Keymap { bindings }
    }

    pub fn lookup(&self, key: Key) -> Option<Command> {
        self.bindings
            .iter()
            .find(|(k, _)| *k == key)
            .map(|&(_, cmd)| cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: Code) -> Key {
        Key {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    #[test]
    fn parses_names_and_modifiers() {
        assert_eq!(Key::parse("q").unwrap(), key(Code::Char('q')));
        assert_eq!(Key::parse("C").unwrap(), key(Code::Char('C')));
        assert_eq!(Key::parse("Space").unwrap(), key(Code::Char(' ')));
        assert_eq!(Key::parse("Enter").unwrap(), key(Code::Enter));
        assert_eq!(
            Key::parse("C-t").unwrap(),
            Key {
                ctrl: true,
                ..key(Code::Char('t'))
            }
        );
        assert_eq!(
            Key::parse("S-Tab").unwrap(),
            Key {
                shift: true,
                ..key(Code::Tab)
            }
        );
        assert_eq!(
            Key::parse("C-A-Left").unwrap(),
            Key {
                ctrl: true,
                alt: true,
                ..key(Code::Left)
            }
        );
        assert_eq!(Key::parse("-").unwrap(), key(Code::Char('-')));
        assert_eq!(
            Key::parse("C-").unwrap_err().to_string(),
            "unknown key name `C-`"
        );
        assert_eq!(
            Key::parse("C--").unwrap(),
            Key {
                ctrl: true,
                ..key(Code::Char('-'))
            }
        );
    }

    #[test]
    fn rejects_bad_names() {
        assert!(Key::parse("").is_err());
        assert!(Key::parse("Enterr").is_err());
        assert!(Key::parse("ab").is_err());
        assert!(Key::parse("S-x").is_err());
        assert!(Key::parse("X-a").is_err());
        assert!(Key::parse("aé").is_err());
        assert_eq!(Key::parse("é").unwrap(), key(Code::Char('é')));
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "q", "C", "?", "Space", "C-t", "S-Tab", "A-x", "C-A-Home", "Esc",
        ] {
            assert_eq!(Key::parse(s).unwrap().to_string(), s);
        }
    }

    #[test]
    fn events_map_to_keys() {
        let ev = |code, mods| Key::from_event(KeyEvent::new(code, mods)).unwrap();
        assert_eq!(
            ev(KeyCode::Char('c'), KeyModifiers::SHIFT),
            Key::parse("C").unwrap()
        );
        assert_eq!(
            ev(KeyCode::Char('C'), KeyModifiers::SHIFT),
            Key::parse("C").unwrap()
        );
        assert_eq!(
            ev(KeyCode::Char('?'), KeyModifiers::SHIFT),
            Key::parse("?").unwrap()
        );
        assert_eq!(
            ev(KeyCode::Char('t'), KeyModifiers::CONTROL),
            Key::parse("C-t").unwrap()
        );
        // Ctrl+Shift+x stays apart from Ctrl+x.
        assert_eq!(
            ev(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ),
            Key::parse("C-X").unwrap()
        );
        assert_eq!(
            ev(KeyCode::BackTab, KeyModifiers::SHIFT),
            Key::parse("S-Tab").unwrap()
        );
        assert_eq!(
            ev(KeyCode::Char(' '), KeyModifiers::NONE),
            Key::parse("Space").unwrap()
        );
        assert!(Key::from_event(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)).is_none());
    }

    #[test]
    fn defaults_cover_the_spec_table() {
        let km = Keymap::defaults();
        let look = |s: &str| km.lookup(Key::parse(s).unwrap()).map(Command::id);
        assert_eq!(look("i"), Some("cursor.up"));
        assert_eq!(look("j"), Some("nav.parent"));
        assert_eq!(look("l"), Some("nav.enter"));
        assert_eq!(look("C"), Some("path.copy"));
        assert_eq!(look("c"), Some("file.copy"));
        assert_eq!(look("S-Tab"), Some("focus.prev"));
        assert_eq!(look("C-t"), Some("term.toggle"));
        assert_eq!(look("h"), Some("view.hidden"));
        // No vim keys.
        assert_eq!(look("g"), None);
        assert_eq!(look("G"), None);
        // Every command has at least one default key.
        for &c in Command::ALL {
            assert!(
                km.bindings.iter().any(|&(_, b)| b == c),
                "{} unbound",
                c.id()
            );
        }
    }
}
