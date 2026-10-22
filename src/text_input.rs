//! A one-line text input: the filter, rename and (later) picker fields.

use crate::keymap::{Code, Key};

#[derive(Clone, Debug, Default)]
pub struct TextInput {
    pub text: String,
    /// Byte offset, always on a char boundary.
    cursor: usize,
}

impl TextInput {
    /// Starts with `text` and the cursor at its end.
    pub fn new(text: &str) -> TextInput {
        TextInput {
            text: text.to_string(),
            cursor: text.len(),
        }
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// The text before the cursor, for placing a caret.
    pub fn before_cursor(&self) -> &str {
        &self.text[..self.cursor]
    }

    /// Edits for `key`. Returns false for keys an input doesn't handle.
    pub fn handle(&mut self, key: Key) -> bool {
        match (key.code, key.ctrl, key.alt) {
            (Code::Char('u'), true, false) => self.clear(),
            (Code::Char('w'), true, false) => self.delete_word(),
            (Code::Char(c), false, false) => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            (Code::Backspace, false, false) => {
                if let Some(c) = self.before_cursor().chars().next_back() {
                    self.cursor -= c.len_utf8();
                    self.text.remove(self.cursor);
                }
            }
            (Code::Left, false, false) => {
                if let Some(c) = self.before_cursor().chars().next_back() {
                    self.cursor -= c.len_utf8();
                }
            }
            (Code::Right, false, false) => {
                if let Some(c) = self.text[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            (Code::Home, false, false) => self.cursor = 0,
            (Code::End, false, false) => self.cursor = self.text.len(),
            _ => return false,
        }
        true
    }

    /// Like Helix: skip non-word characters, then delete one word, where a
    /// word is letters, digits and `_`. `show.s01.e05` loses `e05` first.
    fn delete_word(&mut self) {
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let before = self.before_cursor();
        let trimmed = before.trim_end_matches(|c| !is_word(c));
        let start = trimmed.trim_end_matches(is_word).len();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(input: &mut TextInput, keys: &[&str]) {
        for k in keys {
            assert!(input.handle(Key::parse(k).unwrap()), "{k} not handled");
        }
    }

    #[test]
    fn typing_and_editing() {
        let mut t = TextInput::default();
        press(&mut t, &["a", "b", "é", "Space", "C"]);
        assert_eq!(t.text, "abé C");
        press(
            &mut t,
            &[
                "Backspace",
                "Backspace",
                "Left",
                "x",
                "Home",
                ">",
                "End",
                "!",
            ],
        );
        assert_eq!(t.text, ">abxé!");
        assert_eq!(t.before_cursor(), ">abxé!");
        press(&mut t, &["C-u"]);
        assert_eq!(t.text, "");
        press(&mut t, &["Backspace", "Left", "Right"]);
        assert_eq!(t.text, "");
    }

    #[test]
    fn delete_word() {
        let mut t = TextInput::new("show.s01.e05.mkv");
        press(&mut t, &["C-w"]);
        assert_eq!(t.text, "show.s01.e05.");
        press(&mut t, &["C-w"]);
        assert_eq!(t.text, "show.s01.");
        press(&mut t, &["C-w", "C-w", "C-w"]);
        assert_eq!(t.text, "");
        let mut t = TextInput::new("my file");
        press(&mut t, &["Left", "Left", "C-w"]);
        assert_eq!(t.text, "my le");
    }

    #[test]
    fn leaves_other_keys_alone() {
        let mut t = TextInput::new("x");
        for k in ["Enter", "Esc", "Tab", "C-f", "Up"] {
            assert!(!t.handle(Key::parse(k).unwrap()));
        }
        assert_eq!(t.text, "x");
    }
}
