//! The `?` menu: commands grouped by what they apply to, most relevant first.

use std::fs::File;
use std::io::Read;

use crate::app::{App, Focus};
use crate::command::Command::{self, *};
use crate::preview::{FileKind, classify};
use crate::text_input::TextInput;

pub struct HelpState {
    /// Index into the commands shown (not counting section titles).
    pub cursor: usize,
    pub filter: TextInput,
    /// `/` was pressed and the filter is being typed.
    pub typing: bool,
}

pub enum Row {
    Section(String),
    Command(Command),
}

/// The menu's rows for the current context, keeping only commands whose
/// title or id contains `query`. A command shows once, in the first section
/// that lists it, and a section with nothing left is dropped.
pub fn rows(app: &App, query: &str) -> Vec<Row> {
    let query = query.to_lowercase();
    let mut seen: Vec<Command> = Vec::new();
    let mut rows = Vec::new();
    for (title, cmds) in sections(app) {
        let fresh: Vec<Command> = cmds.into_iter().filter(|c| !seen.contains(c)).collect();
        seen.extend(&fresh);
        let shown: Vec<Command> = fresh
            .into_iter()
            .filter(|c| c.title().to_lowercase().contains(&query) || c.id().contains(&query))
            .collect();
        if !shown.is_empty() {
            rows.push(Row::Section(title));
            rows.extend(shown.into_iter().map(Row::Command));
        }
    }
    rows
}

pub fn commands(rows: &[Row]) -> Vec<Command> {
    rows.iter()
        .filter_map(|r| match r {
            Row::Command(c) => Some(*c),
            Row::Section(_) => None,
        })
        .collect()
}

fn sections(app: &App) -> Vec<(String, Vec<Command>)> {
    let mut out = Vec::new();
    if app.focus == Focus::Sidebar {
        out.push((
            "Sidebar".to_string(),
            vec![NavEnter, CursorUp, CursorDown, ShortcutPin],
        ));
    }
    if app.selection.len() >= 2 {
        out.push((
            format!("Selection · {} items", app.selection.len()),
            vec![
                FileProps,
                FileRename,
                FileCopy,
                PathCopy,
                FileTrash,
                FileDelete,
                SelectToggle,
                SelectClear,
            ],
        ));
    }
    if app.focus == Focus::Files
        && let Some(e) = app.current()
    {
        let mut cmds = if e.is_link() {
            vec![NavEnter, OpenEditor, PathCopy]
        } else if e.is_dir() {
            vec![
                NavEnter,
                OpenEditor,
                FileRename,
                PathCopy,
                ShortcutPin,
                FileTrash,
            ]
        } else if e.name.ends_with(".sh") {
            vec![
                ScriptRun,
                OpenEditor,
                OpenDefault,
                FileRename,
                FileCopy,
                PathCopy,
            ]
        } else if is_text(&app.dir.join(&e.file_name)) {
            vec![OpenEditor, OpenDefault, FileCopy, PathCopy, FileRename]
        } else {
            vec![OpenDefault, FileCopy, PathCopy, FileRename]
        };
        cmds.extend([FileProps, SelectToggle, FileTrash, FileDelete]);
        out.push((format!("On {}", e.name), cmds));
    }
    let folder = app
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.display_path(&app.dir));
    out.push((
        format!("In {folder}"),
        vec![
            FileNew,
            FileNewFolder,
            PickerFiles,
            PickerGrep,
            FilterStart,
            TermToggle,
            ViewHidden,
            ViewPreview,
            ViewLog,
            ViewSort,
            ViewSortReverse,
            ShortcutPin,
            NavParent,
        ],
    ));
    out.push((
        "Everywhere".to_string(),
        vec![
            FocusNext,
            FocusPrev,
            HelpOpen,
            AppQuit,
            CursorUp,
            CursorDown,
            CursorTop,
            CursorBottom,
            SelectClear,
        ],
    ));
    out
}

fn is_text(path: &std::path::Path) -> bool {
    let mut head = Vec::new();
    let read = File::open(path).and_then(|f| f.take(8 * 1024).read_to_end(&mut head));
    read.is_ok() && classify(path, &head) == FileKind::Text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn app_on(tmp: &TempDir, name: &str) -> App {
        let mut app = App::new(tmp.path().canonicalize().unwrap(), None, None).unwrap();
        app.cursor = app
            .visible
            .iter()
            .position(|&i| app.entries[i].name == name)
            .unwrap();
        app
    }

    fn titles(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Section(t) => Some(t.clone()),
                Row::Command(_) => None,
            })
            .collect()
    }

    fn setup() -> TempDir {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("build.sh"), "echo hi\n").unwrap();
        fs::write(tmp.path().join("notes.md"), "# hi\n").unwrap();
        fs::write(tmp.path().join("blob"), b"\0\x01").unwrap();
        fs::create_dir(tmp.path().join("src")).unwrap();
        tmp
    }

    #[test]
    fn scripts_list_run_script_first() {
        let tmp = setup();
        let app = app_on(&tmp, "build.sh");
        let rows = rows(&app, "");
        assert_eq!(titles(&rows)[0], "On build.sh");
        assert_eq!(commands(&rows)[0], ScriptRun);
        assert_eq!(ScriptRun.title(), "Run script in terminal panel");
    }

    #[test]
    fn order_depends_on_kind() {
        let tmp = setup();
        let first = |name: &str| commands(&rows(&app_on(&tmp, name), ""))[0];
        assert_eq!(first("notes.md"), OpenEditor);
        assert_eq!(first("blob"), OpenDefault);
        assert_eq!(first("src"), NavEnter);
    }

    #[test]
    fn each_command_once_in_its_first_section() {
        let tmp = setup();
        let mut app = app_on(&tmp, "notes.md");
        app.toggle_select();
        app.move_cursor(-1);
        app.toggle_select();
        let rows = rows(&app, "");
        assert_eq!(titles(&rows)[0], "Selection · 2 items");
        let cmds = commands(&rows);
        let mut unique = cmds.clone();
        unique.sort_by_key(|c| c.id());
        unique.dedup();
        assert_eq!(unique.len(), cmds.len());
        // Every command is reachable from the menu somewhere.
        for c in Command::ALL {
            // Running a script and entering a folder only apply to those.
            if !matches!(c, ScriptRun | NavEnter) {
                assert!(cmds.contains(c), "{} missing", c.id());
            }
        }
    }

    #[test]
    fn filter_matches_titles_and_ids() {
        let tmp = setup();
        let app = app_on(&tmp, "notes.md");
        let rows = rows(&app, "HIDDEN");
        assert_eq!(commands(&rows), [ViewHidden]);
        assert_eq!(
            commands(&super::rows(&app, "picker.")),
            [PickerFiles, PickerGrep]
        );
        assert!(super::rows(&app, "zzz").is_empty());
    }

    #[test]
    fn sidebar_section_leads_in_the_sidebar() {
        let tmp = setup();
        let mut app = app_on(&tmp, "notes.md");
        app.focus = Focus::Sidebar;
        let rows = rows(&app, "");
        assert_eq!(titles(&rows)[0], "Sidebar");
        assert!(!titles(&rows).iter().any(|t| t.starts_with("On ")));
    }
}
