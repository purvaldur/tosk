//! `wl-copy` wrappers.

use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// Put `text` on the Wayland clipboard, optionally with a MIME type.
pub fn copy(mime: Option<&str>, text: &str) -> io::Result<()> {
    let mut cmd = Command::new("wl-copy");
    if let Some(mime) = mime {
        cmd.args(["-t", mime]);
    }
    // wl-copy forks a child that serves the clipboard and keeps inherited
    // pipes open, so only stdin is piped; the parent's exit status is enough.
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("wl-copy failed ({status})")))
    }
}

/// `text/uri-list`: one percent-encoded `file://` URI per line, which GUI
/// apps paste as the files themselves.
pub fn uri_list(paths: &[&Path]) -> String {
    paths
        .iter()
        .map(|p| format!("{}\r\n", file_uri(p)))
        .collect()
}

pub fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// Paths as shell words, space-separated, quoted only when needed.
pub fn shell_words(paths: &[&Path]) -> String {
    paths
        .iter()
        .map(|p| shell_quote(&p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn shell_quote(s: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "/._-+=:,@%~".contains(c);
    if !s.is_empty() && s.chars().all(plain) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn uris_are_percent_encoded() {
        assert_eq!(
            file_uri(Path::new("/home/u/My Files/naïve #1.txt")),
            "file:///home/u/My%20Files/na%C3%AFve%20%231.txt"
        );
        let raw = Path::new(OsStr::from_bytes(b"/tmp/\xff"));
        assert_eq!(file_uri(raw), "file:///tmp/%FF");
        assert_eq!(
            uri_list(&[Path::new("/a"), Path::new("/b c")]),
            "file:///a\r\nfile:///b%20c\r\n"
        );
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(shell_quote("/home/u/notes.md"), "/home/u/notes.md");
        assert_eq!(shell_quote("/home/u/My Files"), "'/home/u/My Files'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(
            shell_words(&[Path::new("/a"), Path::new("/b c")]),
            "/a '/b c'"
        );
    }
}
