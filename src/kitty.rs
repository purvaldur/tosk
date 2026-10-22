//! Opening files in the user's Helix through kitty remote control.
//!
//! The editor window is the one with the user variable `role=helix`. tosk
//! may run in a different kitty instance than Helix (a floating window has
//! its own process), so every socket next to `$KITTY_LISTEN_ON` is tried.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Kitty {
    /// The `kitty` program; a stand-in in tests.
    pub program: PathBuf,
    /// Sockets to try, tosk's own first, as `unix:/path`.
    pub sockets: Vec<String>,
    /// The editor window's `role` user variable.
    pub role: String,
    /// `hx` or `helix`, whichever is installed.
    pub helix: PathBuf,
}

pub enum Opened {
    /// Sent to the running Helix and focused it.
    Existing,
    /// No Helix window: started one in a new tab.
    NewTab,
}

impl Kitty {
    /// From the environment. Fails with advice when remote control isn't set
    /// up or Helix isn't installed.
    pub fn from_env(role: &str) -> Result<Kitty, String> {
        let listen_on = std::env::var("KITTY_LISTEN_ON").map_err(|_| {
            "kitty remote control is off: add `listen_on unix:/tmp/kitty` to kitty.conf".to_string()
        })?;
        let helix = find_in_path(&["hx", "helix"]).ok_or("Helix (hx or helix) isn't installed")?;
        let program = find_in_path(&["kitty"]).ok_or("kitty isn't on PATH")?;
        Ok(Kitty {
            program,
            sockets: sockets(&listen_on),
            role: role.to_string(),
            helix,
        })
    }

    fn command(&self, socket: &str, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.arg("@").arg("--to").arg(socket).args(args);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        cmd
    }

    fn matcher(&self) -> String {
        format!("var:role={}", self.role)
    }

    /// Opens `paths` in the running Helix and focuses it, or else starts
    /// Helix on them in a new tab with `cwd` as its folder. With `line`,
    /// the (single) file opens at that line.
    pub fn open_files(
        &self,
        paths: &[PathBuf],
        line: Option<u64>,
        cwd: &Path,
    ) -> Result<Opened, String> {
        let args: Vec<String> = paths.iter().map(|p| helix_arg(p)).collect();
        let mut text = format!(":open {}\r", args.join(" "));
        if let Some(n) = line {
            text.push_str(&format!(":goto {n}\r"));
        }
        let matcher = self.matcher();
        for socket in &self.sockets {
            // send-key and send-text succeed even when nothing matches, so
            // ask `ls` (which fails then) whether Helix is behind this socket.
            let found = self
                .command(socket, &["ls", "--match", &matcher])
                .status()
                .is_ok_and(|s| s.success());
            if !found {
                continue;
            }
            // Esc first, in case Helix is in insert mode or has a picker
            // open. It goes as a key press: a raw `\x1b` sent with the text
            // reaches Helix glued to the `:` and reads as Alt-:.
            let _ = self
                .command(socket, &["send-key", "--match", &matcher, "escape"])
                .status();
            let mut cmd = self.command(socket, &["send-text", "--match", &matcher, "--stdin"]);
            cmd.stdin(Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| format!("can't run kitty: {e}"))?;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            let _ = self
                .command(socket, &["focus-window", "--match", &matcher])
                .status();
            return Ok(Opened::Existing);
        }
        let mut argv: Vec<OsString> = vec![self.helix.clone().into()];
        argv.extend(paths.iter().map(|p| match line {
            // Helix takes `file:line` on its command line.
            Some(n) => format!("{}:{n}", p.display()).into(),
            None => p.clone().into(),
        }));
        self.launch(Some(cwd), &argv).map(|()| Opened::NewTab)
    }

    /// `hx .` in `dir`, in a new tab.
    pub fn edit_folder(&self, dir: &Path) -> Result<(), String> {
        self.launch(Some(dir), &[self.helix.clone().into(), ".".into()])
    }

    /// `kitten ssh <host> <script>` in a new tab: Helix on the server. The
    /// tab isn't marked as the editor, so local files never go to it.
    pub fn launch_ssh(&self, host_args: &[String], script: &str) -> Result<(), String> {
        let mut argv: Vec<OsString> = vec!["kitten".into(), "ssh".into()];
        argv.extend(host_args.iter().map(OsString::from));
        argv.push(script.into());
        self.launch(None, &argv)
    }

    /// A new tab running `argv`; with `cwd`, it's the editor tab there.
    fn launch(&self, cwd: Option<&Path>, argv: &[OsString]) -> Result<(), String> {
        let socket = self.sockets.first().ok_or("no kitty socket")?;
        let mut cmd = self.command(socket, &["launch", "--type=tab"]);
        if let Some(cwd) = cwd {
            cmd.args(["--var", &format!("role={}", self.role), "--cwd"])
                .arg(cwd);
        }
        cmd.args(argv);
        let out = cmd.output().map_err(|e| format!("can't run kitty: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("kitty @ launch failed: {}", err.trim()))
        }
    }
}

/// `unix:/tmp/kitty-123` and every other `kitty-*` socket in `/tmp`, so a
/// Helix in another kitty instance is found too. Abstract sockets
/// (`unix:@name`) can't be listed, so those are used alone.
pub fn sockets(listen_on: &str) -> Vec<String> {
    let mut out = vec![listen_on.to_string()];
    let Some(path) = listen_on
        .strip_prefix("unix:")
        .filter(|p| !p.starts_with('@'))
    else {
        return out;
    };
    let path = Path::new(path);
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return out;
    };
    // kitty appends `-<pid>` to the configured name.
    let name = name.to_string_lossy();
    let stem = name.rsplit_once('-').map_or(name.as_ref(), |(s, _)| s);
    let prefix = format!("{stem}-");
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut others: Vec<String> = read
        .flatten()
        .filter(|e| {
            use std::os::unix::fs::FileTypeExt;
            e.file_type().is_ok_and(|t| t.is_socket())
                && e.file_name().to_string_lossy().starts_with(&prefix)
        })
        .map(|e| format!("unix:{}", e.path().display()))
        .filter(|s| *s != listen_on)
        .collect();
    others.sort();
    out.extend(others);
    out
}

/// A path for Helix's command line: bare when plain, otherwise in single
/// quotes (taken literally), or in double quotes if it has a `'` itself.
pub fn helix_arg(path: &Path) -> String {
    let s = path.to_string_lossy();
    let plain = |c: char| c.is_alphanumeric() || "/._-+=,@~:".contains(c);
    if s.chars().all(plain) {
        s.into_owned()
    } else if !s.contains('\'') {
        format!("'{s}'")
    } else {
        let escaped = s
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%");
        format!("\"{escaped}\"")
    }
}

fn find_in_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    names.iter().find_map(|name| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|p| p.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use tempfile::TempDir;

    #[test]
    fn quotes_for_helix() {
        let q = |s: &str| helix_arg(Path::new(s));
        assert_eq!(q("/home/u/notes.md"), "/home/u/notes.md");
        assert_eq!(q("/home/u/My Notes.md"), "'/home/u/My Notes.md'");
        assert_eq!(q("/tmp/50%{x}.md"), "'/tmp/50%{x}.md'");
        assert_eq!(q("/tmp/it's here"), "\"/tmp/it's here\"");
    }

    #[test]
    fn finds_sibling_sockets() {
        let tmp = TempDir::new().unwrap();
        let d = tmp.path();
        let _own = UnixListener::bind(d.join("kitty-100")).unwrap();
        let _other = UnixListener::bind(d.join("kitty-200")).unwrap();
        let _unrelated = UnixListener::bind(d.join("other-300")).unwrap();
        fs::write(d.join("kitty-400"), "not a socket").unwrap();
        let own = format!("unix:{}", d.join("kitty-100").display());
        assert_eq!(
            sockets(&own),
            [
                own.clone(),
                format!("unix:{}", d.join("kitty-200").display())
            ]
        );
        assert_eq!(sockets("unix:@kitty"), ["unix:@kitty"]);
    }

    /// A stand-in `kitty` that logs its arguments and stdin, and fails
    /// `ls` unless the socket is `unix:helix-here`. Like the real one,
    /// send-text succeeds either way.
    fn fake_kitty(dir: &Path) -> (PathBuf, PathBuf) {
        let log = dir.join("log");
        let script = dir.join("kitty");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 echo \"ARGS $*\" >> {log}\n\
                 if [ \"$4\" = send-text ]; then cat >> {log}; echo >> {log}; fi\n\
                 if [ \"$4\" = ls ]; then\n\
                   [ \"$3\" = unix:helix-here ] || {{ echo 'No matching windows' >&2; exit 1; }}; fi\n\
                 exit 0\n",
                log = log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        (script, log)
    }

    #[test]
    fn sends_to_a_running_helix_or_launches_one() {
        let tmp = TempDir::new().unwrap();
        let (program, log) = fake_kitty(tmp.path());
        let mut k = Kitty {
            program,
            sockets: vec!["unix:own".into(), "unix:helix-here".into()],
            role: "helix".into(),
            helix: "/usr/bin/helix".into(),
        };
        let files = [PathBuf::from("/p/a.md"), PathBuf::from("/p/b c.md")];
        assert!(matches!(
            k.open_files(&files, None, Path::new("/p")),
            Ok(Opened::Existing)
        ));
        let logged = fs::read_to_string(&log).unwrap();
        assert!(logged.contains("ARGS @ --to unix:own ls --match var:role=helix"));
        assert!(!logged.contains("ARGS @ --to unix:own send-text"));
        assert!(
            logged.contains("ARGS @ --to unix:helix-here send-text --match var:role=helix --stdin")
        );
        assert!(
            logged.contains("ARGS @ --to unix:helix-here send-key --match var:role=helix escape")
        );
        assert!(logged.contains("\n:open /p/a.md '/p/b c.md'\r"));
        assert!(logged.contains("ARGS @ --to unix:helix-here focus-window --match var:role=helix"));

        // No Helix anywhere: a new tab on tosk's own socket.
        fs::remove_file(&log).unwrap();
        k.sockets = vec!["unix:own".into()];
        assert!(matches!(
            k.open_files(&files[..1], None, Path::new("/p")),
            Ok(Opened::NewTab)
        ));
        let logged = fs::read_to_string(&log).unwrap();
        assert!(logged.contains(
            "ARGS @ --to unix:own launch --type=tab --var role=helix --cwd /p /usr/bin/helix /p/a.md"
        ));

        // At a line: :goto after :open, or file:line for a new Helix.
        fs::remove_file(&log).unwrap();
        k.open_files(&files[..1], Some(40), Path::new("/p"))
            .unwrap();
        assert!(
            fs::read_to_string(&log)
                .unwrap()
                .contains("/usr/bin/helix /p/a.md:40")
        );
        fs::remove_file(&log).unwrap();
        k.sockets = vec!["unix:helix-here".into()];
        k.open_files(&files[..1], Some(40), Path::new("/p"))
            .unwrap();
        assert!(
            fs::read_to_string(&log)
                .unwrap()
                .contains(":open /p/a.md\r:goto 40\r")
        );

        fs::remove_file(&log).unwrap();
        k.edit_folder(Path::new("/p/src")).unwrap();
        let logged = fs::read_to_string(&log).unwrap();
        assert!(
            logged.contains("launch --type=tab --var role=helix --cwd /p/src /usr/bin/helix .")
        );

        fs::remove_file(&log).unwrap();
        let host = ["-p".to_string(), "22".to_string(), "me@nas".to_string()];
        k.launch_ssh(&host, "hx '/srv/a b.md'").unwrap();
        let logged = fs::read_to_string(&log).unwrap();
        assert!(logged.contains("launch --type=tab kitten ssh -p 22 me@nas hx '/srv/a b.md'"));
    }
}
