//! Remote folders are listed on this worker, so a slow link or a first
//! connection never holds up input. Only the newest request is answered.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use super::Vfs;
use crate::entry::Entry;

struct Job {
    id: u64,
    vfs: Arc<dyn Vfs>,
    /// `None` for the home folder.
    dir: Option<PathBuf>,
}

pub struct Listed {
    pub id: u64,
    /// The folder listed, resolved when it was the home folder.
    pub dir: PathBuf,
    pub result: io::Result<Vec<Entry>>,
}

pub struct Lister {
    jobs: Sender<Job>,
    pub results: Receiver<Listed>,
}

impl Lister {
    pub fn spawn() -> Lister {
        let (jobs, rx) = mpsc::channel::<Job>();
        let (tx, results) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(mut job) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                let (dir, result) = match job.dir {
                    Some(dir) => {
                        let r = job.vfs.list(&dir);
                        (dir, r)
                    }
                    // The server starts in the home folder.
                    None => match job.vfs.resolve(Path::new(".")) {
                        Ok(home) => {
                            let r = job.vfs.list(&home);
                            (home, r)
                        }
                        Err(e) => (PathBuf::new(), Err(e)),
                    },
                };
                let listed = Listed {
                    id: job.id,
                    dir,
                    result,
                };
                if tx.send(listed).is_err() {
                    break;
                }
            }
        });
        Lister { jobs, results }
    }

    pub fn request(&self, id: u64, vfs: Arc<dyn Vfs>, dir: Option<PathBuf>) {
        let _ = self.jobs.send(Job { id, vfs, dir });
    }
}
