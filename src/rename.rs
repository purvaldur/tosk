//! Rename planning and the two-phase apply that makes swaps work.
//!
//! Bulk templates: `[01]` is a counter from 1 padded to the digits written
//! (`[001]` → 001, 002, …), `[a-b]` starts at `a`, `{name}` is the old name
//! without its extension and `{ext}` the extension without the dot.

use std::path::Path;

use crate::fs::Vfs;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Lit(String),
    Counter { start: u64, width: usize },
    Name,
    Ext,
}

fn parse(template: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut lit = String::new();
    let mut rest = template;
    while let Some(c) = rest.chars().next() {
        let token = match c {
            '[' => counter(rest),
            '{' if rest.starts_with("{name}") => Some((Token::Name, 6)),
            '{' if rest.starts_with("{ext}") => Some((Token::Ext, 5)),
            _ => None,
        };
        match token {
            Some((token, len)) => {
                if !lit.is_empty() {
                    tokens.push(Token::Lit(std::mem::take(&mut lit)));
                }
                tokens.push(token);
                rest = &rest[len..];
            }
            None => {
                lit.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    if !lit.is_empty() {
        tokens.push(Token::Lit(lit));
    }
    tokens
}

/// `[01]` or `[5-9]` at the start of `s`, with its length in bytes.
fn counter(s: &str) -> Option<(Token, usize)> {
    let end = s.find(']')?;
    let inner = &s[1..end];
    let (first, last) = inner.split_once('-').unwrap_or((inner, ""));
    let digits = |d: &str| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit());
    if !digits(first) || !(last.is_empty() || digits(last)) {
        return None;
    }
    let start = first.parse().ok()?;
    Some((
        Token::Counter {
            start,
            width: first.len(),
        },
        end + 1,
    ))
}

/// `archive.tar.gz` → (`archive.tar`, `gz`); a leading dot isn't an extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, ext),
        _ => (name, ""),
    }
}

fn render(tokens: &[Token], index: usize, old: &str) -> String {
    let (stem, ext) = split_ext(old);
    let mut out = String::new();
    for t in tokens {
        match t {
            Token::Lit(s) => out.push_str(s),
            Token::Counter { start, width } => {
                let n = start + index as u64;
                out.push_str(&format!("{n:0width$}"));
            }
            Token::Name => out.push_str(stem),
            Token::Ext => out.push_str(ext),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Empty, `.`, `..` or containing `/`.
    Invalid,
    /// Two rows would get the same name.
    Duplicate,
    /// Another file in the folder, one that isn't being renamed, has it.
    Clash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub old: String,
    pub new: String,
    pub problem: Option<Problem>,
}

/// What a bulk rename of `olds` (in selection order) with `template` would
/// do. `existing` is every name in the folder.
pub fn plan_bulk(template: &str, olds: &[String], existing: &[String]) -> Vec<Row> {
    let tokens = parse(template);
    let news: Vec<String> = olds
        .iter()
        .enumerate()
        .map(|(i, old)| render(&tokens, i, old))
        .collect();
    check(olds, news, existing)
}

/// A single rename to exactly `new`, no template tokens.
pub fn plan_single(old: &str, new: &str, existing: &[String]) -> Row {
    check(&[old.to_string()], vec![new.to_string()], existing).remove(0)
}

fn check(olds: &[String], news: Vec<String>, existing: &[String]) -> Vec<Row> {
    let rows: Vec<Row> = olds
        .iter()
        .zip(&news)
        .map(|(old, new)| {
            let problem = if new.is_empty() || new == "." || new == ".." || new.contains('/') {
                Some(Problem::Invalid)
            } else if news.iter().filter(|n| *n == new).count() > 1 {
                Some(Problem::Duplicate)
            } else if existing.contains(new) && !olds.contains(new) {
                Some(Problem::Clash)
            } else {
                None
            };
            Row {
                old: old.clone(),
                new: new.clone(),
                problem,
            }
        })
        .collect();
    rows
}

/// Renames in two phases, every file to a temporary name and then to its
/// final one, so swaps like `a→b, b→a` work. On failure, everything that
/// was moved is put back where possible. Returns how many were renamed.
pub fn apply(vfs: &dyn Vfs, dir: &Path, rows: &[Row]) -> Result<usize, String> {
    if rows.iter().any(|r| r.problem.is_some()) {
        return Err("fix the names marked ✗ first: empty, clashing or containing /".into());
    }
    let moves: Vec<&Row> = rows.iter().filter(|r| r.old != r.new).collect();
    let tmp = |i: usize| dir.join(format!(".tosk-rename-{}-{i}", std::process::id()));

    for (i, r) in moves.iter().enumerate() {
        if let Err(e) = vfs.rename(&dir.join(&r.old), &tmp(i)) {
            for (j, done) in moves[..i].iter().enumerate() {
                let _ = vfs.rename(&tmp(j), &dir.join(&done.old));
            }
            return Err(format!("can't rename {}: {e}", r.old));
        }
    }
    for (i, r) in moves.iter().enumerate() {
        let target = dir.join(&r.new);
        let result = if vfs.exists(&target) {
            Err(format!("{} appeared in the meantime", r.new))
        } else {
            vfs.rename(&tmp(i), &target).map_err(|e| e.to_string())
        };
        if let Err(e) = result {
            // The first `i` are done; put the rest back under their old names.
            for (j, rest) in moves.iter().enumerate().skip(i) {
                let _ = vfs.rename(&tmp(j), &dir.join(&rest.old));
            }
            return Err(format!(
                "renamed {i} of {}, then failed on {}: {e}",
                moves.len(),
                r.old
            ));
        }
    }
    Ok(moves.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::local::Local;
    use std::fs;
    use tempfile::TempDir;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn news(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.new.as_str()).collect()
    }

    /// A folder holding `names` as files whose content is their own name.
    fn folder(names: &[&str]) -> TempDir {
        let tmp = TempDir::new().unwrap();
        for n in names {
            fs::write(tmp.path().join(n), n).unwrap();
        }
        tmp
    }

    fn listing(dir: &Path) -> Vec<(String, String)> {
        let mut v: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let name = e.file_name().into_string().unwrap();
                (name, fs::read_to_string(e.path()).unwrap())
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn templates() {
        let olds = strings(&["a.mkv", "b.mkv", "c.mkv"]);
        let p = |t| news(&plan_bulk(t, &olds, &olds)).join(" ");
        assert_eq!(p("x[01]"), "x01 x02 x03");
        assert_eq!(p("x[001]"), "x001 x002 x003");
        assert_eq!(p("x[0-9]"), "x0 x1 x2");
        assert_eq!(p("x[8-99]"), "x8 x9 x10");
        assert_eq!(p("{name}-[1].{ext}"), "a-1.mkv b-2.mkv c-3.mkv");
        // Not tokens: kept literally.
        assert_eq!(
            p("[x]{nam}[1-a]"),
            "[x]{nam}[1-a] [x]{nam}[1-a] [x]{nam}[1-a]"
        );
    }

    #[test]
    fn extensions() {
        assert_eq!(split_ext("archive.tar.gz"), ("archive.tar", "gz"));
        assert_eq!(split_ext(".bashrc"), (".bashrc", ""));
        assert_eq!(split_ext("Makefile"), ("Makefile", ""));
        assert_eq!(split_ext(".config.toml"), (".config", "toml"));
    }

    #[test]
    fn validation() {
        let olds = strings(&["a", "b"]);
        let existing = strings(&["a", "b", "taken"]);
        let problems = |t| -> Vec<Option<Problem>> {
            plan_bulk(t, &olds, &existing)
                .into_iter()
                .map(|r| r.problem)
                .collect()
        };
        assert_eq!(problems(""), [Some(Problem::Invalid); 2]);
        assert_eq!(problems("d/[1]"), [Some(Problem::Invalid); 2]);
        assert_eq!(problems("same"), [Some(Problem::Duplicate); 2]);
        // b is being renamed too, so landing on it is fine.
        assert_eq!(problems("b[1]"), [None, None]);
        let row = plan_single("a", "taken", &existing);
        assert_eq!(row.problem, Some(Problem::Clash));
        assert_eq!(
            plan_single("a", "b", &existing).problem,
            Some(Problem::Clash)
        );
        assert_eq!(plan_single("a", "a", &existing).problem, None);
        assert_eq!(
            plan_single("a", "..", &existing).problem,
            Some(Problem::Invalid)
        );
    }

    #[test]
    fn five_episodes_in_selection_order() {
        // Selected out of sort order, numbered in selection order.
        let picked = [
            "Pilot.mkv",
            "ep2 final.mkv",
            "Third.mkv",
            "4.mkv",
            "last.mkv",
        ];
        let tmp = folder(&picked);
        let olds = strings(&picked);
        let rows = plan_bulk("show.s01.e[01].mkv", &olds, &olds);
        assert_eq!(apply(&Local, tmp.path(), &rows), Ok(5));
        let expected: Vec<(String, String)> = (1..=5)
            .map(|i| (format!("show.s01.e{i:02}.mkv"), picked[i - 1].to_string()))
            .collect();
        assert_eq!(listing(tmp.path()), expected);
    }

    #[test]
    fn swaps_and_rotations() {
        let tmp = folder(&["a", "b", "c"]);
        let rows = vec![
            plan_single("a", "b", &[]),
            plan_single("b", "c", &[]),
            plan_single("c", "a", &[]),
        ];
        assert_eq!(apply(&Local, tmp.path(), &rows), Ok(3));
        assert_eq!(
            listing(tmp.path()),
            [
                ("a".into(), "c".into()),
                ("b".into(), "a".into()),
                ("c".into(), "b".into())
            ]
        );
    }

    #[test]
    fn clashes_refuse_and_touch_nothing() {
        let tmp = folder(&["a", "b", "other"]);
        let olds = strings(&["a", "b"]);
        let existing = strings(&["a", "b", "other"]);
        let rows = plan_bulk("other", &olds, &existing);
        assert!(
            apply(&Local, tmp.path(), &rows)
                .unwrap_err()
                .starts_with("fix the names")
        );
        let rows = plan_bulk("{name}", &strings(&["a"]), &existing);
        assert_eq!(apply(&Local, tmp.path(), &rows), Ok(0));
        assert_eq!(listing(tmp.path()).len(), 3);
    }

    #[test]
    fn a_late_clash_rolls_back() {
        let tmp = folder(&["a", "b"]);
        let olds = strings(&["a", "b"]);
        let rows = plan_bulk("n[1]", &olds, &olds);
        // n2 shows up after planning.
        fs::write(tmp.path().join("n2"), "intruder").unwrap();
        let err = apply(&Local, tmp.path(), &rows).unwrap_err();
        assert_eq!(
            err,
            "renamed 1 of 2, then failed on b: n2 appeared in the meantime"
        );
        assert_eq!(
            listing(tmp.path()),
            [
                ("b".into(), "b".into()),
                ("n1".into(), "a".into()),
                ("n2".into(), "intruder".into())
            ]
        );
    }

    #[test]
    fn folders_rename_too() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("old")).unwrap();
        fs::write(tmp.path().join("old/f"), "x").unwrap();
        let rows = [plan_single("old", "new dir", &[])];
        assert_eq!(apply(&Local, tmp.path(), &rows), Ok(1));
        assert!(tmp.path().join("new dir/f").exists());
    }
}
