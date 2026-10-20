use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use jiff::Zoned;
use jiff::tz::TimeZone;
use ratatui_image::picker::Picker;

use crate::counts::{Counted, Counter};
use crate::entry::{Entry, Kind};
use crate::format::{count, selection_summary, tilde};
use crate::fs::Vfs;
use crate::fs::local::Local;
use crate::keymap::Keymap;
use crate::preview::{self, Previewer};
use crate::shortcuts::{Pin, Shortcuts};
use crate::sort::sort_entries;
use crate::{clipboard, launch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Files,
}

/// What `Tab` and `S-Tab` cycle through. The terminal (milestone 6) stays
/// out of it on purpose.
const FOCUS_CYCLE: &[Focus] = &[Focus::Sidebar, Focus::Files];

/// A selected item. The entry is a snapshot, so the selection can span
/// folders.
pub struct Selected {
    pub path: PathBuf,
    pub entry: Entry,
}

pub struct Message {
    pub text: String,
    pub error: bool,
}

pub struct App {
    pub dir: PathBuf,
    /// Every entry in `dir`, sorted, hidden ones included.
    pub entries: Vec<Entry>,
    /// Indexes into `entries` that are shown.
    pub visible: Vec<usize>,
    /// Index into `visible`.
    pub cursor: usize,
    pub focus: Focus,
    pub shortcuts: Shortcuts,
    pub side_cursor: usize,
    /// In the order things were selected; bulk rename numbers in this order.
    pub selection: Vec<Selected>,
    pub show_hidden: bool,
    pub preview: bool,
    /// What the preview pane should show, set before each draw.
    pub preview_key: Option<preview::Key>,
    pub previewer: Previewer,
    /// Newest first, at most two.
    pub messages: VecDeque<Message>,
    pub keymap: Keymap,
    pub quit: bool,
    pub home: Option<PathBuf>,
    pub tz: TimeZone,
    pub this_year: i16,
    /// Name under the cursor, per folder, for the session.
    remembered: HashMap<PathBuf, OsString>,
    counter: Counter,
    vfs: Arc<dyn Vfs>,
}

impl App {
    /// `picker` is set when the terminal speaks kitty graphics. Pinned
    /// shortcuts are kept in `pins`, or only in memory when it's `None`.
    pub fn new(dir: PathBuf, picker: Option<Picker>, pins: Option<PathBuf>) -> Result<App> {
        let now = Zoned::now();
        let home = dirs::home_dir();
        let vfs: Arc<dyn Vfs> = Arc::new(Local);
        let mut app = App {
            dir: dir.clone(),
            entries: Vec::new(),
            visible: Vec::new(),
            cursor: 0,
            focus: Focus::Files,
            shortcuts: Shortcuts::load(home.as_deref(), pins),
            side_cursor: 0,
            selection: Vec::new(),
            show_hidden: false,
            preview: true,
            preview_key: None,
            previewer: Previewer::spawn(vfs.clone(), picker),
            messages: VecDeque::new(),
            keymap: Keymap::defaults(),
            quit: false,
            home,
            tz: now.time_zone().clone(),
            this_year: now.year(),
            remembered: HashMap::new(),
            counter: Counter::spawn(vfs.clone()),
            vfs,
        };
        app.load(dir.clone(), None)
            .with_context(|| format!("can't open {}", dir.display()))?;
        Ok(app)
    }

    pub fn current(&self) -> Option<&Entry> {
        self.visible.get(self.cursor).map(|&i| &self.entries[i])
    }

    /// The shortcut to highlight: the deepest one containing the current folder.
    pub fn here(&self) -> Option<usize> {
        self.shortcuts.deepest_match(&self.dir)
    }

    pub fn display_path(&self, path: &Path) -> String {
        tilde(path, self.home.as_deref())
    }

    pub fn info(&mut self, text: String) {
        self.push_message(text, false);
    }

    pub fn error(&mut self, text: String) {
        self.push_message(text, true);
    }

    fn push_message(&mut self, text: String, error: bool) {
        self.messages.push_front(Message { text, error });
        self.messages.truncate(2);
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let (cursor, len) = self.focused_list();
        *cursor = cursor
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }

    pub fn cursor_to_top(&mut self) {
        *self.focused_list().0 = 0;
    }

    pub fn cursor_to_bottom(&mut self) {
        let (cursor, len) = self.focused_list();
        *cursor = len.saturating_sub(1);
    }

    /// The cursor and length of the list that has focus.
    fn focused_list(&mut self) -> (&mut usize, usize) {
        match self.focus {
            Focus::Sidebar => (&mut self.side_cursor, self.shortcuts.items.len()),
            Focus::Files => (&mut self.cursor, self.visible.len()),
        }
    }

    pub fn cycle_focus(&mut self, step: isize) {
        let n = FOCUS_CYCLE.len() as isize;
        let i = FOCUS_CYCLE
            .iter()
            .position(|&f| f == self.focus)
            .unwrap_or(0) as isize;
        self.focus = FOCUS_CYCLE[(i + step).rem_euclid(n) as usize];
    }

    pub fn nav_parent(&mut self) {
        if self.focus == Focus::Sidebar {
            return;
        }
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else {
            return;
        };
        let came_from = self.dir.file_name().map(OsStr::to_os_string);
        self.go(parent, came_from);
    }

    /// Movement only: folders and links are followed, plain files do nothing.
    /// In the sidebar, jumps to the shortcut and hands focus back to the list.
    pub fn nav_enter(&mut self) {
        if self.focus == Focus::Sidebar {
            let Some(shortcut) = self.shortcuts.items.get(self.side_cursor) else {
                return;
            };
            if self.go(shortcut.path.clone(), None) {
                self.focus = Focus::Files;
            }
            return;
        }
        let Some(e) = self.current() else { return };
        let path = self.dir.join(&e.file_name);
        let name = e.name.clone();
        match (e.kind, e.is_link()) {
            (Kind::Dir, false) => {
                self.go(path, None);
            }
            (Kind::Dir, true) => {
                let target = match self.vfs.resolve(&path) {
                    Ok(t) => t,
                    Err(err) => return self.error(format!("can't follow {name}: {err}")),
                };
                if self.go(target.clone(), None) {
                    let shown = self.display_path(&target);
                    self.info(format!("followed {name} → {shown}"));
                }
            }
            (Kind::File | Kind::Other, true) => {
                let target = match self.vfs.resolve(&path) {
                    Ok(t) => t,
                    Err(err) => return self.error(format!("can't follow {name}: {err}")),
                };
                let (Some(parent), Some(file_name)) = (target.parent(), target.file_name()) else {
                    return;
                };
                if file_name.to_string_lossy().starts_with('.') {
                    self.show_hidden = true;
                }
                self.go(parent.to_path_buf(), Some(file_name.to_os_string()));
            }
            _ => {}
        }
    }

    pub fn is_selected(&self, e: &Entry) -> bool {
        let path = self.dir.join(&e.file_name);
        self.selection.iter().any(|s| s.path == path)
    }

    /// Select or unselect the item under the cursor. The cursor stays put.
    pub fn toggle_select(&mut self) {
        let Some(e) = self.current() else { return };
        let path = self.dir.join(&e.file_name);
        match self.selection.iter().position(|s| s.path == path) {
            Some(i) => {
                self.selection.remove(i);
            }
            None => {
                let entry = e.clone();
                self.selection.push(Selected { path, entry });
            }
        }
    }

    /// Clears the selection (and, from milestone 4, the filter).
    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    pub fn selection_summary(&self) -> String {
        let entries: Vec<&Entry> = self.selection.iter().map(|s| &s.entry).collect();
        selection_summary(&entries)
    }

    /// What file commands act on: the selection, or else the cursor item.
    pub fn targets(&self) -> Vec<PathBuf> {
        if self.selection.is_empty() {
            self.current()
                .map(|e| vec![self.dir.join(&e.file_name)])
                .unwrap_or_default()
        } else {
            self.selection.iter().map(|s| s.path.clone()).collect()
        }
    }

    /// Opens the cursor item with the default app; a folder is entered instead.
    pub fn open_default(&mut self) {
        let Some(e) = self.current() else { return };
        if e.is_dir() {
            return self.nav_enter();
        }
        let path = self.dir.join(&e.file_name);
        let name = e.name.clone();
        match launch::xdg_open(&path) {
            Ok(()) => self.info(format!("opened {name} with xdg-open")),
            Err(err) => self.error(format!("can't run xdg-open: {err}")),
        }
    }

    /// The files themselves, as a uri-list GUI apps paste as files.
    pub fn copy_files(&mut self) {
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let paths: Vec<&Path> = targets.iter().map(PathBuf::as_path).collect();
        match clipboard::copy(Some("text/uri-list"), &clipboard::uri_list(&paths)) {
            Ok(()) => self.info(format!(
                "{} on the clipboard; pastes as files in GUI apps",
                count(paths.len(), "file", "files")
            )),
            Err(err) => self.error(format!("can't copy: {err}")),
        }
    }

    /// The paths as text, shell-quoted where needed.
    pub fn copy_paths(&mut self) {
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let paths: Vec<&Path> = targets.iter().map(PathBuf::as_path).collect();
        let text = clipboard::shell_words(&paths);
        match clipboard::copy(None, &text) {
            Ok(()) if paths.len() == 1 => {
                let shown = self.display_path(paths[0]);
                self.info(format!("copied {shown}"));
            }
            Ok(()) => self.info(format!("copied {} paths", paths.len())),
            Err(err) => self.error(format!("can't copy: {err}")),
        }
    }

    /// In the file list: pin the folder under the cursor, or the current
    /// folder when the cursor is on a file. In the sidebar: unpin.
    pub fn toggle_pin(&mut self) {
        let path = match self.focus {
            Focus::Sidebar => match self.shortcuts.items.get(self.side_cursor) {
                Some(s) => s.path.clone(),
                None => return,
            },
            Focus::Files => match self.current() {
                Some(e) if e.is_dir() => self.dir.join(&e.file_name),
                _ => self.dir.clone(),
            },
        };
        let shown = self.display_path(&path);
        match self.shortcuts.toggle_pin(&path) {
            Ok(Pin::Pinned) => self.info(format!("pinned {shown} to the shortcuts")),
            Ok(Pin::Unpinned) => self.info(format!("unpinned {shown}")),
            Ok(Pin::BuiltIn) => self.info(format!("{shown} is a built-in shortcut")),
            Err(err) => self.error(format!("can't save shortcuts: {err}")),
        }
        self.side_cursor = self
            .side_cursor
            .min(self.shortcuts.items.len().saturating_sub(1));
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let anchor = self.visible.get(self.cursor).copied().unwrap_or(0);
        let name = self.current().map(|e| e.file_name.clone());
        self.refilter(name.as_deref(), anchor);
    }

    pub fn toggle_preview(&mut self) {
        self.preview = !self.preview;
    }

    /// Point the preview at the item under the cursor and ask the worker for
    /// it. `size` is the preview body in cells, or `None` while it's hidden.
    pub fn sync_preview(&mut self, size: Option<(u16, u16)>) {
        self.preview_key = match (size, self.current()) {
            (Some(size), Some(e)) => Some(preview::Key {
                path: self.dir.join(&e.file_name),
                mtime: e.mtime,
                size,
            }),
            _ => None,
        };
        if let Some(key) = &self.preview_key {
            self.previewer.want(key);
        }
    }

    /// Takes what the workers have finished; returns whether anything changed.
    pub fn drain_workers(&mut self) -> bool {
        let mut changed = self.previewer.drain();
        while let Ok(counted) = self.counter.results.try_recv() {
            changed |= self.apply_counts(counted);
        }
        changed
    }

    fn apply_counts(&mut self, counted: Counted) -> bool {
        if counted.dir != self.dir {
            return false;
        }
        let index: HashMap<OsString, usize> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.file_name.clone(), i))
            .collect();
        for (name, n) in counted.counts {
            if let Some(&i) = index.get(&name) {
                self.entries[i].children = n;
            }
            let path = counted.dir.join(&name);
            if let Some(s) = self.selection.iter_mut().find(|s| s.path == path) {
                s.entry.children = n;
            }
        }
        true
    }

    /// Switch folders, reporting a failure as a message. Returns whether it worked.
    fn go(&mut self, dir: PathBuf, focus: Option<OsString>) -> bool {
        match self.load(dir.clone(), focus) {
            Ok(()) => true,
            Err(err) => {
                let shown = self.display_path(&dir);
                self.error(format!("can't open {shown}: {err}"));
                false
            }
        }
    }

    fn load(&mut self, dir: PathBuf, focus: Option<OsString>) -> io::Result<()> {
        let mut entries = self.vfs.list(&dir)?;
        sort_entries(&mut entries);
        if let Some(e) = self.current() {
            self.remembered
                .insert(self.dir.clone(), e.file_name.clone());
        }
        let focus = focus.or_else(|| self.remembered.get(&dir).cloned());
        let folders = entries
            .iter()
            .filter(|e| e.is_dir())
            .map(|e| e.file_name.clone())
            .collect();
        self.counter.request(&dir, folders);
        self.dir = dir;
        self.entries = entries;
        self.refilter(focus.as_deref(), 0);
        Ok(())
    }

    /// Recompute `visible` and put the cursor on `focus`, or else on the
    /// first shown entry at or after `anchor` in `entries`.
    fn refilter(&mut self, focus: Option<&OsStr>, anchor: usize) {
        let show_hidden = self.show_hidden;
        self.visible = (0..self.entries.len())
            .filter(|&i| show_hidden || !self.entries[i].is_hidden())
            .collect();
        let entries = &self.entries;
        self.cursor = focus
            .and_then(|f| self.visible.iter().position(|&i| entries[i].file_name == f))
            .or_else(|| self.visible.iter().position(|&i| i >= anchor))
            .unwrap_or(self.visible.len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn tree() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("a/inner")).unwrap();
        fs::create_dir(r.join("b")).unwrap();
        fs::write(r.join("b/x.txt"), "x").unwrap();
        fs::write(r.join("b/y.txt"), "y").unwrap();
        fs::write(r.join("b/.z"), "z").unwrap();
        fs::write(r.join("file.txt"), "f").unwrap();
        fs::write(r.join(".dot"), "").unwrap();
        symlink("b", r.join("link-b")).unwrap();
        symlink("b/y.txt", r.join("link-y")).unwrap();
        symlink("b/.z", r.join("link-z")).unwrap();
        tmp
    }

    fn name(app: &App) -> &str {
        &app.current().unwrap().name
    }

    fn select(app: &mut App, n: &str) {
        app.cursor = app
            .visible
            .iter()
            .position(|&i| app.entries[i].name == n)
            .unwrap();
    }

    fn root(tmp: &TempDir) -> PathBuf {
        tmp.path().canonicalize().unwrap()
    }

    #[test]
    fn hidden_files_toggle() {
        let tmp = tree();
        let mut app = App::new(root(&tmp), None, None).unwrap();
        let names: Vec<_> = app.visible.iter().map(|&i| &app.entries[i].name).collect();
        assert_eq!(names, ["a", "b", "link-b", "file.txt", "link-y", "link-z"]);
        app.toggle_hidden();
        assert_eq!(app.visible.len(), 7);
        select(&mut app, ".dot");
        app.toggle_hidden();
        // The cursor lands on the next shown entry.
        assert_eq!(name(&app), "file.txt");
    }

    #[test]
    fn enter_and_parent_remember_the_cursor() {
        let tmp = tree();
        let r = root(&tmp);
        let mut app = App::new(r.clone(), None, None).unwrap();
        select(&mut app, "b");
        app.nav_enter();
        assert_eq!(app.dir, r.join("b"));
        app.move_cursor(1);
        assert_eq!(name(&app), "y.txt");
        app.nav_parent();
        assert_eq!(app.dir, r);
        assert_eq!(name(&app), "b");
        app.nav_enter();
        assert_eq!(name(&app), "y.txt");
    }

    #[test]
    fn enter_on_a_file_does_nothing() {
        let tmp = tree();
        let r = root(&tmp);
        let mut app = App::new(r.clone(), None, None).unwrap();
        select(&mut app, "file.txt");
        app.nav_enter();
        assert_eq!(app.dir, r);
        assert_eq!(name(&app), "file.txt");
        assert!(app.messages.is_empty());
    }

    #[test]
    fn links_are_followed() {
        let tmp = tree();
        let r = root(&tmp);
        let mut app = App::new(r.clone(), None, None).unwrap();

        select(&mut app, "link-b");
        app.nav_enter();
        assert_eq!(app.dir, r.join("b"));
        assert!(app.messages[0].text.starts_with("followed link-b → "));

        app.nav_parent();
        select(&mut app, "link-y");
        app.nav_enter();
        assert_eq!(app.dir, r.join("b"));
        assert_eq!(name(&app), "y.txt");

        app.nav_parent();
        select(&mut app, "link-z");
        app.nav_enter();
        assert!(app.show_hidden);
        assert_eq!(name(&app), ".z");
    }

    #[test]
    fn cursor_clamps() {
        let tmp = tree();
        let mut app = App::new(root(&tmp), None, None).unwrap();
        app.move_cursor(-1);
        assert_eq!(app.cursor, 0);
        app.move_cursor(100);
        assert_eq!(name(&app), "link-z");
        app.cursor_to_top();
        assert_eq!(name(&app), "a");
        app.cursor_to_bottom();
        assert_eq!(name(&app), "link-z");
    }

    #[test]
    fn empty_folder_and_root() {
        let tmp = tree();
        let mut app = App::new(root(&tmp).join("a/inner"), None, None).unwrap();
        assert!(app.current().is_none());
        app.move_cursor(1);
        app.nav_enter();
        assert_eq!(app.cursor, 0);

        let mut app = App::new(PathBuf::from("/"), None, None).unwrap();
        app.nav_parent();
        assert_eq!(app.dir, Path::new("/"));
    }

    #[test]
    fn unreadable_folder_keeps_you_in_place() {
        let tmp = tree();
        let r = root(&tmp);
        let mut app = App::new(r.clone(), None, None).unwrap();
        select(&mut app, "b");
        fs::remove_dir_all(r.join("b")).unwrap();
        app.nav_enter();
        assert_eq!(app.dir, r);
        assert!(app.messages[0].error);
    }

    /// Home and a pinned `b`, with pins kept in memory.
    fn with_shortcuts(tmp: &TempDir) -> App {
        let r = root(tmp);
        let mut app = App::new(r.clone(), None, None).unwrap();
        app.shortcuts = Shortcuts::build(Some(&r), [].into_iter(), vec![r.join("b")], None);
        app
    }

    #[test]
    fn tab_cycles_focus() {
        let tmp = tree();
        let mut app = with_shortcuts(&tmp);
        assert_eq!(app.focus, Focus::Files);
        app.cycle_focus(1);
        assert_eq!(app.focus, Focus::Sidebar);
        app.cycle_focus(1);
        assert_eq!(app.focus, Focus::Files);
        app.cycle_focus(-1);
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn sidebar_jumps_and_returns_focus() {
        let tmp = tree();
        let r = root(&tmp);
        let mut app = with_shortcuts(&tmp);
        assert_eq!(app.here(), Some(0));
        app.focus = Focus::Sidebar;
        app.move_cursor(1);
        app.move_cursor(1);
        assert_eq!(app.side_cursor, 1);
        // j does nothing in the sidebar, and the file cursor stays put.
        app.nav_parent();
        assert_eq!((app.dir.clone(), app.cursor), (r.clone(), 0));
        app.nav_enter();
        assert_eq!(app.dir, r.join("b"));
        assert_eq!(app.focus, Focus::Files);
        assert_eq!(app.here(), Some(1));
    }

    #[test]
    fn counts_fill_in_from_the_worker() {
        let tmp = tree();
        let mut app = App::new(root(&tmp), None, None).unwrap();
        for _ in 0..500 {
            app.drain_workers();
            if app
                .entries
                .iter()
                .filter(|e| e.is_dir())
                .all(|e| e.children.is_some())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let b = app.entries.iter().find(|e| e.name == "b").unwrap();
        assert_eq!(b.children, Some(3));
        let link = app.entries.iter().find(|e| e.name == "link-b").unwrap();
        assert_eq!(link.children, Some(3));
    }

    fn names(app: &App) -> Vec<String> {
        app.selection.iter().map(|s| s.entry.name.clone()).collect()
    }

    #[test]
    fn selection_keeps_order_and_spans_folders() {
        let tmp = tree();
        let r = root(&tmp);
        let mut app = App::new(r.clone(), None, None).unwrap();
        select(&mut app, "file.txt");
        app.toggle_select();
        // The cursor doesn't move.
        assert_eq!(name(&app), "file.txt");
        select(&mut app, "a");
        app.toggle_select();
        assert!(app.is_selected(app.current().unwrap()));

        select(&mut app, "b");
        app.nav_enter();
        select(&mut app, "y.txt");
        app.toggle_select();
        select(&mut app, "x.txt");
        app.toggle_select();
        assert_eq!(names(&app), ["file.txt", "a", "y.txt", "x.txt"]);
        assert_eq!(
            app.targets(),
            [
                r.join("file.txt"),
                r.join("a"),
                r.join("b/y.txt"),
                r.join("b/x.txt")
            ]
        );

        // Unselecting keeps the others in order.
        select(&mut app, "y.txt");
        app.toggle_select();
        assert_eq!(names(&app), ["file.txt", "a", "x.txt"]);

        app.clear_selection();
        assert!(app.selection.is_empty());
        assert_eq!(app.targets(), [r.join("b/y.txt")]);
    }

    #[test]
    fn summary_uses_counts_from_the_worker() {
        let tmp = tree();
        let mut app = App::new(root(&tmp), None, None).unwrap();
        select(&mut app, "b");
        app.toggle_select();
        for _ in 0..500 {
            app.drain_workers();
            if app.selection[0].entry.children.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(app.selection_summary(), "“b” selected (containing 3 items)");
        select(&mut app, "file.txt");
        app.toggle_select();
        assert_eq!(
            app.selection_summary(),
            "1 folder selected (containing 3 items), 1 other item selected (1 byte)"
        );
    }

    #[test]
    fn open_default_enters_folders() {
        let tmp = tree();
        let r = root(&tmp);
        let mut app = App::new(r.clone(), None, None).unwrap();
        select(&mut app, "link-b");
        app.open_default();
        assert_eq!(app.dir, r.join("b"));
    }

    #[test]
    fn pinning_from_list_and_sidebar() {
        let tmp = tree();
        let r = root(&tmp);
        let mut app = with_shortcuts(&tmp);
        let labels = |app: &App| -> Vec<String> {
            app.shortcuts
                .items
                .iter()
                .map(|s| s.label.clone())
                .collect()
        };
        assert_eq!(labels(&app), ["Home", "b"]);

        // On a folder: pins that folder.
        select(&mut app, "a");
        app.toggle_pin();
        assert_eq!(labels(&app), ["Home", "b", "a"]);
        // On a file: pins the current folder, which here is Home.
        select(&mut app, "file.txt");
        app.toggle_pin();
        assert!(app.messages[0].text.ends_with("is a built-in shortcut"));

        // In the sidebar: unpins the shortcut under the cursor.
        app.focus = Focus::Sidebar;
        app.cursor_to_bottom();
        app.toggle_pin();
        assert_eq!(labels(&app), ["Home", "b"]);
        assert_eq!(app.side_cursor, 1);
        assert_eq!(app.shortcuts.items[1].path, r.join("b"));
    }
}
