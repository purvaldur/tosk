//! Starting desktop programs without tying them to tosk.

use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

/// `xdg-open <path>`, detached: no stdio, its own process group so Ctrl-c
/// in tosk's terminal never reaches it, and reaped on a thread.
pub fn xdg_open(path: &Path) -> io::Result<()> {
    let mut child = Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    thread::spawn(move || child.wait());
    Ok(())
}
