use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use std::io::Read;
use std::os::unix::fs::MetadataExt;

use super::{Meta, Vfs};
use crate::entry::{Entry, Kind};

pub struct Local;

impl Vfs for Local {
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        // An entry that vanished mid-listing is simply skipped.
        for de in fs::read_dir(dir)?.flatten() {
            out.push(entry_for(&de.path(), de.file_name()));
        }
        Ok(out)
    }

    fn count_children(&self, dir: &Path) -> io::Result<usize> {
        Ok(fs::read_dir(dir)?.count())
    }

    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }

    fn exists(&self, path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn trash(&self, paths: &[PathBuf]) -> io::Result<()> {
        trash::delete_all(paths).map_err(|e| io::Error::other(e.to_string()))
    }

    fn create_file(&self, path: &Path) -> io::Result<()> {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(drop)
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        fs::create_dir(path)
    }

    fn delete(&self, path: &Path) -> io::Result<()> {
        if fs::symlink_metadata(path)?.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
    }

    fn read(&self, path: &Path, max: u64) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        fs::File::open(path)?.take(max).read_to_end(&mut out)?;
        Ok(out)
    }

    fn stat(&self, path: &Path) -> io::Result<Meta> {
        let lmeta = fs::symlink_metadata(path)?;
        let m = fs::metadata(path).unwrap_or(lmeta);
        let name = path.file_name().unwrap_or_default().to_os_string();
        Ok(Meta {
            entry: entry_for(path, name),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
        })
    }
}

fn entry_for(path: &Path, file_name: OsString) -> Entry {
    let lmeta = fs::symlink_metadata(path).ok();
    let is_link = lmeta.as_ref().is_some_and(|m| m.file_type().is_symlink());
    // For symlinks, describe the target.
    let meta = if is_link {
        fs::metadata(path).ok()
    } else {
        lmeta.clone()
    };
    let kind = match &meta {
        Some(m) if m.is_dir() => Kind::Dir,
        Some(m) if m.is_file() => Kind::File,
        Some(_) => Kind::Other,
        None => Kind::Missing,
    };
    let mut e = Entry::new(file_name, kind);
    if is_link {
        e.link_target = Some(fs::read_link(path).unwrap_or_default());
    }
    if let Some(m) = meta.or(lmeta) {
        e.size = m.len();
        e.mtime = m.modified().ok();
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn find<'a>(v: &'a [Entry], name: &str) -> &'a Entry {
        v.iter().find(|e| e.name == name).unwrap()
    }

    #[test]
    fn lists_kinds_sizes_and_links() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("notes.md"), "hello").unwrap();
        fs::write(root.join(".hidden"), "").unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        symlink("src", root.join("src-link")).unwrap();
        symlink("notes.md", root.join("notes-link")).unwrap();
        symlink("nowhere", root.join("broken")).unwrap();

        let v = Local.list(root).unwrap();
        assert_eq!(v.len(), 6);

        let notes = find(&v, "notes.md");
        assert_eq!((notes.kind, notes.size), (Kind::File, 5));
        assert!(!notes.is_link() && notes.mtime.is_some());

        let src = find(&v, "src");
        assert_eq!((src.kind, src.children), (Kind::Dir, None));
        assert_eq!(Local.count_children(&root.join("src")).unwrap(), 2);

        assert!(find(&v, ".hidden").is_hidden());

        let link = find(&v, "src-link");
        assert_eq!(link.kind, Kind::Dir);
        assert_eq!(link.link_target.as_deref(), Some(Path::new("src")));
        assert_eq!(Local.count_children(&root.join("src-link")).unwrap(), 2);

        let flink = find(&v, "notes-link");
        assert_eq!((flink.kind, flink.size), (Kind::File, 5));

        let broken = find(&v, "broken");
        assert_eq!(broken.kind, Kind::Missing);
        assert_eq!(broken.link_target.as_deref(), Some(Path::new("nowhere")));
    }

    #[test]
    fn delete_removes_links_not_their_targets() {
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("keep/inner")).unwrap();
        fs::write(r.join("keep/inner/f"), "x").unwrap();
        symlink("keep", r.join("link")).unwrap();
        fs::create_dir_all(r.join("tree/sub")).unwrap();
        fs::write(r.join("tree/sub/f"), "x").unwrap();
        fs::write(r.join("file"), "x").unwrap();

        Local.delete(&r.join("link")).unwrap();
        assert!(!Local.exists(&r.join("link")));
        assert!(r.join("keep/inner/f").exists());
        Local.delete(&r.join("tree")).unwrap();
        Local.delete(&r.join("file")).unwrap();
        assert!(!Local.exists(&r.join("tree")) && !Local.exists(&r.join("file")));
        assert!(Local.delete(&r.join("gone")).is_err());
    }

    #[test]
    fn creates_files_and_folders_without_overwriting() {
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::write(r.join("taken"), "keep me").unwrap();
        Local.create_file(&r.join("new.txt")).unwrap();
        Local.create_dir(&r.join("dir")).unwrap();
        assert!(r.join("new.txt").is_file() && r.join("dir").is_dir());
        assert!(Local.create_file(&r.join("taken")).is_err());
        assert!(Local.create_dir(&r.join("taken")).is_err());
        assert_eq!(fs::read_to_string(r.join("taken")).unwrap(), "keep me");
    }

    #[test]
    fn missing_folder_is_an_error() {
        let tmp = TempDir::new().unwrap();
        assert!(Local.list(&tmp.path().join("gone")).is_err());
    }
}
