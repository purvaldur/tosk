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

/// Size with the exact count: `4.1 MB (4,100,000 bytes)`, or just
/// `840 bytes` under 1 kB.
pub fn exact_size(bytes: u64) -> String {
    if bytes < 1000 {
        return human_size(bytes);
    }
    format!("{} ({} bytes)", human_size(bytes), thousands(bytes))
}

/// `4,100,000`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `Monday 19 October 2026, 14:03` in the local time zone.
pub fn full_date(t: SystemTime, tz: &TimeZone) -> String {
    match Timestamp::try_from(t) {
        Ok(ts) => ts
            .to_zoned(tz.clone())
            .strftime("%A %-d %B %Y, %H:%M")
            .to_string(),
        Err(_) => String::new(),
    }
}

/// `drwxr-xr-x` from a raw `st_mode`, setuid, setgid and sticky included.
pub fn mode_string(mode: u32) -> String {
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o020000 => 'c',
        0o060000 => 'b',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '-',
    };
    let mut s = String::from(kind);
    // (read, write, execute bit, special bit, special char)
    for (shift, special, mark) in [(6, 0o4000, 's'), (3, 0o2000, 's'), (0, 0o1000, 't')] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        let x = bits & 1 != 0;
        s.push(match (mode & special != 0, x) {
            (true, true) => mark,
            (true, false) => mark.to_ascii_uppercase(),
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    s
}

/// The name for `id` in an `/etc/passwd` or `/etc/group` style file, where
/// the id is the third field.
pub fn name_for_id(table: &str, id: u32) -> Option<String> {
    table.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let found: u32 = fields.nth(1)?.parse().ok()?;
        (found == id).then(|| name.to_string())
    })
}

/// `user:group`, with numbers for ids that aren't in the local tables.
pub fn owner(uid: u32, gid: u32) -> String {
    let lookup = |file: &str, id: u32| {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|t| name_for_id(&t, id))
            .unwrap_or_else(|| id.to_string())
    };
    format!(
        "{}:{}",
        lookup("/etc/passwd", uid),
        lookup("/etc/group", gid)
    )
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

    #[test]
    fn exact_sizes_and_dates() {
        assert_eq!(exact_size(840), "840 bytes");
        assert_eq!(exact_size(4_100_000), "4.1 MB (4,100,000 bytes)");
        assert_eq!(exact_size(4_356), "4.4 kB (4,356 bytes)");
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567_890), "1,234,567,890");
        // 2026-10-19 12:00 UTC
        let t = UNIX_EPOCH + Duration::from_secs(1_792_411_200);
        assert_eq!(
            full_date(t, &TimeZone::UTC),
            "Monday 19 October 2026, 12:00"
        );
    }

    #[test]
    fn modes_and_owners() {
        assert_eq!(mode_string(0o100644), "-rw-r--r--");
        assert_eq!(mode_string(0o040755), "drwxr-xr-x");
        assert_eq!(mode_string(0o120777), "lrwxrwxrwx");
        assert_eq!(mode_string(0o104755), "-rwsr-xr-x");
        assert_eq!(mode_string(0o041777), "drwxrwxrwt");
        assert_eq!(mode_string(0o102640), "-rw-r-S---");
        let passwd = "root:x:0:0::/root:/bin/bash\nthor:x:1000:1000::/home/thor:/bin/zsh\n";
        assert_eq!(name_for_id(passwd, 1000).as_deref(), Some("thor"));
        assert_eq!(name_for_id(passwd, 0).as_deref(), Some("root"));
        assert_eq!(name_for_id(passwd, 5), None);
        assert_eq!(name_for_id("broken line\n", 0), None);
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
