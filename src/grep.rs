//! `F`: search file contents like Helix's global search. The text is a
//! smart-case regex run over every file (gitignore-aware); `%p` (or `%path`)
//! starts a fuzzy filter on the file's path: `name: foo %p stadeployaml`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::{WalkBuilder, WalkState};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Injector, Matcher, Nucleo};

use crate::text_input::TextInput;

/// Wait this long after the last keystroke before starting a new search.
const DEBOUNCE: Duration = Duration::from_millis(120);
/// Stop collecting after this many matching lines.
const MAX_HITS: usize = 100_000;

/// One matching line. Its text isn't kept: the preview shows it in context.
pub struct Line {
    /// Relative to the search root.
    pub path: String,
    /// 1-based.
    pub line: u64,
}

/// A visible result with the path characters the fuzzy filter matched.
pub struct Hit<'a> {
    pub line: &'a Line,
    pub path_matched: Vec<u32>,
}

/// Splits a query into the content regex and the `%p` path filter.
pub fn split_query(query: &str) -> (String, String) {
    let (mut content, mut path) = (Vec::new(), Vec::new());
    let mut in_path = false;
    // Splitting on single spaces keeps runs of spaces in the regex intact.
    for word in query.split(' ') {
        match word.strip_prefix('%') {
            Some(col) if !col.is_empty() && "path".starts_with(col) => in_path = true,
            _ if in_path => path.push(word),
            _ => content.push(word),
        }
    }
    (content.join(" "), path.join(" "))
}

/// A running search: cancelled by dropping, counted for the title.
struct Search {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Drop for Search {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub struct ContentSearch {
    pub root: PathBuf,
    pub input: TextInput,
    /// Index into the matches.
    pub cursor: usize,
    show_hidden: bool,
    nucleo: Nucleo<Line>,
    matcher: RefCell<Matcher>,
    /// What the running search and the path filter were made from.
    searched: String,
    /// The content part as last seen, to tell when typing pauses.
    typed: String,
    path_query: String,
    /// When the content part last changed, for the debounce.
    changed_at: Instant,
    search: Option<Search>,
}

impl ContentSearch {
    pub fn open(root: PathBuf, show_hidden: bool) -> ContentSearch {
        ContentSearch {
            root,
            input: TextInput::default(),
            cursor: 0,
            show_hidden,
            nucleo: Nucleo::new(Config::DEFAULT.match_paths(), Arc::new(|| {}), None, 1),
            matcher: RefCell::new(Matcher::new(Config::DEFAULT.match_paths())),
            searched: String::new(),
            typed: String::new(),
            path_query: String::new(),
            changed_at: Instant::now(),
            search: None,
        }
    }

    /// Restarts the search when the content part settles, refilters paths,
    /// and takes in new results. Returns whether anything visible changed.
    pub fn tick(&mut self) -> bool {
        let (content, path) = split_query(&self.input.text);
        let mut changed = false;
        if path != self.path_query {
            let append = path.starts_with(&self.path_query);
            self.nucleo.pattern.reparse(
                0,
                &path,
                CaseMatching::Smart,
                Normalization::Smart,
                append,
            );
            self.path_query = path;
            self.cursor = 0;
            changed = true;
        }
        if content != self.typed {
            self.typed = content.clone();
            self.changed_at = Instant::now();
        }
        // Search again once typing pauses; clearing the query is instant.
        let settled = self.changed_at.elapsed() >= DEBOUNCE || content.is_empty();
        if content != self.searched && settled {
            self.restart(&content);
            self.searched = content;
            changed = true;
        }
        changed |= self.nucleo.tick(10).changed;
        self.cursor = self.cursor.min(self.counts().0.saturating_sub(1));
        changed
    }

    /// Whether a search is still walking files.
    pub fn searching(&self) -> bool {
        self.search
            .as_ref()
            .is_some_and(|s| !s.done.load(Ordering::Relaxed))
    }

    fn restart(&mut self, content: &str) {
        self.search = None;
        self.nucleo.restart(true);
        self.cursor = 0;
        if content.is_empty() {
            return;
        }
        let matcher = match RegexMatcherBuilder::new().case_smart(true).build(content) {
            Ok(m) => m,
            // Not a valid regex (a lone `(`, say): search for the text as is.
            Err(_) => match RegexMatcherBuilder::new()
                .case_smart(true)
                .fixed_strings(true)
                .build(content)
            {
                Ok(m) => m,
                Err(_) => return,
            },
        };
        let search = Search {
            stop: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicBool::new(false)),
        };
        let (stop, done) = (search.stop.clone(), search.done.clone());
        let (root, hidden, injector) =
            (self.root.clone(), self.show_hidden, self.nucleo.injector());
        thread::spawn(move || {
            walk(&root, hidden, &matcher, &injector, &stop);
            done.store(true, Ordering::Relaxed);
        });
        self.search = Some(search);
    }

    /// (matching lines shown, matching lines found)
    pub fn counts(&self) -> (usize, usize) {
        let s = self.nucleo.snapshot();
        (s.matched_item_count() as usize, s.item_count() as usize)
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.counts().0.saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    /// The highlighted line's file and line number.
    pub fn selected(&self) -> Option<(PathBuf, u64)> {
        let item = self
            .nucleo
            .snapshot()
            .get_matched_item(self.cursor as u32)?;
        Some((self.root.join(&item.data.path), item.data.line))
    }

    /// Calls `f` with `n` results starting at `start`.
    pub fn with_hits<R>(&self, start: usize, n: usize, f: impl FnOnce(Vec<Hit>) -> R) -> R {
        let snapshot = self.nucleo.snapshot();
        let end = (start + n).min(snapshot.matched_item_count() as usize);
        let mut matcher = self.matcher.borrow_mut();
        let pattern = snapshot.pattern().column_pattern(0);
        let hits = if start < end {
            snapshot
                .matched_items(start as u32..end as u32)
                .map(|item| {
                    let mut path_matched = Vec::new();
                    pattern.indices(
                        item.matcher_columns[0].slice(..),
                        &mut matcher,
                        &mut path_matched,
                    );
                    path_matched.sort_unstable();
                    path_matched.dedup();
                    Hit {
                        line: item.data,
                        path_matched,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        f(hits)
    }
}

/// Searches every file under `root` on worker threads, pushing matching
/// lines into `injector` until done, stopped, or `MAX_HITS` is reached.
fn walk(
    root: &std::path::Path,
    show_hidden: bool,
    matcher: &RegexMatcher,
    injector: &Injector<Line>,
    stop: &Arc<AtomicBool>,
) {
    let found = Arc::new(AtomicUsize::new(0));
    WalkBuilder::new(root)
        .hidden(!show_hidden)
        .filter_entry(|e| e.file_name() != ".git")
        .build_parallel()
        .run(|| {
            let (matcher, injector, stop, found) = (
                matcher.clone(),
                injector.clone(),
                stop.clone(),
                found.clone(),
            );
            let mut searcher = SearcherBuilder::new()
                .binary_detection(BinaryDetection::quit(0))
                .line_number(true)
                .build();
            Box::new(move |entry| {
                if stop.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                let Ok(rel) = entry.path().strip_prefix(root) else {
                    return WalkState::Continue;
                };
                let path = rel.to_string_lossy().into_owned();
                let _ = searcher.search_path(
                    &matcher,
                    entry.path(),
                    Lossy(|line, _| {
                        if stop.load(Ordering::Relaxed) {
                            return Ok(false);
                        }
                        let hit = Line {
                            path: path.clone(),
                            line,
                        };
                        injector.push(hit, |l, cols| cols[0] = l.path.as_str().into());
                        if found.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_HITS {
                            stop.store(true, Ordering::Relaxed);
                        }
                        Ok(true)
                    }),
                );
                WalkState::Continue
            })
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn query_splits_on_path_markers() {
        let s = |q: &str| split_query(q);
        assert_eq!(
            s("name: foo %p stadeployaml"),
            ("name: foo".into(), "stadeployaml".into())
        );
        assert_eq!(s("a  b"), ("a  b".into(), String::new()));
        assert_eq!(s("x %path src ui"), ("x".into(), "src ui".into()));
        assert_eq!(s("%pa src"), (String::new(), "src".into()));
        // Not a column: kept in the regex.
        assert_eq!(s("100% done %q"), ("100% done %q".into(), String::new()));
    }

    fn tree() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("stage")).unwrap();
        fs::create_dir_all(r.join("prod")).unwrap();
        fs::write(
            r.join("stage/deployment.yaml"),
            "kind: x\nname: foo\n\tname: foo-bar\n",
        )
        .unwrap();
        fs::write(r.join("prod/deployment.yaml"), "name: foo\n").unwrap();
        fs::write(r.join("notes.md"), "Name: Foo is capitalised\n").unwrap();
        fs::write(r.join("blob.bin"), b"name: foo\0\0").unwrap();
        tmp
    }

    fn settle(s: &mut ContentSearch) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            s.tick();
            if !s.searching() && s.changed_at.elapsed() > DEBOUNCE * 2 {
                s.tick();
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("search didn't finish");
    }

    fn results(s: &ContentSearch) -> Vec<String> {
        let mut v = s.with_hits(0, 100, |hits| {
            hits.iter()
                .map(|h| format!("{}:{}", h.line.path, h.line.line))
                .collect::<Vec<_>>()
        });
        v.sort();
        v
    }

    #[test]
    fn searches_contents_and_filters_paths() {
        let tmp = tree();
        let mut s = ContentSearch::open(tmp.path().to_path_buf(), false);
        s.input = TextInput::new("name: foo");
        settle(&mut s);
        // Smart case: lowercase matches any case. Binary files are skipped.
        assert_eq!(
            results(&s),
            [
                "notes.md:1",
                "prod/deployment.yaml:1",
                "stage/deployment.yaml:2",
                "stage/deployment.yaml:3",
            ]
        );

        s.input = TextInput::new("name: foo %p stadeployaml");
        settle(&mut s);
        assert_eq!(
            results(&s),
            ["stage/deployment.yaml:2", "stage/deployment.yaml:3"]
        );
        let (path, line) = s.selected().unwrap();
        assert!(path.ends_with("stage/deployment.yaml") && (line == 2 || line == 3));

        // Uppercase makes it case-sensitive.
        s.input = TextInput::new("Name: Foo");
        settle(&mut s);
        assert_eq!(results(&s), ["notes.md:1"]);

        // An invalid regex is searched as plain text.
        fs::write(tmp.path().join("code.rs"), "call(x\n").unwrap();
        s.input = TextInput::new("call(");
        settle(&mut s);
        assert_eq!(results(&s), ["code.rs:1"]);

        s.input = TextInput::default();
        settle(&mut s);
        assert!(results(&s).is_empty());
    }
}
