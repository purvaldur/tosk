pub mod lister;
pub mod local;
pub mod sftp;

use std::io;
use std::path::{Path, PathBuf};

use crate::entry::Entry;

/// An entry with the details properties shows.
pub struct Meta {
    pub entry: Entry,
    /// The raw `st_mode`, of the target for a symlink.
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

/// Filesystem access, local or over ssh.
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
    /// At most `max` bytes from the start of a file.
    fn read(&self, path: &Path, max: u64) -> io::Result<Vec<u8>>;
    /// The entry for `path` itself, with mode and owner.
    fn stat(&self, path: &Path) -> io::Result<Meta>;
}
