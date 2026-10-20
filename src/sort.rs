use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

use crate::entry::Entry;

/// Folders first (symlinks to folders count), then natural, case-insensitive
/// order ignoring a leading dot.
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(compare);
}

pub fn compare(a: &Entry, b: &Entry) -> Ordering {
    b.is_dir()
        .cmp(&a.is_dir())
        .then_with(|| compare_names(&a.name, &b.name))
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
        sort_entries(&mut v);
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["src", "zz-link", "a.txt", "broken"]);
    }
}
