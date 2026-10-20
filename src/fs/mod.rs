pub mod local;

use std::io;
use std::path::{Path, PathBuf};

use crate::entry::Entry;

/// Filesystem access, so the remote backend (milestone 9) can slot in.
pub trait Vfs: Send + Sync {
    /// Every entry in `dir`, unsorted, hidden ones included. Folder item
    /// counts are left out; they come from `count_children` on a worker.
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>>;
    /// Number of direct children of a folder.
    fn count_children(&self, dir: &Path) -> io::Result<usize>;
    /// The absolute path with every symlink resolved.
    fn resolve(&self, path: &Path) -> io::Result<PathBuf>;
}
