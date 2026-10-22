//! `~/.config/tosk/config.toml`. Every key is optional; a missing file
//! means defaults. Problems don't stop tosk: they're collected for a
//! dialog at startup, and whatever is fine is used.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::clipboard::shell_quote;
use crate::command::Command;
use crate::fs::sftp::Host;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    pub show_hidden: bool,
    pub preview: bool,
    /// The kitty user variable that marks the editor window.
    pub editor_role: String,
}

impl Default for General {
    fn default() -> Self {
        General {
            show_hidden: false,
            preview: true,
            editor_role: "helix".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sidebar {
    /// Nerd Font icons in the sidebar and the file list.
    pub icons: bool,
    pub hosts: Vec<HostEntry>,
}

impl Default for Sidebar {
    fn default() -> Self {
        Sidebar {
            icons: true,
            hosts: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostEntry {
    /// A `Host` from `~/.ssh/config`, or `[user@]host[:port]`.
    pub name: String,
    /// Where it opens; the home folder when left out.
    pub root: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Where {
    /// Detached, with the outcome in the log.
    #[default]
    Background,
    /// Typed into the terminal panel.
    Terminal,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandEntry {
    title: Option<String>,
    run: String,
    #[serde(default, rename = "where")]
    place: Where,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct File {
    general: General,
    sidebar: Sidebar,
    keys: BTreeMap<String, String>,
    commands: BTreeMap<String, CommandEntry>,
}

/// A shell command from `[commands.<id>]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Custom {
    pub id: String,
    pub title: String,
    pub run: String,
    pub place: Where,
}

#[derive(Debug, Default)]
pub struct Config {
    pub general: General,
    pub sidebar: Sidebar,
    /// `[keys]` as written: key name → command id or `none`.
    pub keys: Vec<(String, String)>,
    pub commands: Vec<Custom>,
    /// What was wrong, for the startup dialog.
    pub problems: Vec<String>,
}

const PLACEHOLDERS: &[&str] = &["file", "dir", "selection"];

impl Config {
    pub fn load(path: &Path) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => Config {
                problems: vec![format!("can't read {}: {e}", path.display())],
                ..Config::default()
            },
        }
    }

    pub fn parse(text: &str) -> Config {
        let file: File = match toml::from_str(text) {
            Ok(f) => f,
            Err(e) => {
                // Nothing is used from a file that doesn't parse.
                let line = e
                    .span()
                    .map(|span| text[..span.start].matches('\n').count() + 1);
                let msg = e.message().trim().replace('\n', " ");
                let at = line.map(|n| format!("line {n}: ")).unwrap_or_default();
                return Config {
                    problems: vec![format!("{at}{msg}; using the defaults")],
                    ..Config::default()
                };
            }
        };
        let mut problems = Vec::new();
        let mut commands = Vec::new();
        for (id, c) in file.commands {
            if Command::from_id(&id).is_some() {
                problems.push(format!(
                    "[commands.{id}]: {id} is a built-in command; pick another id"
                ));
                continue;
            }
            if let Err(p) = check_placeholders(&c.run) {
                problems.push(format!("[commands.{id}]: {p}"));
                continue;
            }
            commands.push(Custom {
                title: c.title.unwrap_or_else(|| id.clone()),
                id,
                run: c.run,
                place: c.place,
            });
        }
        let mut sidebar = file.sidebar;
        sidebar.hosts.retain(|h| {
            let ok = Host::parse(&h.name).is_some_and(|(_, path)| path.is_none());
            if !ok {
                problems.push(format!(
                    "[[sidebar.hosts]]: “{}” isn't a host name; write [user@]host[:port] and put the folder in root",
                    h.name
                ));
            }
            ok
        });
        Config {
            general: file.general,
            sidebar,
            keys: file.keys.into_iter().collect(),
            commands,
            problems,
        }
    }

    /// The hosts from `[[sidebar.hosts]]`, with where each opens.
    pub fn hosts(&self) -> Vec<(Host, Option<PathBuf>)> {
        self.sidebar
            .hosts
            .iter()
            .filter_map(|h| {
                let (host, _) = Host::parse(&h.name)?;
                Some((host, h.root.as_ref().map(PathBuf::from)))
            })
            .collect()
    }
}

/// Every `{word}` must be a known placeholder; `{{` and `}}` are braces.
fn check_placeholders(run: &str) -> Result<(), String> {
    let mut rest = run;
    while let Some(i) = rest.find('{') {
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix('{') {
            rest = r;
            continue;
        }
        let Some(end) = rest.find('}') else {
            return Err("a { without its }; write {{ for a brace".into());
        };
        let word = &rest[..end];
        if !PLACEHOLDERS.contains(&word) {
            return Err(format!(
                "unknown placeholder {{{word}}}; there's {{file}}, {{dir}} and {{selection}}"
            ));
        }
        rest = &rest[end + 1..];
    }
    Ok(())
}

/// What the placeholders stand for when a command runs.
pub struct Context<'a> {
    pub file: Option<&'a Path>,
    pub dir: &'a Path,
    pub selection: &'a [PathBuf],
}

/// `run` with its placeholders filled in, shell-quoted. Fails when it needs
/// `{file}` and there's no file under the cursor.
pub fn expand(run: &str, cx: &Context) -> Result<String, String> {
    let q = |p: &Path| shell_quote(&p.to_string_lossy());
    let mut out = String::with_capacity(run.len());
    let mut rest = run;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(r) = tail.strip_prefix("{{") {
            out.push('{');
            rest = r;
        } else if let Some(r) = tail.strip_prefix("}}") {
            out.push('}');
            rest = r;
        } else if let Some(r) = tail.strip_prefix("{file}") {
            let file = cx.file.ok_or("there's no file under the cursor")?;
            out.push_str(&q(file));
            rest = r;
        } else if let Some(r) = tail.strip_prefix("{dir}") {
            out.push_str(&q(cx.dir));
            rest = r;
        } else if let Some(r) = tail.strip_prefix("{selection}") {
            let words: Vec<String> = cx.selection.iter().map(|p| q(p)).collect();
            out.push_str(&words.join(" "));
            rest = r;
        } else {
            out.push_str(&tail[..1]);
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_empty_means_defaults() {
        let c = Config::load(Path::new("/nonexistent/tosk/config.toml"));
        assert!(c.problems.is_empty());
        assert!(c.general.preview && !c.general.show_hidden);
        assert_eq!(c.general.editor_role, "helix");
        assert!(c.sidebar.icons);
        let c = Config::parse("");
        assert!(c.problems.is_empty() && c.keys.is_empty());
    }

    #[test]
    fn reads_the_spec_example() {
        let c = Config::parse(
            r#"
[general]
show_hidden = true
preview = false
editor_role = "editor"

[sidebar]
icons = false

[[sidebar.hosts]]
name = "nas"
root = "/"

[keys]
"C-t" = "term.toggle"
"S-Tab" = "focus.prev"
W = "wallpaper"

[commands.wallpaper]
title = "Set as wallpaper"
run = "hyprctl hyprpaper wallpaper ,{file}"
where = "background"
"#,
        );
        assert_eq!(c.problems, Vec::<String>::new());
        assert!(c.general.show_hidden && !c.general.preview);
        assert_eq!(c.general.editor_role, "editor");
        assert!(!c.sidebar.icons);
        let hosts = c.hosts();
        assert_eq!(hosts[0].0.to_string(), "nas");
        assert_eq!(hosts[0].1.as_deref(), Some(Path::new("/")));
        assert_eq!(c.keys.len(), 3);
        assert_eq!(
            c.commands,
            [Custom {
                id: "wallpaper".into(),
                title: "Set as wallpaper".into(),
                run: "hyprctl hyprpaper wallpaper ,{file}".into(),
                place: Where::Background,
            }]
        );
    }

    #[test]
    fn problems_are_explained() {
        let c = Config::parse("[general]\nshow_hiden = true\n");
        assert_eq!(c.problems.len(), 1);
        assert!(
            c.problems[0].starts_with("line 2: unknown field `show_hiden`"),
            "{}",
            c.problems[0]
        );
        assert!(c.problems[0].ends_with("using the defaults"));

        let c = Config::parse("[general\n");
        assert_eq!(c.problems.len(), 1);

        let c = Config::parse(
            r#"
[commands.copy]
run = "x"
[commands.bad]
run = "echo {fiel}"
[commands.open]
run = "echo {file"
[commands.ok]
run = "printf '{{}}' {selection} in {dir}"
where = "terminal"
[[sidebar.hosts]]
name = "nas:/srv"
"#,
        );
        // `copy` isn't a built-in id (`file.copy` is), so it stays.
        let ids: Vec<&str> = c.commands.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["copy", "ok"]);
        assert_eq!(c.commands[1].place, Where::Terminal);
        let p = c.problems.join("\n");
        assert!(
            p.contains("[commands.bad]: unknown placeholder {fiel}"),
            "{p}"
        );
        assert!(p.contains("[commands.open]: a { without its }"), "{p}");
        assert!(p.contains("“nas:/srv” isn't a host name"), "{p}");
        // `file.copy` is built in, but plain `copy` isn't.
        assert!(!p.contains("[commands.copy]"), "{p}");

        let c = Config::parse("[commands.\"file.copy\"]\nrun = \"x\"\n");
        assert!(c.problems[0].contains("is a built-in command"));

        let c = Config::parse("[commands.x]\nrun = \"y\"\nwhere = \"sky\"\n");
        assert!(c.problems[0].contains("sky"), "{}", c.problems[0]);
    }

    #[test]
    fn expands_placeholders() {
        let sel = [PathBuf::from("/m/a b.jpg"), PathBuf::from("/m/c.jpg")];
        let cx = Context {
            file: Some(Path::new("/m/a b.jpg")),
            dir: Path::new("/m"),
            selection: &sel,
        };
        assert_eq!(
            expand("set ,{file} in {dir}: {selection} {{x}}", &cx).unwrap(),
            "set ,'/m/a b.jpg' in /m: '/m/a b.jpg' /m/c.jpg {x}"
        );
        let none = Context { file: None, ..cx };
        assert_eq!(
            expand("echo {file}", &none).unwrap_err(),
            "there's no file under the cursor"
        );
        assert_eq!(expand("ls {dir}", &none).unwrap(), "ls /m");
    }
}
