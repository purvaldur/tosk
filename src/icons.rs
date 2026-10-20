//! Nerd Font icons (v3 code points).

use crate::entry::{Entry, Kind};
use crate::preview::{FileKind, kind_by_extension};

pub const HOME: char = '\u{f015}';
pub const DESKTOP: char = '\u{f108}';
pub const DOCUMENTS: char = '\u{f0219}';
pub const DOWNLOADS: char = '\u{f024d}';
pub const MUSIC: char = '\u{f001}';
pub const PICTURES: char = '\u{f03e}';
pub const VIDEOS: char = '\u{f008}';
pub const PINNED: char = '\u{f02e}';

const FOLDER: char = '\u{f07b}';
const FOLDER_LINK: char = '\u{f482}';
const BROKEN_LINK: char = '\u{f127}';
const FILE: char = '\u{f15b}';
const TEXT: char = '\u{f15c}';

pub fn for_entry(e: &Entry) -> char {
    match e.kind {
        Kind::Missing => return BROKEN_LINK,
        Kind::Dir if e.is_link() => return FOLDER_LINK,
        Kind::Dir => return FOLDER,
        Kind::File | Kind::Other => {}
    }
    let ext = e
        .name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    let by_name = match ext.as_deref() {
        Some("rs") => Some('\u{e7a8}'),
        Some("md" | "markdown") => Some('\u{f48a}'),
        Some("toml" | "ini" | "conf" | "cfg" | "yaml" | "yml") => Some('\u{e615}'),
        Some("json") => Some('\u{e60b}'),
        Some("sh" | "bash" | "zsh" | "fish") => Some('\u{f489}'),
        Some("py") => Some('\u{e606}'),
        Some("js" | "mjs" | "cjs") => Some('\u{e74e}'),
        Some("ts" | "tsx") => Some('\u{e628}'),
        Some("html" | "htm") => Some('\u{e736}'),
        Some("css" | "scss") => Some('\u{e749}'),
        Some("c" | "h") => Some('\u{e61e}'),
        Some("cpp" | "cc" | "hpp") => Some('\u{e61d}'),
        Some("go") => Some('\u{e627}'),
        Some("lua") => Some('\u{e620}'),
        Some("nix") => Some('\u{f313}'),
        Some("lock") => Some('\u{f023}'),
        Some("txt" | "log") => Some(TEXT),
        _ => None,
    };
    by_name.unwrap_or_else(|| match kind_by_extension(&e.name) {
        Some(FileKind::Image) => '\u{f1c5}',
        Some(FileKind::Video) => '\u{f1c8}',
        Some(FileKind::Audio) => '\u{f1c7}',
        Some(FileKind::Archive) => '\u{f1c6}',
        Some(FileKind::Pdf) => '\u{f1c1}',
        _ => FILE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_by_kind_and_extension() {
        let e = |name: &str, kind| Entry::new(name.into(), kind);
        assert_eq!(for_entry(&e("src", Kind::Dir)), FOLDER);
        let mut link = e("dots", Kind::Dir);
        link.link_target = Some("x".into());
        assert_eq!(for_entry(&link), FOLDER_LINK);
        assert_eq!(for_entry(&e("gone", Kind::Missing)), BROKEN_LINK);
        assert_eq!(for_entry(&e("main.rs", Kind::File)), '\u{e7a8}');
        assert_eq!(for_entry(&e("Photo.JPG", Kind::File)), '\u{f1c5}');
        assert_eq!(for_entry(&e("Makefile", Kind::File)), FILE);
    }
}
