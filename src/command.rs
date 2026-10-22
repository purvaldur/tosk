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
    NavGoto => "nav.goto", "Go to a folder, or ssh://host/path";
    CursorTop => "cursor.top", "First item";
    CursorBottom => "cursor.bottom", "Last item";
    OpenDefault => "open.default", "Open with default app";
    OpenEditor => "open.editor", "Edit in Helix";
    FileCopy => "file.copy", "Copy file to clipboard";
    PathCopy => "path.copy", "Copy path to clipboard";
    FileRename => "file.rename", "Rename (bulk when several are selected)";
    FileNew => "file.new", "New file";
    FileNewFolder => "file.new_folder", "New folder";
    FileTrash => "file.trash", "Move to trash";
    FileDelete => "file.delete", "Delete permanently";
    SelectToggle => "select.toggle", "Select / unselect";
    SelectClear => "select.clear", "Clear selection and filter";
    ScriptRun => "script.run", "Run script in terminal panel";
    PickerFiles => "picker.files", "File picker";
    PickerGrep => "picker.grep", "Search file contents";
    FilterStart => "filter.start", "Filter this folder";
    TermToggle => "term.toggle", "Terminal: open and focus, again to hide";
    FocusNext => "focus.next", "Next area";
    FocusPrev => "focus.prev", "Previous area";
    FileProps => "file.props", "Properties";
    ShortcutPin => "shortcut.pin", "Pin or unpin shortcut";
    ViewPreview => "view.preview", "Toggle preview";
    ViewHidden => "view.hidden", "Toggle hidden files";
    ViewLog => "view.log", "Toggle log";
    ViewSort => "view.sort", "Sort by name, type, size or modified";
    ViewSortReverse => "view.sort_reverse", "Reverse the sort order";
    HelpOpen => "help.open", "Shortcuts for this context";
    AppQuit => "app.quit", "Quit";
}

/// What a key runs: a built-in command, or one from `[commands.*]` in the
/// config, by its index in `App::custom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Builtin(Command),
    Custom(usize),
}

impl From<Command> for Action {
    fn from(c: Command) -> Action {
        Action::Builtin(c)
    }
}

/// Runs a built-in or custom command.
pub fn run(app: &mut App, action: Action) {
    match action {
        Action::Builtin(cmd) => dispatch(app, cmd),
        Action::Custom(_) if app.focus == Focus::Sidebar => {
            let title = app.action_title(action);
            app.info(format!("{title} works on the file list; Tab over first"));
        }
        Action::Custom(i) => app.run_custom(i),
    }
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
    // In the sidebar, Space and Enter open the shortcut like l does.
    if app.focus == Focus::Sidebar && matches!(cmd, OpenDefault | SelectToggle) {
        return app.nav_enter();
    }
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
        NavGoto => app.start_goto(),
        FocusNext => app.cycle_focus(1),
        FocusPrev => app.cycle_focus(-1),
        ViewHidden => app.toggle_hidden(),
        ViewPreview => app.toggle_preview(),
        ViewLog => app.show_log = !app.show_log,
        ViewSort => app.cycle_sort(),
        ViewSortReverse => app.reverse_sort(),
        SelectToggle => app.toggle_select(),
        SelectClear => app.clear_selection(),
        OpenDefault => app.open_default(),
        FileCopy => app.copy_files(),
        PathCopy => app.copy_paths(),
        ShortcutPin => app.toggle_pin(),
        FileTrash => app.start_remove(false),
        FileDelete => app.start_remove(true),
        FileRename => app.start_rename(),
        FileNew => app.start_create(false),
        FileNewFolder => app.start_create(true),
        HelpOpen => app.open_help(),
        OpenEditor => app.open_editor(),
        TermToggle => app.toggle_terminal(),
        ScriptRun => app.run_script(),
        PickerFiles => app.open_picker(),
        PickerGrep => app.open_search(),
        FileProps => app.open_props(),
        FilterStart => {
            app.focus = Focus::Files;
            app.start_filter();
        }
        AppQuit => app.quit = true,
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
                "file.new",
                "file.new_folder",
                "file.trash",
                "file.delete",
                "select.toggle",
                "script.run",
                "file.props"
            ]
        );
    }
}
