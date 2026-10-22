//! Recursive sizes for properties, counted on a thread so a huge folder
//! never holds up input. Symlinks inside folders aren't followed.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// How often running totals are sent while counting.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Everything that isn't a folder: files, symlinks, sockets.
    pub files: u64,
    pub folders: u64,
    /// Apparent size of the files, like Nautilus.
    pub bytes: u64,
    /// Folders that couldn't be read.
    pub unreadable: u64,
    pub done: bool,
}

/// A count in progress; dropping it stops the thread.
pub struct Du {
    rx: Receiver<Totals>,
    cancel: Arc<AtomicBool>,
    pub totals: Totals,
}

impl Du {
    /// Counts everything under `paths`. The paths themselves are followed if
    /// they're symlinks; what's inside folders isn't.
    pub fn spawn(paths: Vec<PathBuf>) -> Du {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        thread::spawn(move || count(paths, &stop, &tx));
        Du {
            rx,
            cancel,
            totals: Totals::default(),
        }
    }

    /// Takes the newest totals; returns whether they changed.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok(t) = self.rx.try_recv() {
            self.totals = t;
            changed = true;
        }
        changed
    }
}

impl Drop for Du {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn count(paths: Vec<PathBuf>, stop: &AtomicBool, tx: &Sender<Totals>) {
    let mut t = Totals::default();
    let mut stack = Vec::new();
    for path in paths {
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => {
                t.folders += 1;
                stack.push(path);
            }
            Ok(m) => {
                t.files += 1;
                t.bytes += m.len();
            }
            // A broken symlink is still something selected.
            Err(_) => t.files += 1,
        }
    }
    let mut last = Instant::now();
    while let Some(dir) = stack.pop() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            t.unreadable += 1;
            continue;
        };
        for entry in read.flatten() {
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => {
                    t.folders += 1;
                    stack.push(entry.path());
                }
                _ => {
                    t.files += 1;
                    t.bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }
        if last.elapsed() >= PROGRESS_EVERY {
            if tx.send(t).is_err() {
                return;
            }
            last = Instant::now();
        }
    }
    t.done = true;
    let _ = tx.send(t);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn finish(paths: Vec<PathBuf>) -> Totals {
        let mut du = Du::spawn(paths);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !du.totals.done {
            assert!(Instant::now() < deadline, "du didn't finish");
            du.drain();
            thread::sleep(Duration::from_millis(5));
        }
        du.totals
    }

    #[test]
    fn counts_recursively_without_following_inner_links() {
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("top/a/b")).unwrap();
        fs::write(r.join("top/one"), "12345").unwrap();
        fs::write(r.join("top/a/b/two"), "123").unwrap();
        fs::create_dir(r.join("big")).unwrap();
        fs::write(r.join("big/huge"), vec![0u8; 10_000]).unwrap();
        // Inside the folder: counted as a link, not walked into.
        symlink(r.join("big"), r.join("top/to-big")).unwrap();
        fs::write(r.join("loose"), "1").unwrap();

        let t = finish(vec![r.join("top")]);
        assert_eq!((t.files, t.folders, t.unreadable), (3, 3, 0));
        let link_len = fs::symlink_metadata(r.join("top/to-big")).unwrap().len();
        assert_eq!(t.bytes, 8 + link_len);

        // Several paths, and a top-level link is followed.
        symlink(r.join("big"), r.join("big-link")).unwrap();
        let t = finish(vec![r.join("big-link"), r.join("loose")]);
        assert_eq!((t.files, t.folders, t.bytes), (2, 1, 10_001));
    }
}
