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
    /// Whether anything (a broken symlink included) exists at `path`.
    fn exists(&self, path: &Path) -> bool;
    /// Renames without checking the target; callers make sure it's free.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Moves to the freedesktop trash, restorable from any file manager.
    fn trash(&self, paths: &[PathBuf]) -> io::Result<()>;
    /// Creates an empty file; fails if anything is already there.
    fn create_file(&self, path: &Path) -> io::Result<()>;
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    /// Removes for good. Symlinks are removed, never followed.
    fn delete(&self, path: &Path) -> io::Result<()>;
}
