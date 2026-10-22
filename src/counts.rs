//! Folder item counts ("N items") for the file list, on a worker so a
//! folder full of folders opens instantly.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::fs::Vfs;

/// Send what's counted at least this often, so the list fills in smoothly.
const BATCH_EVERY: Duration = Duration::from_millis(30);

struct Job {
    vfs: Arc<dyn Vfs>,
    generation: u64,
    dir: PathBuf,
    names: Vec<OsString>,
}

pub struct Counted {
    pub dir: PathBuf,
    pub counts: Vec<(OsString, Option<usize>)>,
}

pub struct Counter {
    jobs: Sender<Job>,
    pub results: Receiver<Counted>,
    /// Bumped by every request; the worker drops jobs that fall behind.
    generation: Arc<AtomicU64>,
}

impl Counter {
    pub fn spawn() -> Counter {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (result_tx, results) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        thread::spawn(move || {
            for job in job_rx {
                if !run(job, &current, &result_tx) {
                    break;
                }
            }
        });
        Counter {
            jobs,
            results,
            generation,
        }
    }

    /// Count the children of each named folder in `dir`, replacing any
    /// request still in progress.
    pub fn request(&self, vfs: Arc<dyn Vfs>, dir: &Path, names: Vec<OsString>) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if names.is_empty() {
            return;
        }
        let _ = self.jobs.send(Job {
            vfs,
            generation,
            dir: dir.to_path_buf(),
            names,
        });
    }
}

/// Returns false once the receiving side is gone.
fn run(job: Job, current: &AtomicU64, tx: &Sender<Counted>) -> bool {
    let vfs = &*job.vfs;
    let mut batch = Vec::new();
    let mut last = Instant::now();
    for name in job.names {
        if current.load(Ordering::SeqCst) != job.generation {
            return true;
        }
        let n = vfs.count_children(&job.dir.join(&name)).ok();
        batch.push((name, n));
        if last.elapsed() >= BATCH_EVERY {
            let counts = std::mem::take(&mut batch);
            if tx
                .send(Counted {
                    dir: job.dir.clone(),
                    counts,
                })
                .is_err()
            {
                return false;
            }
            last = Instant::now();
        }
    }
    batch.is_empty()
        || tx
            .send(Counted {
                dir: job.dir,
                counts: batch,
            })
            .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::local::Local;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn counts_arrive() {
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("a/x")).unwrap();
        fs::write(r.join("a/y"), "").unwrap();
        fs::create_dir(r.join("empty")).unwrap();
        let c = Counter::spawn();
        c.request(
            Arc::new(Local),
            r,
            vec!["a".into(), "empty".into(), "gone".into()],
        );
        let mut got = Vec::new();
        while got.len() < 3 {
            let batch = c.results.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(batch.dir, r);
            got.extend(batch.counts);
        }
        got.sort();
        assert_eq!(
            got,
            [
                ("a".into(), Some(2)),
                ("empty".into(), Some(0)),
                ("gone".into(), None)
            ]
        );
    }
}
