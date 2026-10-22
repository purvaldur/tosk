use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

use crate::entry::Entry;

/// What the list is ordered by; `s` cycles through them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Name,
    /// By extension, then name.
    Type,
    /// Largest first. Folders stay by name, since their counts arrive late.
    Size,
    /// Newest first.
    Modified,
}

impl SortBy {
    pub fn next(self) -> SortBy {
        match self {
            SortBy::Name => SortBy::Type,
            SortBy::Type => SortBy::Size,
            SortBy::Size => SortBy::Modified,
            SortBy::Modified => SortBy::Name,
        }
    }
}

/// What to sort by and whether `S` flipped it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sort {
    pub by: SortBy,
    pub reverse: bool,
}

impl Sort {
    pub fn label(self) -> &'static str {
        match (self.by, self.reverse) {
            (SortBy::Name, false) => "name",
            (SortBy::Name, true) => "name, Z to A",
            (SortBy::Type, false) => "type",
            (SortBy::Type, true) => "type, reversed",
            (SortBy::Size, false) => "size, largest first",
            (SortBy::Size, true) => "size, smallest first",
            (SortBy::Modified, false) => "modified, newest first",
            (SortBy::Modified, true) => "modified, oldest first",
        }
    }
}

/// Folders first (symlinks to folders count), then `sort.by`, with natural,
/// case-insensitive names (ignoring a leading dot) breaking ties. Reversing
/// flips everything but folders-first.
pub fn sort_entries(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| compare(a, b, sort));
}

pub fn compare(a: &Entry, b: &Entry, sort: Sort) -> Ordering {
    let dirs = a.is_dir() && b.is_dir();
    let key = match sort.by {
        SortBy::Name => Ordering::Equal,
        SortBy::Type if dirs => Ordering::Equal,
        SortBy::Type => extension(&a.name).cmp(&extension(&b.name)),
        SortBy::Size if dirs => Ordering::Equal,
        SortBy::Size => b.size.cmp(&a.size),
        SortBy::Modified => b.mtime.cmp(&a.mtime),
    };
    let order = key.then_with(|| compare_names(&a.name, &b.name));
    let order = if sort.reverse { order.reverse() } else { order };
    b.is_dir().cmp(&a.is_dir()).then(order)
}

/// Lowercased, empty for none; a leading dot doesn't start one.
fn extension(name: &str) -> String {
    match strip_dot(name).rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_lowercase(),
        _ => String::new(),
    }
}

pub fn compare_names(a: &str, b: &str) -> Ordering {
    // Fall back to the raw names so the order is total and stable.
    natural_cmp(strip_dot(a), strip_dot(b)).then_with(|| a.cmp(b))
}

fn strip_dot(s: &str) -> &str {
    s.strip_prefix('.').unwrap_or(s)
}

/// Digit runs compare as numbers (`e5` after `e04`), everything else
/// compares case-insensitively.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        let ord = match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (da, db) = (take_digits(&mut a), take_digits(&mut b));
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                // Compare by length first so numbers of any size work.
                ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb))
            }
            (Some(x), Some(y)) => {
                a.next();
                b.next();
                x.to_lowercase().cmp(y.to_lowercase())
            }
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
}

fn take_digits(it: &mut Peekable<Chars>) -> String {
    let mut s = String::new();
    while let Some(&c) = it.peek() {
        if !c.is_ascii_digit() {
            break;
        }
        s.push(c);
        it.next();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Kind;

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        v.sort_by(|a, b| compare_names(a, b));
        v
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(
            sorted(&["e5", "e04", "e10", "e1"]),
            ["e1", "e04", "e5", "e10"]
        );
        assert_eq!(
            sorted(&["show.s01.e10.mkv", "show.s01.e9.mkv", "show.s01.e02.mkv"]),
            ["show.s01.e02.mkv", "show.s01.e9.mkv", "show.s01.e10.mkv"]
        );
    }

    #[test]
    fn huge_numbers_do_not_overflow() {
        assert_eq!(
            sorted(&["a99999999999999999999999", "a100000000000000000000000"]),
            ["a99999999999999999999999", "a100000000000000000000000"]
        );
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(
            sorted(&["banana", "Apple", "cherry"]),
            ["Apple", "banana", "cherry"]
        );
        assert_eq!(sorted(&["Ärger", "ärgern"]), ["Ärger", "ärgern"]);
    }

    #[test]
    fn leading_dot_is_ignored() {
        assert_eq!(
            sorted(&["config", ".bashrc", "Downloads", ".cache"]),
            [".bashrc", ".cache", "config", "Downloads"]
        );
    }

    #[test]
    fn ties_are_stable() {
        assert_eq!(sorted(&["a", "A", ".a"]), [".a", "A", "a"]);
        assert_eq!(sorted(&["e1", "e01", "e001"]), ["e001", "e01", "e1"]);
    }

    #[test]
    fn folders_first_including_links_to_folders() {
        let mut dir_link = Entry::new("zz-link".into(), Kind::Dir);
        dir_link.link_target = Some("/tmp".into());
        let mut v = vec![
            Entry::new("a.txt".into(), Kind::File),
            dir_link,
            Entry::new("broken".into(), Kind::Missing),
            Entry::new("src".into(), Kind::Dir),
        ];
        sort_entries(&mut v, Sort::default());
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["src", "zz-link", "a.txt", "broken"]);
    }

    fn entry(name: &str, kind: Kind, size: u64, age_secs: u64) -> Entry {
        let mut e = Entry::new(name.into(), kind);
        e.size = size;
        e.mtime =
            Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 - age_secs));
        e
    }

    fn order(by: SortBy) -> Vec<String> {
        order_in(Sort { by, reverse: false })
    }

    fn order_in(sort: Sort) -> Vec<String> {
        let mut v = vec![
            entry("b.txt", Kind::File, 10, 5),
            entry("a.rs", Kind::File, 300, 50),
            entry("zz", Kind::Dir, 0, 1),
            entry("c.TXT", Kind::File, 20, 30),
            entry("Makefile", Kind::File, 300, 10),
            entry("aa", Kind::Dir, 0, 100),
        ];
        sort_entries(&mut v, sort);
        v.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn sort_modes() {
        assert_eq!(
            order(SortBy::Name),
            ["aa", "zz", "a.rs", "b.txt", "c.TXT", "Makefile"]
        );
        // No extension sorts first; ties go by name.
        assert_eq!(
            order(SortBy::Type),
            ["aa", "zz", "Makefile", "a.rs", "b.txt", "c.TXT"]
        );
        // Folders keep name order; equal sizes go by name.
        assert_eq!(
            order(SortBy::Size),
            ["aa", "zz", "a.rs", "Makefile", "c.TXT", "b.txt"]
        );
        assert_eq!(
            order(SortBy::Modified),
            ["zz", "aa", "b.txt", "Makefile", "c.TXT", "a.rs"]
        );
    }

    #[test]
    fn reversed_keeps_folders_on_top() {
        let rev = |by| order_in(Sort { by, reverse: true });
        assert_eq!(
            rev(SortBy::Size),
            ["zz", "aa", "b.txt", "c.TXT", "Makefile", "a.rs"]
        );
        assert_eq!(
            rev(SortBy::Modified),
            ["aa", "zz", "a.rs", "c.TXT", "Makefile", "b.txt"]
        );
        assert_eq!(
            rev(SortBy::Name),
            ["zz", "aa", "Makefile", "c.TXT", "b.txt", "a.rs"]
        );
    }

    #[test]
    fn cycle() {
        let mut by = SortBy::default();
        for _ in 0..4 {
            by = by.next();
        }
        assert_eq!(by, SortBy::Name);
        let label = |by, reverse| Sort { by, reverse }.label();
        assert_eq!(label(SortBy::Size, true), "size, smallest first");
        assert_eq!(label(SortBy::Modified, true), "modified, oldest first");
    }
}
