use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use jiff::Zoned;
use jiff::tz::TimeZone;
use ratatui_image::picker::Picker;

use crate::command::Command;
use crate::counts::{Counted, Counter};
use crate::entry::{Entry, Kind};
use crate::format::{count, selection_summary, tilde};
use crate::fs::Vfs;
use crate::fs::local::Local;
use crate::git::{GitStatus, GitWatcher};
use crate::grep::ContentSearch;
use crate::help::HelpState;
use crate::keymap::Keymap;
use crate::kitty::{Kitty, Opened};
use crate::modal::Modal;
use crate::picker::FilePicker;
use crate::preview::{self, FileKind, Previewer, classify};
use crate::props::Props;
use crate::rename;
use crate::shortcuts::{Pin, Shortcuts};
use crate::sort::{Sort, sort_entries};
use crate::term_panel::TermPanel;
use crate::text_input::TextInput;
use crate::{clipboard, launch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Files,
    /// The terminal panel; keys go to the shell. Not in the `Tab` cycle.
    Terminal,
}

/// How often the git segment is refreshed while nothing else asks.
const GIT_REFRESH: Duration = Duration::from_secs(5);

/// What `Tab` and `S-Tab` cycle through. The terminal (milestone 6) stays
/// out of it on purpose.
const FOCUS_CYCLE: &[Focus] = &[Focus::Sidebar, Focus::Files];

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
    /// Names in the current folder, in the order they were selected (bulk
    /// rename numbers in this order). Only ever holds what's visible: it
    /// clears when the folder changes and loses what a filter hides.
    pub selection: Vec<OsString>,
    pub filter: TextInput,
    /// The filter is being typed.
    pub filtering: bool,
    pub modal: Option<Modal>,
    /// The shell, started on first use and kept while the panel is hidden.
    pub terminal: Option<TermPanel>,
    pub terminal_open: bool,
    /// Where focus goes back to when the terminal hides.
    focus_before_terminal: Focus,
    /// What the panel runs: `$SHELL`, or `/bin/sh`.
    pub shell: std::ffi::OsString,
    /// The panel's last known size in cells, (rows, cols).
    terminal_size: (u16, u16),
    pub show_hidden: bool,
    pub sort: Sort,
    pub preview: bool,
    /// The two-line message area at the bottom; `L` hides it.
    pub show_log: bool,
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
    /// The work tree `dir` is in, for the segment and the file marks;
    /// `None` outside one.
    pub git: Option<GitStatus>,
    git_watch: GitWatcher,
    /// When git was last asked, for the periodic refresh.
    git_asked: Instant,
    /// Results of fire-and-forget jobs (kitty calls), as log messages.
    jobs: (Sender<Message>, Receiver<Message>),
    pub(crate) vfs: Arc<dyn Vfs>,
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
            filter: TextInput::default(),
            filtering: false,
            modal: None,
            terminal: None,
            terminal_open: false,
            focus_before_terminal: Focus::Files,
            shell: std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into()),
            terminal_size: (6, 80),
            show_hidden: false,
            sort: Sort::default(),
            preview: true,
            show_log: true,
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
            git: None,
            git_watch: GitWatcher::spawn(),
            git_asked: Instant::now(),
            jobs: mpsc::channel(),
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
            Focus::Files | Focus::Terminal => (&mut self.cursor, self.visible.len()),
        }
    }

    /// `Ctrl-s`: hidden → show and focus; shown → focus; focused → hide and
    /// give focus back.
    pub fn toggle_terminal(&mut self) {
        if self.focus == Focus::Terminal {
            self.terminal_open = false;
            self.focus = self.focus_before_terminal;
        } else {
            self.focus_terminal();
        }
    }

    /// Shows and focuses the panel, starting the shell in the current folder
    /// if there isn't one. Returns whether there's a shell to talk to.
    fn focus_terminal(&mut self) -> bool {
        if self.terminal.is_none() {
            let (rows, cols) = self.terminal_size;
            match TermPanel::spawn(&self.shell, &self.dir, rows, cols) {
                Ok(t) => self.terminal = Some(t),
                Err(e) => {
                    self.error(format!("can't start a shell: {e}"));
                    return false;
                }
            }
        }
        self.terminal_open = true;
        if self.focus != Focus::Terminal {
            self.focus_before_terminal = self.focus;
            self.focus = Focus::Terminal;
        }
        true
    }

    /// `x`: run the `.sh` file under the cursor in the terminal panel, so its
    /// output stays in the scrollback.
    pub fn run_script(&mut self) {
        let Some(e) = self.current() else { return };
        let target_is_sh = e
            .link_target
            .as_ref()
            .and_then(|t| t.file_name())
            .is_some_and(|n| n.to_string_lossy().ends_with(".sh"));
        if e.is_dir() || !(e.name.ends_with(".sh") || target_is_sh) {
            return self.error("x runs .sh files".into());
        }
        let path = self.dir.join(&e.file_name);
        if self.focus_terminal()
            && let Some(t) = &mut self.terminal
        {
            let line = format!("bash {}\r", clipboard::shell_quote(&path.to_string_lossy()));
            t.write(line.as_bytes());
        }
    }

    /// Keeps the pty the size of the panel's body, (rows, cols).
    pub fn sync_terminal_size(&mut self, size: Option<(u16, u16)>) {
        let Some(size) = size else { return };
        self.terminal_size = size;
        if let Some(t) = &mut self.terminal {
            t.resize(size.0, size.1);
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
        self.selection.contains(&e.file_name)
    }

    /// Select or unselect the item under the cursor. The cursor stays put.
    pub fn toggle_select(&mut self) {
        let Some(name) = self.current().map(|e| e.file_name.clone()) else {
            return;
        };
        match self.selection.iter().position(|n| *n == name) {
            Some(i) => {
                self.selection.remove(i);
            }
            None => self.selection.push(name),
        }
    }

    /// Clears the selection and the filter.
    pub fn clear_selection(&mut self) {
        self.selection.clear();
        if !self.filter.text.is_empty() {
            self.filter.clear();
            self.refilter_here();
        }
    }

    pub fn selection_summary(&self) -> String {
        let entries: Vec<&Entry> = self
            .selection
            .iter()
            .filter_map(|n| self.entries.iter().find(|e| e.file_name == *n))
            .collect();
        selection_summary(&entries)
    }

    /// What file commands act on: the selection, or else the cursor item.
    pub fn targets(&self) -> Vec<PathBuf> {
        if self.selection.is_empty() {
            self.current()
                .map(|e| vec![self.dir.join(&e.file_name)])
                .unwrap_or_default()
        } else {
            self.selection.iter().map(|n| self.dir.join(n)).collect()
        }
    }

    /// `p`: properties of the selection, or of the cursor item.
    pub fn open_props(&mut self) {
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        let props = Props::new(&targets, self.home.as_deref(), &self.tz);
        self.modal = Some(Modal::Props(Box::new(props)));
    }

    /// Asks for a fresh git status for the current folder.
    fn refresh_git(&mut self) {
        self.git_watch.request(&self.dir);
        self.git_asked = Instant::now();
    }

    pub fn open_help(&mut self) {
        self.modal = Some(Modal::Help(HelpState {
            cursor: 0,
            filter: TextInput::default(),
            typing: false,
        }));
    }

    /// `f`: find files under the current folder.
    pub fn open_picker(&mut self) {
        let picker = FilePicker::open(self.dir.clone(), self.show_hidden);
        self.modal = Some(Modal::Picker(Box::new(picker)));
        self.focus = Focus::Files;
    }

    /// `F`: search the contents of files under the current folder.
    pub fn open_search(&mut self) {
        let search = ContentSearch::open(self.dir.clone(), self.show_hidden);
        self.modal = Some(Modal::Search(Box::new(search)));
        self.focus = Focus::Files;
    }

    /// `Shift-Enter` in a picker or search: open the highlighted file in
    /// Helix, at the matched line for a content search.
    pub fn picker_edit(&mut self) {
        let (path, line) = match &self.modal {
            Some(Modal::Picker(p)) => (p.selected_path(), None),
            Some(Modal::Search(s)) => match s.selected() {
                Some((path, line)) => (Some(path), Some(line)),
                None => (None, None),
            },
            _ => return,
        };
        let Some(path) = path else { return };
        if let Err(what) = text_or_not(&path) {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            return self.error(format!("Shift-Enter edits text files; {name} is {what}"));
        }
        self.modal = None;
        self.edit_files(vec![path], line);
    }

    /// `Enter` in a picker or search: go to the file's folder with the
    /// cursor on it.
    pub fn picker_jump(&mut self) {
        let path = match &self.modal {
            Some(Modal::Picker(p)) => p.selected_path(),
            Some(Modal::Search(s)) => s.selected().map(|(path, _)| path),
            _ => return,
        };
        let Some(path) = path else {
            return;
        };
        self.modal = None;
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        if name.to_string_lossy().starts_with('.') {
            self.show_hidden = true;
        }
        self.go(parent.to_path_buf(), Some(name.to_os_string()));
    }

    pub fn start_filter(&mut self) {
        self.filtering = true;
    }

    /// Keeps the filter and goes back to normal keys.
    pub fn keep_filter(&mut self) {
        self.filtering = false;
    }

    pub fn cancel_filter(&mut self) {
        self.filtering = false;
        self.filter.clear();
        self.refilter_here();
    }

    /// Re-applies hidden files and the filter, keeping the cursor on the
    /// same item when it's still shown.
    pub fn refilter_here(&mut self) {
        let anchor = self.visible.get(self.cursor).copied().unwrap_or(0);
        let name = self.current().map(|e| e.file_name.clone());
        self.refilter(name.as_deref(), anchor);
    }

    /// Ask before trashing (or with `delete`, removing) the targets.
    pub fn start_remove(&mut self, delete: bool) {
        let paths = self.targets();
        if !paths.is_empty() {
            self.modal = Some(Modal::Confirm { delete, paths });
        }
    }

    /// `y` in the confirm dialog.
    pub fn confirm_remove(&mut self) {
        let Some(Modal::Confirm { delete, paths }) = self.modal.take() else {
            return;
        };
        let n = paths.len();
        let result = if delete {
            paths
                .iter()
                .enumerate()
                .try_for_each(|(i, p)| self.vfs.delete(p).map_err(|e| (i, e)))
        } else {
            self.vfs.trash(&paths).map_err(|e| (0, e))
        };
        match (result, delete) {
            (Ok(()), false) => self.info(format!("{n} moved to trash, restorable")),
            (Ok(()), true) => self.info(format!("{n} deleted for good")),
            (Err((_, e)), false) => self.error(format!("can't move to trash: {e}")),
            (Err((i, e)), true) => self.error(format!("deleted {i} of {n}, then: {e}")),
        }
        self.selection.clear();
        self.reload(None);
    }

    /// A plain rename of the cursor item, or a bulk rename of the selection.
    pub fn start_rename(&mut self) {
        let names: Vec<OsString> = if self.selection.is_empty() {
            self.current()
                .map(|e| e.file_name.clone())
                .into_iter()
                .collect()
        } else {
            self.selection.clone()
        };
        let olds: Option<Vec<String>> = names.into_iter().map(|n| n.into_string().ok()).collect();
        let Some(olds) = olds else {
            return self.error("can't rename names that aren't valid UTF-8".into());
        };
        let input = match olds.as_slice() {
            [] => return,
            [one] => TextInput::new(one),
            _ => TextInput::default(),
        };
        self.modal = Some(Modal::Rename { olds, input });
    }

    /// `n` / `N`: ask for a name for a new file or folder here.
    pub fn start_create(&mut self, folder: bool) {
        self.modal = Some(Modal::Create {
            folder,
            input: TextInput::default(),
        });
    }

    /// `Enter` in the create dialog. Never overwrites anything.
    pub fn apply_create(&mut self) {
        let Some(Modal::Create { folder, input }) = &self.modal else {
            return;
        };
        let (folder, name) = (*folder, input.text.clone());
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return self.error("a name can't be empty, . or .., or contain /".into());
        }
        let path = self.dir.join(&name);
        if self.vfs.exists(&path) {
            return self.error(format!("“{name}” already exists here"));
        }
        let made = if folder {
            self.vfs.create_dir(&path)
        } else {
            self.vfs.create_file(&path)
        };
        self.modal = None;
        match made {
            Ok(()) => {
                if name.starts_with('.') {
                    self.show_hidden = true;
                }
                self.info(format!("created {name}"));
                self.selection.clear();
                self.filter.clear();
                self.reload(Some(OsString::from(&name)));
            }
            Err(e) => self.error(format!("can't create {name}: {e}")),
        }
    }

    /// Every name in the folder, hidden ones included.
    pub fn existing_names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }

    /// The preview rows of the open bulk rename.
    pub fn bulk_rows(&self) -> Vec<rename::Row> {
        match &self.modal {
            Some(Modal::Rename { olds, input }) if olds.len() > 1 => {
                rename::plan_bulk(&input.text, olds, &self.existing_names())
            }
            _ => Vec::new(),
        }
    }

    /// `Enter` in the rename dialog.
    pub fn apply_rename(&mut self) {
        let Some(Modal::Rename { olds, input }) = &self.modal else {
            return;
        };
        let existing = self.existing_names();
        let rows = if olds.len() == 1 {
            vec![rename::plan_single(&olds[0], &input.text, &existing)]
        } else {
            rename::plan_bulk(&input.text, olds, &existing)
        };
        if let [row] = rows.as_slice() {
            match row.problem {
                Some(rename::Problem::Clash) => {
                    return self.error(format!("“{}” already exists here", row.new));
                }
                Some(_) => {
                    return self.error("a name can't be empty, . or .., or contain /".into());
                }
                None => {}
            }
        } else if rows.iter().any(|r| r.problem.is_some()) {
            return self
                .error("fix the names marked ✗ first: empty, clashing or containing /".into());
        }
        self.modal = None;
        let first = rows.first().map(|r| OsString::from(&r.new));
        match rename::apply(&*self.vfs, &self.dir, &rows) {
            Ok(0) => {}
            Ok(1) => self.info(format!("renamed {} → {}", rows[0].old, rows[0].new)),
            Ok(n) => self.info(format!("renamed {n} files")),
            Err(e) => self.error(e),
        }
        self.selection.clear();
        self.reload(first);
    }

    /// Re-reads the current folder after a file operation, keeping the
    /// cursor on `focus`, on the same item, or else nearby.
    fn reload(&mut self, focus: Option<OsString>) {
        let anchor = self.visible.get(self.cursor).copied().unwrap_or(0);
        let focus = focus.or_else(|| self.current().map(|e| e.file_name.clone()));
        match self.vfs.list(&self.dir) {
            Ok(mut entries) => {
                sort_entries(&mut entries, self.sort);
                self.entries = entries;
                self.count_folders();
                self.refilter(focus.as_deref(), anchor);
                self.refresh_git();
            }
            Err(e) => {
                let shown = self.display_path(&self.dir);
                self.error(format!("can't reread {shown}: {e}"));
            }
        }
    }

    /// `e`: text files open in the running Helix (or a new one in a kitty
    /// tab); a folder gets `hx .` in a new tab.
    pub fn open_editor(&mut self) {
        let targets = self.targets();
        let Some(first) = targets.first() else { return };
        if targets.len() == 1 && first.is_dir() {
            let dir = first.clone();
            let shown = self.display_path(&dir);
            return self.job(move |k| {
                k.edit_folder(&dir)
                    .map(|()| format!("started Helix in {shown}, in a new tab"))
            });
        }
        for path in &targets {
            if let Err(what) = text_or_not(path) {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                return self.error(format!("e edits text files; {name} is {what}"));
            }
        }
        self.edit_files(targets, None);
    }

    /// Opens text files in Helix, the first one at `line` if given.
    fn edit_files(&mut self, targets: Vec<PathBuf>, line: Option<u64>) {
        let mut what = match targets.as_slice() {
            [one] => one
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            many => format!("{} files", many.len()),
        };
        if let Some(n) = line {
            what.push_str(&format!(" at line {n}"));
        }
        let cwd = self.dir.clone();
        self.job(move |k| {
            k.open_files(&targets, line, &cwd)
                .map(|opened| match opened {
                    Opened::Existing => format!("opened {what} in Helix"),
                    Opened::NewTab => format!("started Helix on {what}, in a new tab"),
                })
        });
    }

    /// Runs a kitty call on a thread; its outcome arrives as a message.
    fn job(&mut self, f: impl FnOnce(&Kitty) -> Result<String, String> + Send + 'static) {
        let kitty = match Kitty::from_env("helix") {
            Ok(k) => k,
            Err(e) => return self.error(e),
        };
        let tx = self.jobs.0.clone();
        thread::spawn(move || {
            let message = match f(&kitty) {
                Ok(text) => Message { text, error: false },
                Err(text) => Message { text, error: true },
            };
            let _ = tx.send(message);
        });
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
            Focus::Files | Focus::Terminal => match self.current() {
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

    /// `s`: name → type → size → modified, each in its natural direction.
    pub fn cycle_sort(&mut self) {
        let by = self.sort.by.next();
        self.set_sort(Sort { by, reverse: false });
    }

    /// `S`: flip the current order.
    pub fn reverse_sort(&mut self) {
        let sort = Sort {
            reverse: !self.sort.reverse,
            ..self.sort
        };
        self.set_sort(sort);
    }

    /// Re-sorts, keeping the cursor on its item.
    fn set_sort(&mut self, sort: Sort) {
        // Before sorting: `visible` still indexes the old order.
        let name = self.current().map(|e| e.file_name.clone());
        self.sort = sort;
        sort_entries(&mut self.entries, sort);
        self.refilter(name.as_deref(), 0);
        self.info(format!("sorted by {}", sort.label()));
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.refilter_here();
    }

    pub fn toggle_preview(&mut self) {
        self.preview = !self.preview;
    }

    /// Point the preview at the item under the cursor, or at the picker's
    /// highlighted file while a picker is open, and ask the worker for it.
    /// The sizes are the preview bodies in cells, `None` while hidden.
    pub fn sync_preview(&mut self, pane: Option<(u16, u16)>, picker: Option<(u16, u16)>) {
        self.preview_key = match &self.modal {
            Some(Modal::Picker(p)) => picker
                .zip(p.selected_path())
                .map(|(size, path)| file_key(path, size, None)),
            Some(Modal::Search(s)) => picker
                .zip(s.selected())
                .map(|(size, (path, line))| file_key(path, size, Some(line))),
            _ => pane.zip(self.current()).map(|(size, e)| preview::Key {
                path: self.dir.join(&e.file_name),
                mtime: e.mtime,
                size,
                line: None,
            }),
        };
        if let Some(key) = &self.preview_key {
            self.previewer.want(key);
        }
    }

    /// Takes what the workers have finished; returns whether anything changed.
    pub fn drain_workers(&mut self) -> bool {
        let mut changed = self.previewer.drain();
        while let Ok(m) = self.jobs.1.try_recv() {
            self.push_message(m.text, m.error);
            changed = true;
        }
        if let Some(t) = &mut self.terminal {
            changed |= t.drain();
            if t.exited() {
                self.terminal = None;
                self.terminal_open = false;
                if self.focus == Focus::Terminal {
                    self.focus = self.focus_before_terminal;
                }
                let key = self
                    .keymap
                    .first_key(Command::TermToggle)
                    .map(|k| k.pretty())
                    .unwrap_or_else(|| "term.toggle".into());
                self.info(format!("the shell exited; {key} starts a new one"));
                changed = true;
            }
        }
        match &mut self.modal {
            Some(Modal::Picker(p)) => changed |= p.tick(),
            Some(Modal::Search(s)) => changed |= s.tick(),
            Some(Modal::Props(p)) => changed |= p.drain(),
            _ => {}
        }
        while let Ok((dir, status)) = self.git_watch.results.try_recv() {
            if dir == self.dir {
                changed |= self.git != status;
                self.git = status;
            }
        }
        // Commits and edits made elsewhere show up within a few seconds.
        if self.git_asked.elapsed() >= GIT_REFRESH {
            self.refresh_git();
        }
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
        sort_entries(&mut entries, self.sort);
        if let Some(e) = self.current() {
            self.remembered
                .insert(self.dir.clone(), e.file_name.clone());
        }
        let focus = focus.or_else(|| self.remembered.get(&dir).cloned());
        // Selection and filter belong to the folder they were made in.
        self.selection.clear();
        self.filter.clear();
        self.filtering = false;
        // Within the same work tree the old status stays up while the
        // new one comes, so the segment and marks don't blink.
        if self.git.as_ref().is_some_and(|g| !dir.starts_with(&g.root)) {
            self.git = None;
        }
        self.dir = dir;
        self.entries = entries;
        self.count_folders();
        self.refresh_git();
        self.refilter(focus.as_deref(), 0);
        Ok(())
    }

    fn count_folders(&self) {
        let folders = self
            .entries
            .iter()
            .filter(|e| e.is_dir())
            .map(|e| e.file_name.clone())
            .collect();
        self.counter.request(&self.dir, folders);
    }

    /// Recompute `visible` from the hidden setting and the filter, drop
    /// selected items that are no longer shown, and put the cursor on
    /// `focus`, or else on the first shown entry at or after `anchor`.
    fn refilter(&mut self, focus: Option<&OsStr>, anchor: usize) {
        let show_hidden = self.show_hidden;
        let query = self.filter.text.to_lowercase();
        self.visible = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                (show_hidden || !e.is_hidden()) && e.name.to_lowercase().contains(&query)
            })
            .collect();
        let entries = &self.entries;
        let visible = &self.visible;
        self.selection
            .retain(|n| visible.iter().any(|&i| entries[i].file_name == *n));
        self.cursor = focus
            .and_then(|f| visible.iter().position(|&i| entries[i].file_name == f))
            .or_else(|| visible.iter().position(|&i| i >= anchor))
            .unwrap_or(visible.len().saturating_sub(1));
    }
}

/// Ok for a text file; otherwise what it is, for the message.
fn text_or_not(path: &Path) -> Result<(), &'static str> {
    if path.is_dir() {
        return Err("a folder");
    }
    let mut head = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(8 * 1024).read_to_end(&mut head));
    if read.is_err() {
        return Err("unreadable");
    }
    match classify(path, &head) {
        FileKind::Text => Ok(()),
        FileKind::Image => Err("an image"),
        FileKind::Pdf => Err("a PDF"),
        FileKind::Video => Err("a video"),
        FileKind::Audio => Err("an audio file"),
        FileKind::Archive => Err("an archive"),
        FileKind::Binary => Err("a binary file"),
    }
}

/// A preview key for a file that isn't in the listing.
fn file_key(path: PathBuf, size: (u16, u16), line: Option<u64>) -> preview::Key {
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    preview::Key {
        path,
        mtime,
        size,
        line,
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
        app.selection
            .iter()
            .map(|n| n.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn selection_keeps_order_and_stays_in_the_folder() {
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
        select(&mut app, "link-y");
        app.toggle_select();
        assert_eq!(names(&app), ["file.txt", "a", "link-y"]);
        assert_eq!(
            app.targets(),
            [r.join("file.txt"), r.join("a"), r.join("link-y")]
        );

        // Unselecting keeps the others in order.
        select(&mut app, "a");
        app.toggle_select();
        assert_eq!(names(&app), ["file.txt", "link-y"]);

        // Leaving the folder drops the selection, and coming back doesn't
        // bring it back.
        select(&mut app, "b");
        app.nav_enter();
        assert!(app.selection.is_empty());
        app.nav_parent();
        assert!(app.selection.is_empty());
        assert_eq!(app.targets(), [r.join("b")]);
    }

    #[test]
    fn hiding_dotfiles_unselects_them() {
        let tmp = tree();
        let mut app = App::new(root(&tmp), None, None).unwrap();
        app.toggle_hidden();
        select(&mut app, ".dot");
        app.toggle_select();
        select(&mut app, "a");
        app.toggle_select();
        app.toggle_hidden();
        assert_eq!(names(&app), ["a"]);
    }

    #[test]
    fn summary_uses_counts_from_the_worker() {
        let tmp = tree();
        let mut app = App::new(root(&tmp), None, None).unwrap();
        select(&mut app, "b");
        app.toggle_select();
        for _ in 0..500 {
            app.drain_workers();
            if app
                .entries
                .iter()
                .find(|e| e.name == "b")
                .unwrap()
                .children
                .is_some()
            {
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

    /// Local, except trash moves into a folder of the test's own.
    struct TestTrash(PathBuf);

    impl Vfs for TestTrash {
        fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
            Local.list(dir)
        }
        fn count_children(&self, dir: &Path) -> io::Result<usize> {
            Local.count_children(dir)
        }
        fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
            Local.resolve(path)
        }
        fn exists(&self, path: &Path) -> bool {
            Local.exists(path)
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            Local.rename(from, to)
        }
        fn trash(&self, paths: &[PathBuf]) -> io::Result<()> {
            for p in paths {
                fs::rename(p, self.0.join(p.file_name().unwrap()))?;
            }
            Ok(())
        }
        fn create_file(&self, path: &Path) -> io::Result<()> {
            Local.create_file(path)
        }
        fn create_dir(&self, path: &Path) -> io::Result<()> {
            Local.create_dir(path)
        }
        fn delete(&self, path: &Path) -> io::Result<()> {
            Local.delete(path)
        }
    }

    #[test]
    fn trash_moves_targets_and_keeps_the_cursor_close() {
        let tmp = tree();
        let r = root(&tmp);
        let bin = TempDir::new().unwrap();
        let mut app = App::new(r.clone(), None, None).unwrap();
        app.vfs = Arc::new(TestTrash(bin.path().to_path_buf()));

        select(&mut app, "file.txt");
        app.start_remove(false);
        app.confirm_remove();
        assert_eq!(app.messages[0].text, "1 moved to trash, restorable");
        assert!(bin.path().join("file.txt").exists());
        // The cursor moves to the next item.
        assert_eq!(name(&app), "link-y");

        // Selection wins over the cursor.
        select(&mut app, "a");
        app.toggle_select();
        select(&mut app, "link-z");
        app.toggle_select();
        app.start_remove(false);
        app.confirm_remove();
        assert!(bin.path().join("a").exists());
        // A dangling link now, since its target is relative; it's still there.
        assert!(fs::symlink_metadata(bin.path().join("link-z")).is_ok());
        assert!(r.join("b/.z").exists(), "trashing a link keeps its target");
        assert!(app.selection.is_empty());
    }
}
