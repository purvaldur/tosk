//! The remote backend: SFTP version 3 spoken over the user's own `ssh`, so
//! `~/.ssh/config`, the agent, keys and known_hosts all apply. Requests go
//! one at a time; each worker holds the connection only for a round trip.

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use super::{Meta, Vfs};
use crate::clipboard::shell_quote;
use crate::entry::{Entry, Kind};

// Packet types.
const INIT: u8 = 1;
const VERSION: u8 = 2;
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const READ: u8 = 5;
const LSTAT: u8 = 7;
const OPENDIR: u8 = 11;
const READDIR: u8 = 12;
const REMOVE: u8 = 13;
const MKDIR: u8 = 14;
const RMDIR: u8 = 15;
const REALPATH: u8 = 16;
const STAT: u8 = 17;
const RENAME: u8 = 18;
const READLINK: u8 = 19;
const STATUS: u8 = 101;
const HANDLE: u8 = 102;
const DATA: u8 = 103;
const NAME: u8 = 104;
const ATTRS: u8 = 105;

// Status codes.
const OK: u32 = 0;
const EOF: u32 = 1;
const NO_SUCH_FILE: u32 = 2;
const PERMISSION_DENIED: u32 = 3;
const OP_UNSUPPORTED: u32 = 8;

// Open flags.
const FXF_READ: u32 = 1;
const FXF_WRITE: u32 = 2;
const FXF_CREAT: u32 = 8;
const FXF_EXCL: u32 = 0x20;

// Attribute flags.
const ATTR_SIZE: u32 = 1;
const ATTR_UIDGID: u32 = 2;
const ATTR_PERMISSIONS: u32 = 4;
const ATTR_ACMODTIME: u32 = 8;
const ATTR_EXTENDED: u32 = 0x8000_0000;

/// Bytes asked for per READ; every server handles this much.
const CHUNK: u32 = 32 * 1024;
/// Exit code the remote trash script uses when `gio` is missing.
const NO_GIO: i32 = 127;

/// `[user@]name[:port]`, as written in the sidebar and `ssh://` paths.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Host {
    pub user: Option<String>,
    /// A name from `~/.ssh/config` or an address.
    pub name: String,
    pub port: Option<u16>,
}

impl Host {
    /// Reads `ssh://[user@]host[:port][/path]`, `[user@]host:path` or a bare
    /// `[user@]host`. The path is `None` for the remote home folder.
    pub fn parse(s: &str) -> Option<(Host, Option<PathBuf>)> {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix("ssh://") {
            let (auth, path) = match rest.find('/') {
                Some(i) => (&rest[..i], Some(&rest[i..])),
                None => (rest, None),
            };
            let (user, hostport) = split_user(auth)?;
            let (name, port) = match hostport.rsplit_once(':') {
                Some((n, p)) => (n, Some(p.parse().ok()?)),
                None => (hostport, None),
            };
            let host = Host::new(user, name, port)?;
            return Some((host, path.map(PathBuf::from)));
        }
        // scp style: the first `:` before any `/` ends the host.
        let colon = s.find(':').filter(|&i| !s[..i].contains('/'));
        let (auth, path) = match colon {
            Some(i) => (&s[..i], Some(&s[i + 1..])),
            None if !s.contains('/') => (s, None),
            None => return None,
        };
        let (user, name) = split_user(auth)?;
        let host = Host::new(user, name, None)?;
        let path = path.filter(|p| !p.is_empty()).map(PathBuf::from);
        Some((host, path))
    }

    fn new(user: Option<&str>, name: &str, port: Option<u16>) -> Option<Host> {
        // Nothing ssh could take for an option, and nothing that needs quoting.
        let ok = |s: &str| {
            !s.is_empty()
                && !s.starts_with('-')
                && s.chars()
                    .all(|c| c.is_alphanumeric() || "._-+[]%".contains(c))
        };
        if !ok(name) || user.is_some_and(|u| !ok(u)) {
            return None;
        }
        Some(Host {
            user: user.map(str::to_string),
            name: name.to_string(),
            port,
        })
    }

    /// What ssh connects to: `user@name`, plus `-p port`.
    pub fn ssh_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(p) = self.port {
            args.extend(["-p".to_string(), p.to_string()]);
        }
        args.push(self.destination());
        args
    }

    fn destination(&self) -> String {
        match &self.user {
            Some(u) => format!("{u}@{}", self.name),
            None => self.name.clone(),
        }
    }

    /// `ssh://host/path`, or `ssh://host` for the home folder.
    pub fn uri(&self, path: Option<&Path>) -> String {
        match path {
            Some(p) => format!("ssh://{self}{}", p.display()),
            None => format!("ssh://{self}"),
        }
    }
}

fn split_user(s: &str) -> Option<(Option<&str>, &str)> {
    match s.rsplit_once('@') {
        Some((u, h)) => Some((Some(u), h)),
        None => Some((None, s)),
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.destination())?;
        if let Some(p) = self.port {
            write!(f, ":{p}")?;
        }
        Ok(())
    }
}

/// Every concrete host name in an ssh config: `Host` lines, wildcards left
/// out.
pub fn hosts_in_ssh_config(text: &str) -> Vec<Host> {
    let mut out: Vec<Host> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let mut words = line.split_whitespace();
        if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("host")) {
            continue;
        }
        for name in words {
            if name.contains(['*', '?', '!']) {
                continue;
            }
            if let Some(h) = Host::new(None, name, None)
                && !out.contains(&h)
            {
                out.push(h);
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default)]
struct Attrs {
    size: Option<u64>,
    uid: Option<u32>,
    gid: Option<u32>,
    perm: Option<u32>,
    mtime: Option<u32>,
}

impl Attrs {
    fn kind(&self) -> Kind {
        match self.perm.map(|p| p & 0o170000) {
            Some(0o040000) => Kind::Dir,
            Some(0o100000) | None => Kind::File,
            Some(_) => Kind::Other,
        }
    }

    fn is_link(&self) -> bool {
        self.perm.is_some_and(|p| p & 0o170000 == 0o120000)
    }
}

/// Builds a request body.
#[derive(Default)]
struct Out(Vec<u8>);

impl Out {
    fn u32(mut self, n: u32) -> Self {
        self.0.extend_from_slice(&n.to_be_bytes());
        self
    }
    fn u64(mut self, n: u64) -> Self {
        self.0.extend_from_slice(&n.to_be_bytes());
        self
    }
    fn bytes(self, b: &[u8]) -> Self {
        let mut s = self.u32(b.len() as u32);
        s.0.extend_from_slice(b);
        s
    }
    fn path(self, p: &Path) -> Self {
        self.bytes(p.as_os_str().as_bytes())
    }
}

/// Reads a response body.
struct In<'a>(&'a [u8]);

impl In<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.0.len() < n {
            return Err(bad("short packet"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn attrs(&mut self) -> io::Result<Attrs> {
        let flags = self.u32()?;
        let mut a = Attrs::default();
        if flags & ATTR_SIZE != 0 {
            a.size = Some(self.u64()?);
        }
        if flags & ATTR_UIDGID != 0 {
            a.uid = Some(self.u32()?);
            a.gid = Some(self.u32()?);
        }
        if flags & ATTR_PERMISSIONS != 0 {
            a.perm = Some(self.u32()?);
        }
        if flags & ATTR_ACMODTIME != 0 {
            let _atime = self.u32()?;
            a.mtime = Some(self.u32()?);
        }
        if flags & ATTR_EXTENDED != 0 {
            for _ in 0..self.u32()? {
                self.bytes()?;
                self.bytes()?;
            }
        }
        Ok(a)
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("sftp: {what}"))
}

/// A STATUS reply as an error (or `Ok` for status OK).
fn status(mut body: In) -> io::Result<()> {
    let code = body.u32()?;
    let msg = String::from_utf8_lossy(&body.bytes().unwrap_or_default()).into_owned();
    let kind = match code {
        OK => return Ok(()),
        EOF => io::ErrorKind::UnexpectedEof,
        NO_SUCH_FILE => io::ErrorKind::NotFound,
        PERMISSION_DENIED => io::ErrorKind::PermissionDenied,
        OP_UNSUPPORTED => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Other,
    };
    let msg = if msg.is_empty() {
        format!("sftp error {code}")
    } else {
        msg
    };
    Err(io::Error::new(kind, msg.to_lowercase()))
}

/// A running `ssh -s host sftp`.
struct Conn {
    child: Child,
    w: ChildStdin,
    r: BufReader<ChildStdout>,
    next_id: u32,
}

impl Conn {
    fn send(&mut self, kind: u8, id: Option<u32>, body: &[u8]) -> io::Result<()> {
        let len = 1 + id.map_or(0, |_| 4) + body.len();
        let mut pkt = Vec::with_capacity(4 + len);
        pkt.extend_from_slice(&(len as u32).to_be_bytes());
        pkt.push(kind);
        if let Some(id) = id {
            pkt.extend_from_slice(&id.to_be_bytes());
        }
        pkt.extend_from_slice(body);
        self.w.write_all(&pkt)?;
        self.w.flush()
    }

    fn recv(&mut self) -> io::Result<(u8, Vec<u8>)> {
        let mut len = [0; 4];
        self.r.read_exact(&mut len)?;
        let len = u32::from_be_bytes(len) as usize;
        if len == 0 || len > 1 << 24 {
            return Err(bad("bad packet length"));
        }
        let mut pkt = vec![0; len];
        self.r.read_exact(&mut pkt)?;
        let kind = pkt[0];
        Ok((kind, pkt.split_off(1)))
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Sftp {
    pub host: Host,
    /// The ssh program; a stand-in in tests.
    ssh: PathBuf,
    /// Options for every ssh run: batch mode, keepalives, multiplexing.
    options: Vec<String>,
    conn: Mutex<Option<Conn>>,
    /// What ssh said on stderr, for connection errors.
    stderr: Arc<Mutex<String>>,
}

impl Sftp {
    pub fn new(host: Host) -> Sftp {
        let control = dirs::runtime_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("tosk-ssh-%C");
        let options = [
            // Never prompt: there's no terminal to answer on.
            "BatchMode=yes".to_string(),
            "ConnectTimeout=10".to_string(),
            "ServerAliveInterval=15".to_string(),
            "ServerAliveCountMax=3".to_string(),
            // One login for browsing, trash, delete and downloads.
            "ControlMaster=auto".to_string(),
            format!("ControlPath={}", control.display()),
            "ControlPersist=60".to_string(),
        ]
        .into_iter()
        .flat_map(|o| ["-o".to_string(), o])
        .collect();
        Sftp::with_program(host, "ssh".into(), options)
    }

    fn with_program(host: Host, ssh: PathBuf, options: Vec<String>) -> Sftp {
        Sftp {
            host,
            ssh,
            options,
            conn: Mutex::new(None),
            stderr: Arc::new(Mutex::new(String::new())),
        }
    }

    fn ssh(&self) -> Command {
        let mut cmd = Command::new(&self.ssh);
        cmd.args(&self.options);
        if let Some(p) = self.host.port {
            cmd.arg("-p").arg(p.to_string());
        }
        cmd
    }

    fn connect(&self) -> io::Result<Conn> {
        let mut cmd = self.ssh();
        cmd.args(["-s", "--", &self.host.destination(), "sftp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| io::Error::new(e.kind(), format!("can't run ssh: {e}")))?;
        let w = child.stdin.take().expect("piped");
        let r = BufReader::new(child.stdout.take().expect("piped"));
        let mut err = child.stderr.take().expect("piped");
        let log = self.stderr.clone();
        log.lock().unwrap().clear();
        thread::spawn(move || {
            let mut buf = [0; 1024];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut log = log.lock().unwrap();
                log.push_str(&String::from_utf8_lossy(&buf[..n]));
                // Only the latest lines matter.
                if log.len() > 4096 {
                    let cut = log.len() - 2048;
                    log.replace_range(..cut, "");
                }
            }
        });
        let mut conn = Conn {
            child,
            w,
            r,
            next_id: 1,
        };
        let hello = conn
            .send(INIT, None, &Out::default().u32(3).0)
            .and_then(|()| conn.recv());
        match hello {
            Ok((VERSION, _)) => Ok(conn),
            Ok(_) => Err(bad("unexpected reply to INIT")),
            Err(e) => {
                // ssh has exited by now; give its stderr a moment to arrive.
                drop(conn);
                thread::sleep(Duration::from_millis(50));
                Err(self.ssh_error(e))
            }
        }
    }

    /// The last thing ssh said, which explains a failed connection better
    /// than a broken pipe does.
    fn ssh_error(&self, e: io::Error) -> io::Error {
        let log = self.stderr.lock().unwrap();
        let said = log
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string);
        match said {
            Some(line) => io::Error::new(io::ErrorKind::ConnectionRefused, line),
            None => e,
        }
    }

    /// One request and its reply. A connection that died while idle is
    /// replaced once.
    fn request(&self, kind: u8, body: Out) -> io::Result<(u8, Vec<u8>)> {
        let mut guard = self.conn.lock().unwrap();
        for attempt in 0..2 {
            let fresh = guard.is_none();
            if fresh {
                *guard = Some(self.connect()?);
            }
            let conn = guard.as_mut().unwrap();
            let id = conn.next_id;
            conn.next_id = conn.next_id.wrapping_add(1);
            if let Err(e) = conn.send(kind, Some(id), &body.0) {
                *guard = None;
                if fresh || attempt == 1 {
                    return Err(self.ssh_error(e));
                }
                continue;
            }
            return match conn.recv() {
                Ok((t, reply)) => {
                    let mut r = In(&reply);
                    if r.u32()? != id {
                        *guard = None;
                        return Err(bad("reply out of order"));
                    }
                    Ok((t, r.0.to_vec()))
                }
                Err(e) => {
                    *guard = None;
                    Err(self.ssh_error(e))
                }
            };
        }
        unreachable!("the second attempt always returns")
    }

    /// A request answered by STATUS.
    fn simple(&self, kind: u8, body: Out) -> io::Result<()> {
        match self.request(kind, body)? {
            (STATUS, b) => status(In(&b)),
            _ => Err(bad("expected a status")),
        }
    }

    fn handle(&self, kind: u8, body: Out) -> io::Result<Vec<u8>> {
        match self.request(kind, body)? {
            (HANDLE, b) => In(&b).bytes(),
            (STATUS, b) => status(In(&b)).and(Err(bad("no handle"))),
            _ => Err(bad("expected a handle")),
        }
    }

    fn close(&self, handle: &[u8]) {
        let _ = self.simple(CLOSE, Out::default().bytes(handle));
    }

    fn attrs(&self, kind: u8, path: &Path) -> io::Result<Attrs> {
        match self.request(kind, Out::default().path(path))? {
            (ATTRS, b) => In(&b).attrs(),
            (STATUS, b) => status(In(&b)).and(Err(bad("no attributes"))),
            _ => Err(bad("expected attributes")),
        }
    }

    /// One name from REALPATH or READLINK.
    fn one_name(&self, kind: u8, path: &Path) -> io::Result<PathBuf> {
        match self.request(kind, Out::default().path(path))? {
            (NAME, b) => {
                let mut r = In(&b);
                if r.u32()? == 0 {
                    return Err(bad("empty name"));
                }
                Ok(PathBuf::from(OsStr::from_bytes(&r.bytes()?)))
            }
            (STATUS, b) => status(In(&b)).and(Err(bad("no name"))),
            _ => Err(bad("expected a name")),
        }
    }

    /// Every name in `dir` with its lstat attributes, `.` and `..` left out.
    fn read_dir(&self, dir: &Path) -> io::Result<Vec<(Vec<u8>, Attrs)>> {
        let handle = self.handle(OPENDIR, Out::default().path(dir))?;
        let mut out = Vec::new();
        let result = loop {
            match self.request(READDIR, Out::default().bytes(&handle)) {
                Ok((NAME, b)) => {
                    let mut r = In(&b);
                    let parsed = (|| {
                        for _ in 0..r.u32()? {
                            let name = r.bytes()?;
                            let _long = r.bytes()?;
                            let attrs = r.attrs()?;
                            if name != b"." && name != b".." {
                                out.push((name, attrs));
                            }
                        }
                        Ok(())
                    })();
                    if let Err(e) = parsed {
                        break Err(e);
                    }
                }
                Ok((STATUS, b)) => match status(In(&b)) {
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break Ok(()),
                    Err(e) => break Err(e),
                    Ok(()) => break Err(bad("readdir said ok")),
                },
                Ok(_) => break Err(bad("expected names")),
                Err(e) => break Err(e),
            }
        };
        self.close(&handle);
        result.map(|()| out)
    }

    fn entry(&self, dir: &Path, name: Vec<u8>, attrs: Attrs) -> Entry {
        let file_name = OsStr::from_bytes(&name).to_os_string();
        let path = dir.join(&file_name);
        let (attrs, link_target, kind) = if attrs.is_link() {
            let target = self.one_name(READLINK, &path).unwrap_or_default();
            match self.attrs(STAT, &path) {
                Ok(t) => (t, Some(target), t.kind()),
                Err(_) => (attrs, Some(target), Kind::Missing),
            }
        } else {
            (attrs, None, attrs.kind())
        };
        let mut e = Entry::new(file_name, kind);
        e.link_target = link_target;
        e.size = attrs.size.unwrap_or(0);
        e.mtime = attrs
            .mtime
            .map(|t| UNIX_EPOCH + Duration::from_secs(u64::from(t)));
        e
    }

    /// Runs `script` with the remote user's shell and returns its output.
    fn exec(&self, script: &str) -> io::Result<Output> {
        let mut cmd = self.ssh();
        cmd.args(["--", &self.host.destination(), script])
            .stdin(Stdio::null());
        cmd.output()
            .map_err(|e| io::Error::new(e.kind(), format!("can't run ssh: {e}")))
    }

    /// Removes `path` and everything under it with SFTP requests alone, for
    /// servers that only allow SFTP.
    fn remove_tree(&self, path: &Path) -> io::Result<()> {
        let attrs = self.attrs(LSTAT, path)?;
        if attrs.kind() == Kind::Dir && !attrs.is_link() {
            for (name, _) in self.read_dir(path)? {
                self.remove_tree(&path.join(OsStr::from_bytes(&name)))?;
            }
            self.simple(RMDIR, Out::default().path(path))
        } else {
            self.simple(REMOVE, Out::default().path(path))
        }
    }

    /// Up to `max` bytes of `path` into `sink`, a chunk at a time;
    /// returns how many.
    fn read_into(&self, path: &Path, max: u64, sink: &mut dyn Write) -> io::Result<u64> {
        let handle = self.handle(OPEN, Out::default().path(path).u32(FXF_READ).u32(0))?;
        let mut done: u64 = 0;
        let result = loop {
            let left = max - done;
            if left == 0 {
                break Ok(());
            }
            let ask = left.min(u64::from(CHUNK)) as u32;
            let body = Out::default().bytes(&handle).u64(done).u32(ask);
            match self.request(READ, body) {
                Ok((DATA, b)) => match In(&b).bytes() {
                    Ok(data) if data.is_empty() => break Ok(()),
                    Ok(data) => {
                        if let Err(e) = sink.write_all(&data) {
                            break Err(e);
                        }
                        done += data.len() as u64;
                    }
                    Err(e) => break Err(e),
                },
                Ok((STATUS, b)) => match status(In(&b)) {
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break Ok(()),
                    Err(e) => break Err(e),
                    Ok(()) => break Err(bad("read said ok")),
                },
                Ok(_) => break Err(bad("expected data")),
                Err(e) => break Err(e),
            }
        };
        self.close(&handle);
        result.map(|()| done)
    }

    /// Copies the remote file `from` to the local file `to`.
    pub fn download(&self, from: &Path, to: &Path) -> io::Result<u64> {
        let mut file = std::io::BufWriter::new(std::fs::File::create(to)?);
        let n = self.read_into(from, u64::MAX, &mut file)?;
        file.flush()?;
        Ok(n)
    }

    /// The home folder: where the server starts.
    pub fn home(&self) -> io::Result<PathBuf> {
        self.one_name(REALPATH, Path::new("."))
    }
}

fn stderr_line(out: &Output) -> String {
    let err = String::from_utf8_lossy(&out.stderr);
    err.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("failed")
        .to_string()
}

impl Vfs for Sftp {
    fn list(&self, dir: &Path) -> io::Result<Vec<Entry>> {
        let names = self.read_dir(dir)?;
        Ok(names
            .into_iter()
            .map(|(name, attrs)| self.entry(dir, name, attrs))
            .collect())
    }

    fn count_children(&self, dir: &Path) -> io::Result<usize> {
        Ok(self.read_dir(dir)?.len())
    }

    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        self.one_name(REALPATH, path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.attrs(LSTAT, path).is_ok()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.simple(RENAME, Out::default().path(from).path(to))
    }

    /// `gio trash` on the server, the same trash its file managers use.
    fn trash(&self, paths: &[PathBuf]) -> io::Result<()> {
        let quoted: Vec<String> = paths
            .iter()
            .map(|p| shell_quote(&p.to_string_lossy()))
            .collect();
        let script = format!(
            "command -v gio >/dev/null 2>&1 || exit {NO_GIO}; gio trash -- {}",
            quoted.join(" ")
        );
        let out = self.exec(&script)?;
        match out.status.code() {
            Some(0) => Ok(()),
            Some(NO_GIO) => Err(io::Error::other(format!(
                "{} has no gio to trash with; D deletes for good",
                self.host
            ))),
            _ => Err(io::Error::other(stderr_line(&out))),
        }
    }

    fn create_file(&self, path: &Path) -> io::Result<()> {
        let flags = FXF_WRITE | FXF_CREAT | FXF_EXCL;
        let handle = self.handle(OPEN, Out::default().path(path).u32(flags).u32(0))?;
        self.close(&handle);
        Ok(())
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        self.simple(MKDIR, Out::default().path(path).u32(0))
    }

    /// `rm -r` on the server, or one request per file where ssh only
    /// allows SFTP.
    fn delete(&self, path: &Path) -> io::Result<()> {
        // Fails early, with a clear error, for something that isn't there.
        self.attrs(LSTAT, path)?;
        let script = format!("rm -r -- {}", shell_quote(&path.to_string_lossy()));
        match self.exec(&script) {
            Ok(out) if out.status.success() => Ok(()),
            _ if !self.exists(path) => Ok(()),
            _ => self.remove_tree(path),
        }
    }

    fn read(&self, path: &Path, max: u64) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        self.read_into(path, max, &mut out)?;
        Ok(out)
    }

    fn stat(&self, path: &Path) -> io::Result<Meta> {
        let lattrs = self.attrs(LSTAT, path)?;
        let name = path.file_name().unwrap_or_default().as_bytes().to_vec();
        let dir = path.parent().unwrap_or(Path::new("/"));
        let entry = self.entry(dir, name, lattrs);
        // A link reports its target's mode, when the target is there.
        let attrs = if lattrs.is_link() {
            self.attrs(STAT, path).unwrap_or(lattrs)
        } else {
            lattrs
        };
        Ok(Meta {
            entry,
            mode: attrs.perm.unwrap_or(0),
            uid: attrs.uid.unwrap_or(0),
            gid: attrs.gid.unwrap_or(0),
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use tempfile::TempDir;

    const SERVER: &str = "/usr/lib/ssh/sftp-server";

    /// A stand-in `ssh`: `-s … sftp` runs the real sftp-server in `home`;
    /// anything else runs the command with `sh` there, with only the tools
    /// in `bin` on the PATH. Every call is logged.
    pub(crate) fn fake(tmp: &Path) -> Option<Sftp> {
        if !Path::new(SERVER).exists() {
            return None;
        }
        let home = tmp.join("home");
        let bin = tmp.join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        for tool in ["rm", "sh"] {
            let real = which(tool);
            symlink(real, bin.join(tool)).unwrap();
        }
        let script = tmp.join("ssh");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 echo \"$*\" >> {log}\n\
                 for last; do :; done\n\
                 cd {home}\n\
                 case \" $* \" in *\" -s \"*) exec {SERVER} ;; esac\n\
                 PATH={bin} exec sh -c \"$last\"\n",
                log = tmp.join("ssh.log").display(),
                home = home.display(),
                bin = bin.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let host = Host::parse("me@nas").unwrap().0;
        Some(Sftp::with_program(
            host,
            script,
            vec!["-o".into(), "BatchMode=yes".into()],
        ))
    }

    fn which(tool: &str) -> PathBuf {
        ["/usr/bin", "/bin"]
            .iter()
            .map(|d| Path::new(d).join(tool))
            .find(|p| p.exists())
            .unwrap()
    }

    #[test]
    fn parses_hosts() {
        let p = |s: &str| Host::parse(s).map(|(h, p)| (h.to_string(), p));
        assert_eq!(p("nas"), Some(("nas".into(), None)));
        assert_eq!(p("thor@nas"), Some(("thor@nas".into(), None)));
        assert_eq!(
            p("thor@nas:/srv/media"),
            Some(("thor@nas".into(), Some("/srv/media".into())))
        );
        assert_eq!(p("nas:"), Some(("nas".into(), None)));
        assert_eq!(
            p("ssh://thor@nas:2222/srv"),
            Some(("thor@nas:2222".into(), Some("/srv".into())))
        );
        assert_eq!(p("ssh://nas"), Some(("nas".into(), None)));
        assert_eq!(p("ssh://nas/"), Some(("nas".into(), Some("/".into()))));
        // Local paths, and nothing ssh could read as an option.
        assert_eq!(p("/home/thor"), None);
        assert_eq!(p("./a:b"), None);
        assert_eq!(p("-oProxyCommand=x"), None);
        assert_eq!(p("ssh://-p/x"), None);
        assert_eq!(p("ssh://nas:port/x"), None);
        assert_eq!(p("na s"), None);

        let (h, _) = Host::parse("ssh://u@h:22").unwrap();
        assert_eq!(h.ssh_args(), ["-p", "22", "u@h"]);
        assert_eq!(h.uri(Some(Path::new("/srv"))), "ssh://u@h:22/srv");
    }

    #[test]
    fn reads_ssh_config_hosts() {
        let config =
            "Host nas pi\n  HostName 10.0.0.2\nHost *.lan !bad\nhost   box\nMatch all\nHost nas\n";
        let names: Vec<String> = hosts_in_ssh_config(config)
            .iter()
            .map(Host::to_string)
            .collect();
        assert_eq!(names, ["nas", "pi", "box"]);
    }

    #[test]
    fn browses_and_edits_over_sftp() {
        let tmp = TempDir::new().unwrap();
        let Some(s) = fake(tmp.path()) else { return };
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join("media/show")).unwrap();
        fs::write(home.join("media/a.txt"), "hello").unwrap();
        fs::write(home.join("media/show/e01.mkv"), "x").unwrap();
        symlink("a.txt", home.join("media/link")).unwrap();
        symlink("gone", home.join("media/broken")).unwrap();
        symlink("show", home.join("media/dirlink")).unwrap();

        assert_eq!(s.home().unwrap(), home.canonicalize().unwrap());
        let media = home.join("media");
        let mut v = s.list(&media).unwrap();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "broken", "dirlink", "link", "show"]);
        let get = |n: &str| v.iter().find(|e| e.name == n).unwrap();
        assert_eq!((get("a.txt").kind, get("a.txt").size), (Kind::File, 5));
        assert!(get("a.txt").mtime.is_some());
        assert_eq!(get("show").kind, Kind::Dir);
        assert_eq!(get("link").link_target.as_deref(), Some(Path::new("a.txt")));
        assert_eq!((get("link").kind, get("link").size), (Kind::File, 5));
        assert_eq!(get("broken").kind, Kind::Missing);
        assert_eq!(get("dirlink").kind, Kind::Dir);
        assert_eq!(s.count_children(&media.join("show")).unwrap(), 1);
        assert_eq!(
            s.resolve(&media.join("dirlink")).unwrap(),
            media.join("show").canonicalize().unwrap()
        );

        assert_eq!(s.read(&media.join("a.txt"), 3).unwrap(), b"hel");
        assert_eq!(s.read(&media.join("a.txt"), 100).unwrap(), b"hello");
        let big: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        fs::write(media.join("big"), &big).unwrap();
        assert_eq!(s.read(&media.join("big"), 1 << 20).unwrap(), big);
        let copy = tmp.path().join("copy");
        assert_eq!(s.download(&media.join("big"), &copy).unwrap(), 100_000);
        assert_eq!(fs::read(&copy).unwrap(), big);

        let meta = s.stat(&media.join("a.txt")).unwrap();
        assert_eq!(meta.mode & 0o170000, 0o100000);
        assert_eq!(meta.entry.size, 5);

        // Create, without overwriting.
        s.create_file(&media.join("new.txt")).unwrap();
        assert!(s.create_file(&media.join("a.txt")).is_err());
        assert_eq!(fs::read(media.join("a.txt")).unwrap(), b"hello");
        s.create_dir(&media.join("dir")).unwrap();
        assert!(media.join("dir").is_dir());
        assert!(s.exists(&media.join("new.txt")));
        assert!(!s.exists(&media.join("nope")));

        s.rename(&media.join("new.txt"), &media.join("renamed.txt"))
            .unwrap();
        assert!(media.join("renamed.txt").exists());

        let err = s.list(&media.join("nope")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn deletes_and_trashes() {
        let tmp = TempDir::new().unwrap();
        let Some(s) = fake(tmp.path()) else { return };
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join("tree/sub")).unwrap();
        fs::write(home.join("tree/sub/f"), "x").unwrap();
        fs::create_dir_all(home.join("keep")).unwrap();
        fs::write(home.join("keep/f"), "x").unwrap();
        symlink(home.join("keep"), home.join("link")).unwrap();

        s.delete(&home.join("link")).unwrap();
        assert!(home.join("keep/f").exists());
        s.delete(&home.join("tree")).unwrap();
        assert!(!home.join("tree").exists());
        assert!(s.delete(&home.join("tree")).is_err());
        // Over SFTP alone, as on a server that refuses commands.
        fs::create_dir_all(home.join("t2/sub")).unwrap();
        fs::write(home.join("t2/sub/f"), "x").unwrap();
        s.remove_tree(&home.join("t2")).unwrap();
        assert!(!home.join("t2").exists());

        // The stand-in server has no gio: refuse, and leave the file be.
        let err = s.trash(&[home.join("keep/f")]).unwrap_err();
        assert!(err.to_string().contains("has no gio"), "{err}");
        assert!(home.join("keep/f").exists());
        // With one, the paths reach it quoted.
        let gio = tmp.path().join("bin/gio");
        fs::write(
            &gio,
            format!(
                "#!/bin/sh\necho \"$@\" > {}\n",
                tmp.path().join("gio.log").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&gio, fs::Permissions::from_mode(0o755)).unwrap();
        s.trash(&[home.join("keep/f"), home.join("it's")]).unwrap();
        let logged = fs::read_to_string(tmp.path().join("gio.log")).unwrap();
        assert_eq!(
            logged.trim(),
            format!(
                "trash -- {} {}",
                home.join("keep/f").display(),
                home.join("it's").display()
            )
        );
    }

    #[test]
    fn connection_errors_say_what_ssh_said() {
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("ssh");
        fs::write(
            &script,
            "#!/bin/sh\necho 'ssh: connect to host nas port 22: Connection refused' >&2\nexit 255\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let s = Sftp::with_program(Host::parse("nas").unwrap().0, script, vec![]);
        let err = s.list(Path::new("/")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "ssh: connect to host nas port 22: Connection refused"
        );
    }
}
