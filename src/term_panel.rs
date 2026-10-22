//! The terminal panel: `$SHELL` in a pty, parsed by vt100 and drawn by
//! tui-term. The shell lives on while the panel is hidden.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tui_term::vt100;

/// Lines kept above the visible screen.
const SCROLLBACK: usize = 1000;

pub struct TermPanel {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// A ping per chunk of output from the reader thread.
    output: Receiver<()>,
    size: (u16, u16),
    /// Where the shell started.
    pub cwd: PathBuf,
}

impl TermPanel {
    /// Starts `shell` in `cwd` at `rows` × `cols`.
    pub fn spawn(shell: &OsStr, cwd: &Path, rows: u16, cols: u16) -> Result<TermPanel> {
        let (rows, cols) = (rows.max(1), cols.max(1));
        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new(shell);
        cmd.cwd(cwd);
        // The panel is a plain xterm-alike, not kitty: programs mustn't send
        // kitty graphics or keyboard sequences into it.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env_remove("KITTY_WINDOW_ID");
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, output) = mpsc::channel();
        let feed = parser.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut p) = feed.lock() {
                            p.process(&buf[..n]);
                        }
                        if tx.send(()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(TermPanel {
            parser,
            writer,
            master: pair.master,
            child,
            output,
            size: (rows, cols),
            cwd: cwd.to_path_buf(),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Sends a key, encoded for the shell.
    pub fn key(&mut self, ev: KeyEvent) {
        let app_cursor = self.with_screen(|s| s.application_cursor());
        if let Some(bytes) = encode(ev, app_cursor) {
            self.write(&bytes);
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        if (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Ok(mut p) = self.parser.lock() {
            p.screen_mut().set_size(rows, cols);
        }
    }

    /// Takes the reader's pings; returns whether there was new output.
    pub fn drain(&mut self) -> bool {
        self.output.try_iter().count() > 0
    }

    /// Whether the shell has exited.
    pub fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    pub fn with_screen<R>(&self, f: impl FnOnce(&vt100::Screen) -> R) -> R {
        let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        f(p.screen())
    }
}

impl Drop for TermPanel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The bytes a terminal sends for `ev`. `app_cursor` is the shell's
/// application cursor mode (arrows as `ESC O A` instead of `ESC [ A`).
pub fn encode(ev: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let m = ev.modifiers;
    let (ctrl, alt, shift) = (
        m.contains(KeyModifiers::CONTROL),
        m.contains(KeyModifiers::ALT),
        m.contains(KeyModifiers::SHIFT),
    );
    // xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
    let modifier = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let cursor = |letter: char| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[{n};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let bytes = match ev.code {
        KeyCode::Char(c) if ctrl => {
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 & 0x1f,
                ' ' | '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '7' | '/' => 0x1f,
                '?' | '8' => 0x7f,
                _ => return Some(with_alt(alt, c.to_string().into_bytes())),
            };
            with_alt(alt, vec![b])
        }
        KeyCode::Char(c) => with_alt(alt, c.to_string().into_bytes()),
        KeyCode::Enter => with_alt(alt, b"\r".to_vec()),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace if ctrl => with_alt(alt, vec![0x08]),
        KeyCode::Backspace => with_alt(alt, vec![0x7f]),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        KeyCode::F(n @ 5..=12) => {
            let codes = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde(codes[(n - 5) as usize])
        }
        _ => return None,
    };
    Some(bytes)
}

fn with_alt(alt: bool, mut bytes: Vec<u8>) -> Vec<u8> {
    if alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn enc(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
        encode(KeyEvent::new(code, mods), false).unwrap()
    }

    #[test]
    fn encodes_keys() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(enc(KeyCode::Char('a'), none), b"a");
        assert_eq!(enc(KeyCode::Char('é'), none), "é".as_bytes());
        assert_eq!(enc(KeyCode::Char('c'), ctrl), [3]);
        assert_eq!(enc(KeyCode::Char('C'), ctrl | KeyModifiers::SHIFT), [3]);
        assert_eq!(enc(KeyCode::Char('['), ctrl), [0x1b]);
        assert_eq!(enc(KeyCode::Char('b'), KeyModifiers::ALT), b"\x1bb");
        assert_eq!(enc(KeyCode::Enter, none), b"\r");
        assert_eq!(enc(KeyCode::Tab, none), b"\t");
        assert_eq!(enc(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z");
        assert_eq!(enc(KeyCode::Backspace, none), [0x7f]);
        assert_eq!(enc(KeyCode::Esc, none), [0x1b]);
        assert_eq!(enc(KeyCode::Up, none), b"\x1b[A");
        assert_eq!(enc(KeyCode::Left, ctrl), b"\x1b[1;5D");
        assert_eq!(enc(KeyCode::Delete, none), b"\x1b[3~");
        assert_eq!(enc(KeyCode::F(1), none), b"\x1bOP");
        assert_eq!(enc(KeyCode::F(5), none), b"\x1b[15~");
        let app = encode(KeyEvent::new(KeyCode::Up, none), true).unwrap();
        assert_eq!(app, b"\x1bOA");
    }

    fn wait_for(t: &mut TermPanel, needle: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            t.drain();
            if t.with_screen(|s| s.contents().contains(needle)) {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn runs_a_shell_in_the_folder() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        // A predictable shell rather than the user's.
        let mut t = TermPanel::spawn("/bin/sh".as_ref(), &dir, 10, 60).unwrap();
        t.write(b"pwd; echo $TERM; echo done-$((1+1))\r");
        assert!(
            wait_for(&mut t, "done-2"),
            "{}",
            t.with_screen(|s| s.contents())
        );
        let screen = t.with_screen(|s| s.contents());
        assert!(screen.contains(&dir.display().to_string()));
        assert!(screen.contains("xterm-256color"));

        t.resize(5, 30);
        t.write(b"stty size\r");
        assert!(wait_for(&mut t, "5 30"));

        t.write(b"exit\r");
        let start = Instant::now();
        while !t.exited() && start.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(t.exited());
    }
}
