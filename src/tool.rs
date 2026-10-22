//! Running the optional helper programs (`pdftoppm`, `ffprobe`, `git`, …)
//! with a time limit, so a stuck one can't hold up its worker for good.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum ToolError {
    /// The program isn't installed.
    Missing(String),
    /// It ran but failed or took too long; the reason, for the preview.
    Failed(String),
}

impl ToolError {
    pub fn message(&self) -> String {
        match self {
            ToolError::Missing(p) => format!("{p} isn't installed"),
            ToolError::Failed(why) => why.clone(),
        }
    }
}

fn spawn(
    program: &str,
    args: &[&std::ffi::OsStr],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
) -> Result<Child, ToolError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ToolError::Missing(program.to_string()),
        _ => ToolError::Failed(format!("can't run {program}: {e}")),
    })
}

/// Everything `program` writes to stdout, if it exits successfully within
/// `timeout`. `env` is added to tosk's own environment.
pub fn output(
    program: &str,
    args: &[&std::ffi::OsStr],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<Vec<u8>, ToolError> {
    let mut child = spawn(program, args, cwd, env)?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    let err = thread::spawn(move || {
        let mut err = String::new();
        let _ = stderr.read_to_string(&mut err);
        err
    });
    let Ok(out) = rx.recv_timeout(timeout) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(ToolError::Failed(format!("{program} took too long")));
    };
    let status = child
        .wait()
        .map_err(|e| ToolError::Failed(format!("{program}: {e}")))?;
    if status.success() {
        return Ok(out);
    }
    let err = err.join().unwrap_or_default();
    let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    Err(ToolError::Failed(if first.is_empty() {
        format!("{program} failed")
    } else {
        format!("{program}: {}", first.trim())
    }))
}

/// The first `max` lines of stdout, and whether there were more. The
/// program is stopped once enough has been read.
pub fn first_lines(
    program: &str,
    args: &[&std::ffi::OsStr],
    max: usize,
    timeout: Duration,
) -> Result<(Vec<String>, bool), ToolError> {
    let mut child = spawn(program, args, None, &[])?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut lines = Vec::new();
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            lines.push(line);
            if lines.len() > max {
                break;
            }
        }
        let _ = tx.send(lines);
    });
    let got = rx.recv_timeout(timeout);
    let _ = child.kill();
    let status = child.wait();
    let Ok(mut lines) = got else {
        return Err(ToolError::Failed(format!("{program} took too long")));
    };
    let more = lines.len() > max;
    lines.truncate(max);
    // A kill after enough lines isn't a failure; an early exit with
    // nothing read is.
    if lines.is_empty() && !status.is_ok_and(|s| s.success()) {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let first = err.lines().next().unwrap_or("").trim().to_string();
        return Err(ToolError::Failed(if first.is_empty() {
            format!("{program} failed")
        } else {
            first
        }));
    }
    Ok((lines, more))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn args<'a>(a: &'a [&'a str]) -> Vec<&'a OsStr> {
        a.iter().map(OsStr::new).collect()
    }

    #[test]
    fn output_and_errors() {
        let t = Duration::from_secs(5);
        assert_eq!(
            output("sh", &args(&["-c", "printf hi"]), None, &[], t).unwrap(),
            b"hi"
        );
        assert_eq!(
            output("sh", &args(&["-c", "echo nope >&2; exit 3"]), None, &[], t),
            Err(ToolError::Failed("sh: nope".into()))
        );
        assert_eq!(
            output("tosk-no-such-program", &[], None, &[], t),
            Err(ToolError::Missing("tosk-no-such-program".into()))
        );
        assert_eq!(
            output(
                "sleep",
                &args(&["5"]),
                None,
                &[],
                Duration::from_millis(100)
            ),
            Err(ToolError::Failed("sleep took too long".into()))
        );
    }

    #[test]
    fn stops_after_enough_lines() {
        let t = Duration::from_secs(5);
        // `yes` never ends on its own.
        let (lines, more) = first_lines("yes", &args(&["x"]), 3, t).unwrap();
        assert_eq!((lines, more), (vec!["x".to_string(); 3], true));
        let (lines, more) = first_lines("printf", &args(&["a\\nb\\n"]), 3, t).unwrap();
        assert_eq!(
            (lines, more),
            (vec!["a".to_string(), "b".to_string()], false)
        );
    }
}
