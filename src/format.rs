use std::path::Path;
use std::time::SystemTime;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::entry::Entry;

/// Compact SI size for the file list: `356B`, `4.1M`, `12M`, `1.2G`.
pub fn compact_size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes}B");
    }
    let units = ['K', 'M', 'G'];
    let mut value = bytes as f64;
    let mut i = 0;
    value /= 1000.0;
    while value >= 1000.0 && i < units.len() - 1 {
        value /= 1000.0;
        i += 1;
    }
    if value < 10.0 {
        format!("{value:.1}{}", units[i])
    } else {
        format!("{}{}", value.round() as u64, units[i])
    }
}

/// `Oct 19` for this year, `Oct 19 2024` otherwise.
pub fn list_date(t: SystemTime, tz: &TimeZone, this_year: i16) -> String {
    let Ok(ts) = Timestamp::try_from(t) else {
        return String::new();
    };
    let z = ts.to_zoned(tz.clone());
    if z.year() == this_year {
        z.strftime("%b %d").to_string()
    } else {
        z.strftime("%b %d %Y").to_string()
    }
}

/// Nautilus-style sizes for summaries and properties: `1 byte`,
/// `840 bytes`, `1.2 kB`, `4.1 MB`.
pub fn human_size(bytes: u64) -> String {
    if bytes < 1000 {
        return count(bytes as usize, "byte", "bytes");
    }
    let units = ["kB", "MB", "GB", "TB"];
    let mut value = bytes as f64 / 1000.0;
    let mut i = 0;
    while value >= 1000.0 && i < units.len() - 1 {
        value /= 1000.0;
        i += 1;
    }
    format!("{value:.1} {}", units[i])
}

/// The selection summary, in Nautilus's words. Folder sizes are left out;
/// "containing" counts direct children and is dropped until every folder
/// has been counted.
pub fn selection_summary(items: &[&Entry]) -> String {
    let (dirs, files): (Vec<&Entry>, Vec<&Entry>) = items.iter().partition(|e| e.is_dir());
    let containing = || {
        dirs.iter()
            .map(|d| d.children)
            .sum::<Option<usize>>()
            .map(|n| format!(" (containing {})", count(n, "item", "items")))
            .unwrap_or_default()
    };
    let size = human_size(files.iter().map(|f| f.size).sum());
    match (dirs.len(), files.len()) {
        (0, 0) => String::new(),
        (1, 0) => format!("“{}” selected{}", dirs[0].name, containing()),
        (0, 1) => format!("“{}” selected ({size})", files[0].name),
        (d, 0) => format!("{} selected{}", count(d, "folder", "folders"), containing()),
        (0, f) => format!("{} selected ({size})", count(f, "item", "items")),
        (d, f) => format!(
            "{} selected{}, {} selected ({size})",
            count(d, "folder", "folders"),
            containing(),
            count(f, "other item", "other items"),
        ),
    }
}

/// `1 item`, `2 items`.
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Show a path with `~` for the home folder.
pub fn tilde(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Kind;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn compact_sizes() {
        assert_eq!(compact_size(0), "0B");
        assert_eq!(compact_size(356), "356B");
        assert_eq!(compact_size(999), "999B");
        assert_eq!(compact_size(1000), "1.0K");
        assert_eq!(compact_size(1460), "1.5K");
        assert_eq!(compact_size(4_100_000), "4.1M");
        assert_eq!(compact_size(12_500_000), "13M");
        assert_eq!(compact_size(999_000), "999K");
        assert_eq!(compact_size(1_200_000_000), "1.2G");
        assert_eq!(compact_size(5_000_000_000_000), "5000G");
    }

    #[test]
    fn dates() {
        // 2026-10-19 12:00 UTC
        let t = UNIX_EPOCH + Duration::from_secs(1_792_411_200);
        assert_eq!(list_date(t, &TimeZone::UTC, 2026), "Oct 19");
        assert_eq!(list_date(t, &TimeZone::UTC, 2027), "Oct 19 2026");
        let t = UNIX_EPOCH + Duration::from_secs(1_791_072_000); // 2026-10-04
        assert_eq!(list_date(t, &TimeZone::UTC, 2026), "Oct 04");
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(1), "1 byte");
        assert_eq!(human_size(840), "840 bytes");
        assert_eq!(human_size(1200), "1.2 kB");
        assert_eq!(human_size(4_100_000), "4.1 MB");
        assert_eq!(human_size(2_500_000_000_000_000), "2500.0 TB");
    }

    fn file(name: &str, size: u64) -> Entry {
        let mut e = Entry::new(name.into(), Kind::File);
        e.size = size;
        e
    }

    fn folder(name: &str, children: Option<usize>) -> Entry {
        let mut e = Entry::new(name.into(), Kind::Dir);
        e.children = children;
        e
    }

    #[test]
    fn selection_summaries() {
        let notes = file("NOTES.md", 840);
        let cargo = file("Cargo.toml", 360);
        let src = folder("src", Some(3));
        let docs = folder("docs", Some(5));
        let one = folder("one", Some(1));
        let s = |items: &[&Entry]| selection_summary(items);

        assert_eq!(s(&[]), "");
        assert_eq!(s(&[&notes]), "“NOTES.md” selected (840 bytes)");
        assert_eq!(s(&[&src]), "“src” selected (containing 3 items)");
        assert_eq!(s(&[&one]), "“one” selected (containing 1 item)");
        assert_eq!(s(&[&notes, &cargo]), "2 items selected (1.2 kB)");
        assert_eq!(s(&[&src, &docs]), "2 folders selected (containing 8 items)");
        assert_eq!(
            s(&[&src, &notes, &cargo]),
            "1 folder selected (containing 3 items), 2 other items selected (1.2 kB)"
        );
        assert_eq!(
            s(&[&src, &docs, &notes]),
            "2 folders selected (containing 8 items), 1 other item selected (840 bytes)"
        );
        assert_eq!(s(&[&file("empty", 0)]), "“empty” selected (0 bytes)");
        assert_eq!(s(&[&file("a", 1)]), "“a” selected (1 byte)");
    }

    #[test]
    fn summary_waits_for_counts_and_treats_links_by_target() {
        let counting = folder("big", None);
        let src = folder("src", Some(3));
        assert_eq!(selection_summary(&[&counting]), "“big” selected");
        assert_eq!(selection_summary(&[&counting, &src]), "2 folders selected");
        let mut link = folder("dots", Some(2));
        link.link_target = Some("dotfiles".into());
        assert_eq!(
            selection_summary(&[&link]),
            "“dots” selected (containing 2 items)"
        );
    }

    #[test]
    fn tilde_paths() {
        let home = Path::new("/home/u");
        assert_eq!(tilde(Path::new("/home/u"), Some(home)), "~");
        assert_eq!(
            tilde(Path::new("/home/u/code/tosk"), Some(home)),
            "~/code/tosk"
        );
        assert_eq!(tilde(Path::new("/home/user2"), Some(home)), "/home/user2");
        assert_eq!(tilde(Path::new("/etc"), None), "/etc");
    }
}
