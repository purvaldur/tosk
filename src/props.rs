//! The properties overlay (`p`): fields for one item, or a summary of
//! several, with folder sizes counted on a `du` thread.

use std::collections::BTreeSet;
use std::fs::Metadata;
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;

use crate::du::{Du, Totals};
use crate::format::{count, exact_size, full_date, human_size, mode_string, owner, tilde};
use crate::git;
use crate::preview::classify;

/// Names listed under the fields when several items are selected.
const MAX_NAMES: usize = 5;

enum Value {
    Text(String),
    /// A folder's recursive contents and size.
    Contents,
    /// The recursive size of everything selected.
    TotalSize,
    /// How many files and folders that took.
    Counted,
}

pub struct Props {
    /// Shown at the top right: the name, or `N items`.
    pub title: String,
    fields: Vec<(&'static str, Value)>,
    /// Several items: the first few names, and how many more there are.
    pub names: Vec<String>,
    pub more: usize,
    du: Option<Du>,
}

/// One row as drawn: label, value, and whether it's still being counted.
pub struct Row {
    pub label: &'static str,
    pub value: String,
    pub pending: bool,
}

impl Props {
    /// Gathers the fields for `paths` (at least one). Only folder sizes are
    /// left to the background.
    pub fn new(paths: &[PathBuf], home: Option<&Path>, tz: &TimeZone) -> Props {
        match paths {
            [one] => single(one, home, tz),
            many => several(many, home),
        }
    }

    /// Whether folder sizes are still being counted.
    pub fn counting(&self) -> bool {
        self.du.as_ref().is_some_and(|d| !d.totals.done)
    }

    /// Takes progress from the count; returns whether anything changed.
    pub fn drain(&mut self) -> bool {
        self.du.as_mut().is_some_and(Du::drain)
    }

    pub fn rows(&self) -> Vec<Row> {
        let totals = self.du.as_ref().map(|d| d.totals).unwrap_or_default();
        let pending = !totals.done;
        self.fields
            .iter()
            .map(|(label, value)| {
                let (value, pending) = match value {
                    Value::Text(s) => (s.clone(), false),
                    _ if pending => (calculating(&totals), true),
                    Value::Contents => (contents(&totals), false),
                    Value::TotalSize => (exact_size(totals.bytes), false),
                    Value::Counted => (counted(&totals), false),
                };
                Row {
                    label,
                    value,
                    pending,
                }
            })
            .collect()
    }
}

fn single(path: &Path, home: Option<&Path>, tz: &TimeZone) -> Props {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let link = std::fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.file_type().is_symlink());
    // Through a symlink to what it points at, when that exists.
    let meta = std::fs::metadata(path).ok();
    let mut fields = vec![("Name", Value::Text(name.clone()))];
    let kind = match (&link, &meta) {
        (Some(_), target) => {
            let to = std::fs::read_link(path)
                .map(|t| t.display().to_string())
                .unwrap_or_default();
            let broken = if target.is_none() { " (missing)" } else { "" };
            format!("Symbolic link → {to}{broken}")
        }
        (None, Some(m)) => kind_label(path, m),
        (None, None) => "Unreadable".to_string(),
    };
    fields.push(("Type", Value::Text(kind)));
    let is_dir = meta.as_ref().is_some_and(Metadata::is_dir);
    let mut du = None;
    if is_dir {
        fields.push(("Contents", Value::Contents));
        du = Some(Du::spawn(vec![path.to_path_buf()]));
    } else if let Some(m) = &meta {
        fields.push(("Size", Value::Text(exact_size(m.len()))));
    }
    if let Some(parent) = path.parent() {
        fields.push(("Location", Value::Text(tilde(parent, home))));
    }
    if let Some(t) = meta.as_ref().and_then(|m| m.modified().ok()) {
        fields.push(("Modified", Value::Text(full_date(t, tz))));
    }
    if let Some(m) = meta.as_ref().or(link.as_ref()) {
        let perms = format!("{}  {}", mode_string(m.mode()), owner(m.uid(), m.gid()));
        fields.push(("Permissions", Value::Text(perms)));
    }
    if let Some((root, branch)) = git::repo_of(path) {
        let git = format!("{branch} · in {}", tilde(&root, home));
        fields.push(("Git", Value::Text(git)));
    }
    Props {
        title: name,
        fields,
        names: Vec::new(),
        more: 0,
        du,
    }
}

fn several(paths: &[PathBuf], home: Option<&Path>) -> Props {
    let folders = paths.iter().filter(|p| p.is_dir()).count();
    let files = paths.len() - folders;
    let mut selected = Vec::new();
    if folders > 0 {
        selected.push(count(folders, "folder", "folders"));
    }
    if files > 0 {
        selected.push(count(files, "file", "files"));
    }
    let parents: BTreeSet<&Path> = paths.iter().filter_map(|p| p.parent()).collect();
    let location = match parents.iter().collect::<Vec<_>>().as_slice() {
        [one] => tilde(one, home),
        many => count(many.len(), "place", "places"),
    };
    let fields = vec![
        ("Selected", Value::Text(selected.join(", "))),
        ("Total size", Value::TotalSize),
        ("Counted", Value::Counted),
        ("Location", Value::Text(location)),
    ];
    let names = paths
        .iter()
        .take(MAX_NAMES)
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    Props {
        title: count(paths.len(), "item", "items"),
        fields,
        names,
        more: paths.len().saturating_sub(MAX_NAMES),
        du: Some(Du::spawn(paths.to_vec())),
    }
}

/// `Folder`, `Text (.md)`, `Image (.png)`, `Socket`, …
fn kind_label(path: &Path, meta: &Metadata) -> String {
    let ft = meta.file_type();
    if ft.is_dir() {
        return "Folder".into();
    }
    if ft.is_fifo() {
        return "Named pipe".into();
    }
    if ft.is_socket() {
        return "Socket".into();
    }
    if ft.is_block_device() || ft.is_char_device() {
        return "Device".into();
    }
    let mut head = Vec::new();
    let _ = std::fs::File::open(path).and_then(|f| f.take(8 * 1024).read_to_end(&mut head));
    let label = classify(path, &head).label();
    match path.extension() {
        Some(ext) => format!("{label} (.{})", ext.to_string_lossy()),
        None => label.to_string(),
    }
}

fn calculating(t: &Totals) -> String {
    if t.bytes == 0 {
        "calculating…".into()
    } else {
        format!("calculating… {} so far", human_size(t.bytes))
    }
}

fn unreadable(t: &Totals) -> String {
    match t.unreadable {
        0 => String::new(),
        n => format!(" · {} unreadable", count(n as usize, "folder", "folders")),
    }
}

/// `340 files, 25 folders · 4.1 MB (4,100,000 bytes)`; the folder itself
/// isn't one of its folders.
fn contents(t: &Totals) -> String {
    let inner = t.folders.saturating_sub(1) as usize;
    format!(
        "{}, {} · {}{}",
        count(t.files as usize, "file", "files"),
        count(inner, "folder", "folders"),
        exact_size(t.bytes),
        unreadable(t)
    )
}

/// `12 files in 3 folders`.
fn counted(t: &Totals) -> String {
    let files = count(t.files as usize, "file", "files");
    let folders = match t.folders {
        0 => String::new(),
        n => format!(" in {}", count(n as usize, "folder", "folders")),
    };
    format!("{files}{folders}{}", unreadable(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    fn settle(p: &mut Props) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while p.counting() {
            assert!(Instant::now() < deadline, "du didn't finish");
            p.drain();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn value<'a>(rows: &'a [Row], label: &str) -> &'a str {
        rows.iter()
            .find(|r| r.label == label)
            .map(|r| r.value.as_str())
            .unwrap_or_else(|| panic!("no {label} row"))
    }

    fn labels(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.label).collect()
    }

    #[test]
    fn one_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("notes.md");
        fs::write(&path, "x".repeat(1200)).unwrap();
        let p = Props::new(std::slice::from_ref(&path), None, &TimeZone::UTC);
        assert!(!p.counting());
        let rows = p.rows();
        assert_eq!(
            labels(&rows),
            [
                "Name",
                "Type",
                "Size",
                "Location",
                "Modified",
                "Permissions"
            ]
        );
        assert_eq!(value(&rows, "Name"), "notes.md");
        assert_eq!(value(&rows, "Type"), "Text (.md)");
        assert_eq!(value(&rows, "Size"), "1.2 kB (1,200 bytes)");
        assert_eq!(value(&rows, "Location"), tmp.path().display().to_string());
        assert!(value(&rows, "Permissions").starts_with("-rw"));
    }

    #[test]
    fn folder_contents_count_in_the_background() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("src");
        fs::create_dir_all(dir.join("ui")).unwrap();
        fs::write(dir.join("main.rs"), "12345").unwrap();
        fs::write(dir.join("ui/mod.rs"), "123").unwrap();
        let mut p = Props::new(std::slice::from_ref(&dir), None, &TimeZone::UTC);
        assert_eq!(value(&p.rows(), "Type"), "Folder");
        // Straight after opening, nothing has been counted yet.
        let rows = p.rows();
        let contents = rows.iter().find(|r| r.label == "Contents").unwrap();
        assert!(contents.pending);
        assert_eq!(contents.value, "calculating…");
        settle(&mut p);
        assert_eq!(value(&p.rows(), "Contents"), "2 files, 1 folder · 8 bytes");
    }

    #[test]
    fn links_and_git() {
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("repo/.git")).unwrap();
        fs::write(r.join("repo/.git/HEAD"), "ref: refs/heads/trunk\n").unwrap();
        fs::write(r.join("repo/a.png"), "").unwrap();
        symlink("a.png", r.join("repo/link")).unwrap();
        symlink("gone", r.join("repo/broken")).unwrap();
        let tz = TimeZone::UTC;
        let rows = Props::new(&[r.join("repo/a.png")], Some(r), &tz).rows();
        assert_eq!(value(&rows, "Type"), "Image (.png)");
        assert_eq!(value(&rows, "Git"), "trunk · in ~/repo");
        assert_eq!(value(&rows, "Location"), "~/repo");
        let rows = Props::new(&[r.join("repo/link")], Some(r), &tz).rows();
        assert_eq!(value(&rows, "Type"), "Symbolic link → a.png");
        let rows = Props::new(&[r.join("repo/broken")], Some(r), &tz).rows();
        assert_eq!(value(&rows, "Type"), "Symbolic link → gone (missing)");
        assert!(value(&rows, "Permissions").starts_with("lrwx"));
    }

    #[test]
    fn several_items() {
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("d/e")).unwrap();
        fs::write(r.join("d/e/x"), "1234").unwrap();
        let mut paths = vec![r.join("d")];
        for i in 0..6 {
            let f = r.join(format!("f{i}"));
            fs::write(&f, "12").unwrap();
            paths.push(f);
        }
        let mut p = Props::new(&paths, None, &TimeZone::UTC);
        assert_eq!(p.title, "7 items");
        settle(&mut p);
        let rows = p.rows();
        assert_eq!(value(&rows, "Selected"), "1 folder, 6 files");
        assert_eq!(value(&rows, "Total size"), "16 bytes");
        assert_eq!(value(&rows, "Counted"), "7 files in 2 folders");
        assert_eq!(value(&rows, "Location"), r.display().to_string());
        assert_eq!(p.names, ["d", "f0", "f1", "f2", "f3"]);
        assert_eq!(p.more, 2);

        let p = Props::new(&[r.join("f0"), r.join("d/e/x")], None, &TimeZone::UTC);
        assert_eq!(value(&p.rows(), "Location"), "2 places");
    }
}
