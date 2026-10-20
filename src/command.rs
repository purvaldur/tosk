use crate::app::{App, Focus};

macro_rules! commands {
    ($($variant:ident => $id:literal, $title:literal;)*) => {
        /// Every action is a command; keys only point at command ids.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Command {
            $($variant),*
        }

        impl Command {
            pub const ALL: &[Command] = &[$(Command::$variant),*];

            pub fn id(self) -> &'static str {
                match self {
                    $(Command::$variant => $id),*
                }
            }

            pub fn title(self) -> &'static str {
                match self {
                    $(Command::$variant => $title),*
                }
            }
        }
    };
}

commands! {
    CursorUp => "cursor.up", "Move up";
    CursorDown => "cursor.down", "Move down";
    NavParent => "nav.parent", "Parent folder";
    NavEnter => "nav.enter", "Enter folder / follow link";
    CursorTop => "cursor.top", "First item";
    CursorBottom => "cursor.bottom", "Last item";
    OpenDefault => "open.default", "Open with default app";
    OpenEditor => "open.editor", "Edit in Helix";
    FileCopy => "file.copy", "Copy file to clipboard";
    PathCopy => "path.copy", "Copy path to clipboard";
    FileRename => "file.rename", "Rename (bulk when several are selected)";
    FileTrash => "file.trash", "Move to trash";
    FileDelete => "file.delete", "Delete permanently";
    SelectToggle => "select.toggle", "Select / unselect";
    SelectClear => "select.clear", "Clear selection and filter";
    ScriptRun => "script.run", "Run script in terminal panel";
    PickerRoot => "picker.root", "File picker from launch folder";
    PickerHere => "picker.here", "File picker from this folder";
    FilterStart => "filter.start", "Filter this folder";
    TermToggle => "term.toggle", "Terminal: open and focus, again to hide";
    FocusNext => "focus.next", "Next area";
    FocusPrev => "focus.prev", "Previous area";
    FileProps => "file.props", "Properties";
    ShortcutPin => "shortcut.pin", "Pin or unpin shortcut";
    ViewPreview => "view.preview", "Toggle preview";
    ViewHidden => "view.hidden", "Toggle hidden files";
    HelpOpen => "help.open", "Shortcuts for this context";
    AppQuit => "app.quit", "Quit";
}

impl Command {
    pub fn from_id(id: &str) -> Option<Command> {
        Command::ALL.iter().copied().find(|c| c.id() == id)
    }

    /// `open.*`, `file.*`, `path.copy`, `select.toggle` and `script.run`.
    pub fn acts_on_files(self) -> bool {
        let id = self.id();
        id.starts_with("open.")
            || id.starts_with("file.")
            || matches!(id, "path.copy" | "select.toggle" | "script.run")
    }
}

pub fn dispatch(app: &mut App, cmd: Command) {
    use Command::*;
    if app.focus == Focus::Sidebar && cmd.acts_on_files() {
        return app.info(format!(
            "{} works on the file list; Tab over first",
            cmd.title()
        ));
    }
    match cmd {
        CursorUp => app.move_cursor(-1),
        CursorDown => app.move_cursor(1),
        CursorTop => app.cursor_to_top(),
        CursorBottom => app.cursor_to_bottom(),
        NavParent => app.nav_parent(),
        NavEnter => app.nav_enter(),
        FocusNext => app.cycle_focus(1),
        FocusPrev => app.cycle_focus(-1),
        ViewHidden => app.toggle_hidden(),
        ViewPreview => app.toggle_preview(),
        SelectToggle => app.toggle_select(),
        SelectClear => app.clear_selection(),
        OpenDefault => app.open_default(),
        FileCopy => app.copy_files(),
        PathCopy => app.copy_paths(),
        ShortcutPin => app.toggle_pin(),
        AppQuit => app.quit = true,
        _ => app.info(format!("{} isn't built yet", cmd.title())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for &c in Command::ALL {
            assert_eq!(Command::from_id(c.id()), Some(c));
        }
        assert_eq!(Command::from_id("nope"), None);
    }

    #[test]
    fn file_commands() {
        let on_files: Vec<_> = Command::ALL
            .iter()
            .filter(|c| c.acts_on_files())
            .map(|c| c.id())
            .collect();
        assert_eq!(
            on_files,
            [
                "open.default",
                "open.editor",
                "file.copy",
                "path.copy",
                "file.rename",
                "file.trash",
                "file.delete",
                "select.toggle",
                "script.run",
                "file.props"
            ]
        );
    }
}
