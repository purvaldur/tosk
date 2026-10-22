//! The `f`/`F` file pickers: a gitignore-aware walk streamed into nucleo.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use ignore::{WalkBuilder, WalkState};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Matcher, Nucleo};

use crate::text_input::TextInput;

pub struct FilePicker {
    pub root: PathBuf,
    pub input: TextInput,
    /// Index into the matches.
    pub cursor: usize,
    nucleo: Nucleo<String>,
    /// For the highlighted characters, which nucleo's snapshot doesn't keep.
    matcher: RefCell<Matcher>,
    /// What the pattern was last parsed from.
    query: String,
    /// Stops the walk when the picker closes.
    cancel: Arc<AtomicBool>,
}

/// A visible result: its path relative to the root and the char indices
/// that matched.
pub struct Hit {
    pub path: String,
    pub matched: Vec<u32>,
}

impl FilePicker {
    /// Starts walking `root` on worker threads; results stream in.
    pub fn open(root: PathBuf, show_hidden: bool) -> FilePicker {
        let nucleo = Nucleo::new(Config::DEFAULT.match_paths(), Arc::new(|| {}), None, 1);
        let cancel = Arc::new(AtomicBool::new(false));
        let injector = nucleo.injector();
        let (walk_root, stop) = (root.clone(), cancel.clone());
        thread::spawn(move || {
            WalkBuilder::new(&walk_root)
                .hidden(!show_hidden)
                .filter_entry(|e| e.file_name() != ".git")
                .build_parallel()
                .run(|| {
                    let (injector, stop, root) =
                        (injector.clone(), stop.clone(), walk_root.clone());
                    Box::new(move |entry| {
                        if stop.load(Ordering::Relaxed) {
                            return WalkState::Quit;
                        }
                        let Ok(entry) = entry else {
                            return WalkState::Continue;
                        };
                        let is_dir = entry.file_type().is_none_or(|t| t.is_dir());
                        if !is_dir && let Ok(rel) = entry.path().strip_prefix(&root) {
                            let path = rel.to_string_lossy().into_owned();
                            injector.push(path, |p, cols| cols[0] = p.as_str().into());
                        }
                        WalkState::Continue
                    })
                });
        });
        FilePicker {
            root,
            input: TextInput::default(),
            cursor: 0,
            nucleo,
            matcher: RefCell::new(Matcher::new(Config::DEFAULT.match_paths())),
            query: String::new(),
            cancel,
        }
    }

    /// Lets nucleo take in new files and the current query. Returns whether
    /// the results changed.
    pub fn tick(&mut self) -> bool {
        if self.input.text != self.query {
            // Typing more only narrows, which nucleo can do incrementally.
            let append = self.input.text.starts_with(&self.query);
            self.nucleo.pattern.reparse(
                0,
                &self.input.text,
                CaseMatching::Smart,
                Normalization::Smart,
                append,
            );
            self.query = self.input.text.clone();
            self.cursor = 0;
        }
        let changed = self.nucleo.tick(10).changed;
        let matched = self.counts().0;
        self.cursor = self.cursor.min(matched.saturating_sub(1));
        changed
    }

    /// (matches, files found so far)
    pub fn counts(&self) -> (usize, usize) {
        let s = self.nucleo.snapshot();
        (s.matched_item_count() as usize, s.item_count() as usize)
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.counts().0.saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    /// The highlighted result, relative to the root.
    pub fn selected(&self) -> Option<&str> {
        let item = self
            .nucleo
            .snapshot()
            .get_matched_item(self.cursor as u32)?;
        Some(item.data.as_str())
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        self.selected().map(|rel| self.root.join(rel))
    }

    /// `n` results starting at `start`, best first.
    pub fn hits(&self, start: usize, n: usize) -> Vec<Hit> {
        let mut matcher = self.matcher.borrow_mut();
        let snapshot = self.nucleo.snapshot();
        let end = (start + n).min(snapshot.matched_item_count() as usize);
        if start >= end {
            return Vec::new();
        }
        let pattern = snapshot.pattern().column_pattern(0);
        snapshot
            .matched_items(start as u32..end as u32)
            .map(|item| {
                let mut matched = Vec::new();
                pattern.indices(
                    item.matcher_columns[0].slice(..),
                    &mut matcher,
                    &mut matched,
                );
                matched.sort_unstable();
                matched.dedup();
                Hit {
                    path: item.data.clone(),
                    matched,
                }
            })
            .collect()
    }
}

/// What the file picker and the content search share: a query and a
/// result cursor.
pub trait Pick {
    fn input(&mut self) -> &mut TextInput;
    fn move_cursor(&mut self, delta: isize);
}

impl Pick for FilePicker {
    fn input(&mut self) -> &mut TextInput {
        &mut self.input
    }
    fn move_cursor(&mut self, delta: isize) {
        FilePicker::move_cursor(self, delta);
    }
}

impl Pick for crate::grep::ContentSearch {
    fn input(&mut self) -> &mut TextInput {
        &mut self.input
    }
    fn move_cursor(&mut self, delta: isize) {
        crate::grep::ContentSearch::move_cursor(self, delta);
    }
}

impl Drop for FilePicker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// Ticks until `done` holds or five seconds pass.
    fn settle(p: &mut FilePicker, done: impl Fn(&FilePicker) -> bool) {
        let start = Instant::now();
        while !done(p) && start.elapsed() < Duration::from_secs(5) {
            p.tick();
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn tree() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("src/ui")).unwrap();
        fs::create_dir_all(r.join("target/debug")).unwrap();
        fs::create_dir_all(r.join(".git")).unwrap();
        for f in [
            "src/main.rs",
            "src/ui/list.rs",
            "Cargo.toml",
            ".hidden.txt",
            "target/debug/junk",
            ".git/HEAD",
        ] {
            fs::write(r.join(f), "").unwrap();
        }
        fs::write(r.join(".gitignore"), "/target\n").unwrap();
        // .gitignore only counts inside a repository, as in git itself.
        fs::write(r.join(".git/config"), "").unwrap();
        tmp
    }

    fn all(p: &FilePicker) -> Vec<String> {
        let mut v: Vec<String> = p.hits(0, 100).into_iter().map(|h| h.path).collect();
        v.sort();
        v
    }

    #[test]
    fn walks_files_respecting_gitignore_and_hidden() {
        let tmp = tree();
        let mut p = FilePicker::open(tmp.path().to_path_buf(), false);
        settle(&mut p, |p| p.counts().1 >= 3);
        thread::sleep(Duration::from_millis(50));
        p.tick();
        assert_eq!(all(&p), ["Cargo.toml", "src/main.rs", "src/ui/list.rs"]);

        let mut p = FilePicker::open(tmp.path().to_path_buf(), true);
        settle(&mut p, |p| p.counts().1 >= 5);
        thread::sleep(Duration::from_millis(50));
        p.tick();
        assert_eq!(
            all(&p),
            [
                ".gitignore",
                ".hidden.txt",
                "Cargo.toml",
                "src/main.rs",
                "src/ui/list.rs"
            ]
        );
    }

    #[test]
    fn fuzzy_matches_with_highlights() {
        let tmp = tree();
        let mut p = FilePicker::open(tmp.path().to_path_buf(), false);
        settle(&mut p, |p| p.counts().1 >= 3);
        p.input = TextInput::new("uilst");
        settle(&mut p, |p| p.counts().0 == 1);
        assert_eq!(p.selected(), Some("src/ui/list.rs"));
        let hit = &p.hits(0, 10)[0];
        let chars: Vec<char> = hit.path.chars().collect();
        let picked: String = hit.matched.iter().map(|&i| chars[i as usize]).collect();
        assert_eq!(picked, "uilst");
        assert_eq!(p.selected_path(), Some(tmp.path().join("src/ui/list.rs")));

        p.input = TextInput::new("nothing-like-this");
        settle(&mut p, |p| p.counts().0 == 0);
        assert_eq!(p.selected(), None);
        p.move_cursor(3);
        assert_eq!(p.cursor, 0);
    }
}
