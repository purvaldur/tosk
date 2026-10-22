//! The git segment and file marks, from `git status` on a worker, and
//! the repository lookup for properties.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::tool;

/// Requests closer together than this are answered once.
const DEBOUNCE: Duration = Duration::from_millis(150);
const TIMEOUT: Duration = Duration::from_secs(10);

/// Keeps the user's git config out of the tests.
#[cfg(test)]
const ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
];
#[cfg(not(test))]
const ENV: &[(&str, &str)] = &[];

/// What git says about a file, weakest first: a folder takes the
/// strongest mark inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mark {
    Ignored,
    Untracked,
    Added,
    Deleted,
    Modified,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitStatus {
    /// The work tree's top folder, spelled the way tosk reached it (through
    /// symlinks, if that's how).
    pub root: PathBuf,
    pub branch: String,
    /// Commits ahead of and behind the upstream, if there is one.
    pub ahead_behind: Option<(u32, u32)>,
    /// Changed, staged, conflicted and untracked paths.
    pub changed: usize,
    /// Paths relative to `root`. An untracked or ignored folder is one entry.
    marks: HashMap<PathBuf, Mark>,
    /// Folders with marked paths inside, and the strongest mark.
    inside: HashMap<PathBuf, Mark>,
}

impl GitStatus {
    /// The mark for `path`, and whether it's a folder's summary of what's
    /// inside rather than its own.
    pub fn mark(&self, path: &Path) -> Option<(Mark, bool)> {
        let rel = path.strip_prefix(&self.root).ok()?;
        if let Some(&m) = self.marks.get(rel) {
            return Some((m, false));
        }
        // Everything in an untracked or ignored folder is too.
        let inherited = rel
            .ancestors()
            .skip(1)
            .filter(|a| !a.as_os_str().is_empty())
            .find_map(|a| self.marks.get(a).filter(|m| **m <= Mark::Untracked));
        if let Some(&m) = inherited {
            return Some((m, false));
        }
        self.inside.get(rel).map(|&m| (m, true))
    }
}

/// Reads `git status --porcelain=v2 --branch --ignored -z` for the work
/// tree at `root`.
pub fn parse(out: &str, root: PathBuf) -> GitStatus {
    let mut head = None;
    let mut oid = None;
    let mut ahead_behind = None;
    let mut changed = 0;
    let mut marks = HashMap::new();
    let mut records = out.split('\0');
    while let Some(rec) = records.next() {
        if let Some(rest) = rec.strip_prefix("# branch.head ") {
            head = Some(rest.to_string());
        } else if let Some(rest) = rec.strip_prefix("# branch.oid ") {
            oid = Some(rest.to_string());
        } else if let Some(rest) = rec.strip_prefix("# branch.ab ") {
            let mut parts = rest.split(' ');
            let n = |p: Option<&str>, sign| {
                p.and_then(|s| s.strip_prefix(sign))
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0)
            };
            ahead_behind = Some((n(parts.next(), '+'), n(parts.next(), '-')));
        } else if let Some((kind, path, mark)) = entry(rec) {
            if kind == '2' {
                // A rename's old path follows as its own record.
                records.next();
            }
            if mark != Mark::Ignored {
                changed += 1;
            }
            marks.insert(PathBuf::from(path.trim_end_matches('/')), mark);
        }
    }
    // A detached HEAD shows its commit instead.
    let branch = match (head.as_deref(), oid.as_deref()) {
        (Some("(detached)"), Some(oid)) if oid != "(initial)" => oid.chars().take(7).collect(),
        (Some(head), _) => head.to_string(),
        (None, _) => "?".to_string(),
    };
    let mut inside: HashMap<PathBuf, Mark> = HashMap::new();
    for (path, &mark) in marks.iter().filter(|(_, m)| **m != Mark::Ignored) {
        for dir in path.ancestors().skip(1) {
            if dir.as_os_str().is_empty() {
                break;
            }
            let strongest = inside.entry(dir.to_path_buf()).or_insert(mark);
            *strongest = (*strongest).max(mark);
        }
    }
    GitStatus {
        root,
        branch,
        ahead_behind,
        changed,
        marks,
        inside,
    }
}

/// One path record: its type, path and mark.
fn entry(rec: &str) -> Option<(char, &str, Mark)> {
    let kind = rec.chars().next()?;
    // Fields before the path: `1 XY sub mH mI mW hH hI path`, rename and
    // copy add a score, unmerged has three modes and hashes.
    let (fields, mark) = match kind {
        '?' => return Some((kind, rec.get(2..)?, Mark::Untracked)),
        '!' => return Some((kind, rec.get(2..)?, Mark::Ignored)),
        '1' => (8, None),
        '2' => (9, Some(Mark::Added)),
        'u' => (10, Some(Mark::Conflict)),
        _ => return None,
    };
    let mut parts = rec.splitn(fields + 1, ' ');
    let xy = parts.nth(1)?;
    let path = parts.nth(fields - 2)?;
    let mark = mark.unwrap_or_else(|| {
        let mut xy = xy.chars();
        match (xy.next(), xy.next()) {
            (Some('D'), _) | (_, Some('D')) => Mark::Deleted,
            (Some('A'), _) => Mark::Added,
            _ => Mark::Modified,
        }
    });
    Some((kind, path, mark))
}

/// The status of the work tree `dir` is in, or `None` outside one (or
/// without git).
pub fn status(dir: &Path) -> Option<GitStatus> {
    let run = |args: &[&str]| {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        tool::output("git", &args, Some(dir), ENV, TIMEOUT).ok()
    };
    // `dir` relative to the top, so the root keeps tosk's spelling of the
    // path rather than git's resolved one.
    let prefix = run(&["rev-parse", "--show-prefix"])?;
    let depth = Path::new(String::from_utf8_lossy(&prefix).trim())
        .components()
        .count();
    let root = dir.ancestors().nth(depth)?.to_path_buf();
    let out = run(&[
        "--no-optional-locks",
        "status",
        "--porcelain=v2",
        "--branch",
        "--ignored",
        "-z",
    ])?;
    Some(parse(&String::from_utf8_lossy(&out), root))
}

pub struct GitWatcher {
    requests: Sender<PathBuf>,
    pub results: Receiver<(PathBuf, Option<GitStatus>)>,
}

impl GitWatcher {
    pub fn spawn() -> GitWatcher {
        let (requests, rx) = mpsc::channel::<PathBuf>();
        let (tx, results) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(mut dir) = rx.recv() {
                // Folder hopping sends a burst; only the last one counts.
                while let Ok(newer) = rx.recv_timeout(DEBOUNCE) {
                    dir = newer;
                }
                let st = status(&dir);
                if tx.send((dir, st)).is_err() {
                    break;
                }
            }
        });
        GitWatcher { requests, results }
    }

    pub fn request(&self, dir: &Path) {
        let _ = self.requests.send(dir.to_path_buf());
    }
}

/// The repository `path` is in, and its branch, found by looking for
/// `.git` upwards: (work tree root, branch). Cheap enough for the UI
/// thread, unlike `git status`.
pub fn repo_of(path: &Path) -> Option<(PathBuf, String)> {
    let start = if path.is_dir() { path } else { path.parent()? };
    let root = start.ancestors().find(|a| a.join(".git").exists())?;
    let dot_git = root.join(".git");
    // Worktrees and submodules have a `.git` file pointing elsewhere.
    let git_dir = if dot_git.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let target = text.strip_prefix("gitdir:")?.trim();
        root.join(target)
    } else {
        dot_git
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let branch = match head.strip_prefix("ref: ") {
        Some(r) => r.strip_prefix("refs/heads/").unwrap_or(r).to_string(),
        None => head.chars().take(7).collect(),
    };
    Some((root.to_path_buf(), branch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn parses_porcelain_v2() {
        let out = [
            "# branch.oid 1234567890abcdef",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "1 .M N... 100644 100644 100644 a b src/ui/list.rs",
            "1 A. N... 000000 100644 100644 a b src/new file.rs",
            "1 .D N... 100644 100644 000000 a b docs/gone.md",
            "2 R. N... 100644 100644 100644 a b R100 src/moved.rs",
            "src/old.rs",
            "u UU N... 1 2 3 4 a b c conflict.rs",
            "? notes/",
            "! target/",
            "",
        ]
        .join("\0");
        let st = parse(&out, PathBuf::from("/r"));
        assert_eq!(st.branch, "main");
        assert_eq!(st.ahead_behind, Some((2, 1)));
        assert_eq!(st.changed, 6);
        let mark = |p: &str| st.mark(&Path::new("/r").join(p));
        assert_eq!(mark("src/ui/list.rs"), Some((Mark::Modified, false)));
        assert_eq!(mark("src/new file.rs"), Some((Mark::Added, false)));
        assert_eq!(mark("docs/gone.md"), Some((Mark::Deleted, false)));
        assert_eq!(mark("src/moved.rs"), Some((Mark::Added, false)));
        assert_eq!(mark("src/old.rs"), None);
        assert_eq!(mark("conflict.rs"), Some((Mark::Conflict, false)));
        // Folders: their own mark, inherited, or the strongest inside.
        assert_eq!(mark("notes"), Some((Mark::Untracked, false)));
        assert_eq!(mark("notes/deep/a.md"), Some((Mark::Untracked, false)));
        assert_eq!(mark("target/debug"), Some((Mark::Ignored, false)));
        assert_eq!(mark("src"), Some((Mark::Modified, true)));
        assert_eq!(mark("src/ui"), Some((Mark::Modified, true)));
        assert_eq!(mark("docs"), Some((Mark::Deleted, true)));
        assert_eq!(mark("README.md"), None);
        assert_eq!(st.mark(Path::new("/elsewhere/x")), None);

        let detached = "# branch.oid 1234567890abcdef\0# branch.head (detached)\0";
        let st = parse(detached, PathBuf::from("/r"));
        assert_eq!(
            (st.branch.as_str(), st.ahead_behind, st.changed),
            ("1234567", None, 0)
        );
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .envs(ENV.iter().copied())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    /// The milestone's acceptance check: two commits ahead shows ↑2.
    #[test]
    fn ahead_of_upstream() {
        let tmp = TempDir::new().unwrap();
        let (remote, work) = (tmp.path().join("remote"), tmp.path().join("work"));
        fs::create_dir(&remote).unwrap();
        git(&remote, &["init", "-q", "--bare", "-b", "main"]);
        git(tmp.path(), &["clone", "-q", "remote", "work"]);
        git(&work, &["checkout", "-q", "-b", "main"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git(&work, &["push", "-q", "-u", "origin", "main"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&work, &["commit", "-q", "--allow-empty", "-m", "two"]);
        fs::write(work.join("dirty.txt"), "x").unwrap();
        fs::create_dir(work.join("sub")).unwrap();

        let w = GitWatcher::spawn();
        w.request(&work.join("sub"));
        let (dir, st) = w.results.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(dir, work.join("sub"));
        let st = st.unwrap();
        assert_eq!(st.root, work);
        assert_eq!(st.branch, "main");
        assert_eq!(st.ahead_behind, Some((2, 0)));
        assert_eq!(st.changed, 1);
        assert_eq!(
            st.mark(&work.join("dirty.txt")),
            Some((Mark::Untracked, false))
        );
        assert_eq!(status(tmp.path()), None);
        // Reached through a symlink, the root keeps that spelling.
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&work, &link).unwrap();
        let st = status(&link.join("sub")).unwrap();
        assert_eq!(st.root, link);
        assert!(st.mark(&link.join("dirty.txt")).is_some());
        assert_eq!(
            repo_of(&work.join("dirty.txt")),
            Some((work.clone(), "main".into()))
        );
        assert_eq!(repo_of(&work.join("sub")), Some((work, "main".into())));
    }
}
