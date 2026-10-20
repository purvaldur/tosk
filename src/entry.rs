use std::ffi::OsString;
use std::path::PathBuf;
use std::time::SystemTime;

/// What an entry is, after following a symlink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    /// Sockets, fifos, devices.
    Other,
    /// A symlink whose target doesn't exist.
    Missing,
}

#[derive(Clone, Debug)]
pub struct Entry {
    /// Name as stored on disk, used to build paths.
    pub file_name: OsString,
    /// Name for display; lossy if the name isn't UTF-8.
    pub name: String,
    pub kind: Kind,
    pub size: u64,
    pub mtime: Option<SystemTime>,
    /// The raw `readlink` target when this entry is a symlink.
    pub link_target: Option<PathBuf>,
    /// Direct children, for folders that could be read.
    pub children: Option<usize>,
}

impl Entry {
    pub fn new(file_name: OsString, kind: Kind) -> Self {
        Entry {
            name: file_name.to_string_lossy().into_owned(),
            file_name,
            kind,
            size: 0,
            mtime: None,
            link_target: None,
            children: None,
        }
    }

    /// Folders and symlinks to folders.
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    pub fn is_link(&self) -> bool {
        self.link_target.is_some()
    }

    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}
