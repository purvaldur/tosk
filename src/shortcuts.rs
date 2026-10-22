//! The sidebar: XDG folders, folders the user pinned, then network hosts,
//! each group separated by a divider.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::fs::sftp::{Host, hosts_in_ssh_config};
use crate::icons;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Folders,
    Pinned,
    /// Hosts from `~/.ssh/config` and ones the user added.
    Network,
}

pub struct Shortcut {
    pub label: String,
    /// For a host, empty means its home folder.
    pub path: PathBuf,
    pub icon: char,
    pub group: Group,
    /// The server for network shortcuts.
    pub host: Option<Host>,
    /// Kept in one of tosk's files, so it can be unpinned.
    pub saved: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Pin {
    Pinned,
    Unpinned,
    /// It's one of the XDG folders, or a host from `~/.ssh/config`, and
    /// can't be pinned or unpinned.
    BuiltIn,
}

pub struct Shortcuts {
    pub items: Vec<Shortcut>,
    /// Where pins are saved, one path per line. `None` keeps them in memory.
    store: Option<PathBuf>,
    /// Where added hosts are saved, one `ssh://` address per line.
    hosts_store: Option<PathBuf>,
}

impl Shortcuts {
    /// `store` holds pinned folders, with added hosts in `hosts` beside it;
    /// `ssh_config` lists more hosts.
    /// `config_hosts` come from `[[sidebar.hosts]]` in config.toml.
    pub fn load(
        home: Option<&Path>,
        store: Option<PathBuf>,
        ssh_config: Option<&Path>,
        config_hosts: Vec<(Host, Option<PathBuf>)>,
    ) -> Shortcuts {
        let xdg = [
            (dirs::desktop_dir(), icons::DESKTOP),
            (dirs::document_dir(), icons::DOCUMENTS),
            (dirs::download_dir(), icons::DOWNLOADS),
            (dirs::audio_dir(), icons::MUSIC),
            (dirs::picture_dir(), icons::PICTURES),
            (dirs::video_dir(), icons::VIDEOS),
        ];
        let folders = xdg.into_iter().filter_map(|(p, icon)| Some((p?, icon)));
        let pins = store
            .as_deref()
            .and_then(|f| fs::read(f).ok())
            .map(|bytes| parse_pins(&bytes))
            .unwrap_or_default();
        let hosts_store = store.as_deref().map(|s| s.with_file_name("hosts"));
        let mut s = Shortcuts::build(home, folders, pins, store);
        let mut configured = config_hosts;
        let from_ssh = ssh_config
            .and_then(|f| fs::read_to_string(f).ok())
            .map(|t| hosts_in_ssh_config(&t))
            .unwrap_or_default();
        configured.extend(from_ssh.into_iter().map(|h| (h, None)));
        let added = hosts_store
            .as_deref()
            .and_then(|f| fs::read_to_string(f).ok())
            .map(|t| parse_hosts(&t))
            .unwrap_or_default();
        s.add_hosts(configured, added);
        s.hosts_store = hosts_store;
        s
    }

    /// Hosts from the configs, then the ones the user added.
    pub(crate) fn add_hosts(
        &mut self,
        configured: Vec<(Host, Option<PathBuf>)>,
        added: Vec<(Host, Option<PathBuf>)>,
    ) {
        for (host, path) in configured {
            if self.find_remote(&host, path.as_deref()).is_none() {
                self.items.push(network(host, path, false));
            }
        }
        for (host, path) in added {
            if self.find_remote(&host, path.as_deref()).is_none() {
                self.items.push(network(host, path, true));
            }
        }
    }

    pub(crate) fn build(
        home: Option<&Path>,
        folders: impl Iterator<Item = (PathBuf, char)>,
        pins: Vec<PathBuf>,
        store: Option<PathBuf>,
    ) -> Shortcuts {
        let mut s = Shortcuts {
            items: Vec::new(),
            store,
            hosts_store: None,
        };
        if let Some(home) = home {
            s.items.push(Shortcut {
                label: "Home".to_string(),
                path: home.to_path_buf(),
                icon: icons::HOME,
                group: Group::Folders,
                host: None,
                saved: false,
            });
        }
        for (path, icon) in folders {
            // An unset XDG folder can point at home itself.
            if path.is_dir() && s.find(&path).is_none() {
                s.items.push(Shortcut {
                    label: label(&path),
                    path,
                    icon,
                    group: Group::Folders,
                    host: None,
                    saved: false,
                });
            }
        }
        for path in pins {
            // Pins stay even if the folder is gone for now (an unmounted drive).
            if s.find(&path).is_none() {
                s.items.push(pinned(path));
            }
        }
        s
    }

    fn find(&self, path: &Path) -> Option<usize> {
        self.items
            .iter()
            .position(|s| s.host.is_none() && s.path == path)
    }

    fn find_remote(&self, host: &Host, path: Option<&Path>) -> Option<usize> {
        let path = path.unwrap_or(Path::new(""));
        self.items
            .iter()
            .position(|s| s.host.as_ref() == Some(host) && s.path == path)
    }

    /// Pin a folder on a server (`None` for its home), or unpin it.
    pub fn toggle_pin_remote(&mut self, host: &Host, path: Option<&Path>) -> io::Result<Pin> {
        let result = match self.find_remote(host, path) {
            Some(i) if !self.items[i].saved => return Ok(Pin::BuiltIn),
            Some(i) => {
                self.items.remove(i);
                Pin::Unpinned
            }
            None => {
                let path = path.map(Path::to_path_buf);
                self.items.push(network(host.clone(), path, true));
                Pin::Pinned
            }
        };
        self.save_hosts()?;
        Ok(result)
    }

    /// Unpin the shortcut at `i`, from the sidebar.
    pub fn unpin_at(&mut self, i: usize) -> io::Result<Pin> {
        let Some(s) = self.items.get(i) else {
            return Ok(Pin::BuiltIn);
        };
        match s.host.clone() {
            Some(host) => {
                let path = (!s.path.as_os_str().is_empty()).then(|| s.path.clone());
                self.toggle_pin_remote(&host, path.as_deref())
            }
            None => {
                let path = s.path.clone();
                self.toggle_pin(&path)
            }
        }
    }

    fn save_hosts(&self) -> io::Result<()> {
        let Some(store) = &self.hosts_store else {
            return Ok(());
        };
        let mut text = String::new();
        for s in self.items.iter().filter(|s| s.saved) {
            if let Some(host) = &s.host {
                let path = (!s.path.as_os_str().is_empty()).then_some(s.path.as_path());
                text.push_str(&host.uri(path));
                text.push('\n');
            }
        }
        write_atomically(store, text.as_bytes())
    }

    /// Pin `path`, or unpin it if it's pinned already.
    pub fn toggle_pin(&mut self, path: &Path) -> io::Result<Pin> {
        let result = match self.find(path) {
            Some(i) if self.items[i].group != Group::Pinned => return Ok(Pin::BuiltIn),
            Some(i) => {
                self.items.remove(i);
                Pin::Unpinned
            }
            None => {
                self.items.push(pinned(path.to_path_buf()));
                Pin::Pinned
            }
        };
        self.save()?;
        Ok(result)
    }

    fn save(&self) -> io::Result<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let mut bytes = Vec::new();
        for s in self.items.iter().filter(|s| s.group == Group::Pinned) {
            bytes.extend_from_slice(s.path.as_os_str().as_bytes());
            bytes.push(b'\n');
        }
        write_atomically(store, &bytes)
    }

    /// The shortcut containing `dir` on `host` (`None` here), preferring
    /// the deepest: `~/Videos/show` highlights Videos, not Home. A host's
    /// home shortcut matches anywhere on that host.
    pub fn deepest_match(&self, host: Option<&Host>, dir: &Path) -> Option<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, s)| s.host.as_ref() == host && dir.starts_with(&s.path))
            .max_by_key(|(_, s)| s.path.components().count())
            .map(|(i, _)| i)
    }
}

/// Write then rename, so a crash never leaves a half-written file.
fn write_atomically(store: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = store.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = store.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, store)
}

fn pinned(path: PathBuf) -> Shortcut {
    Shortcut {
        label: label(&path),
        path,
        icon: icons::PINNED,
        group: Group::Pinned,
        host: None,
        saved: true,
    }
}

/// `nas`, or `nas:media` for a folder on it.
fn network(host: Host, path: Option<PathBuf>, saved: bool) -> Shortcut {
    let label = match path.as_deref().and_then(Path::file_name) {
        Some(name) => format!("{host}:{}", name.to_string_lossy()),
        None => host.to_string(),
    };
    Shortcut {
        label,
        path: path.unwrap_or_default(),
        icon: icons::SERVER,
        group: Group::Network,
        host: Some(host),
        saved,
    }
}

/// `ssh://` lines; anything else is skipped.
fn parse_hosts(text: &str) -> Vec<(Host, Option<PathBuf>)> {
    text.lines()
        .filter(|l| l.trim_start().starts_with("ssh://"))
        .filter_map(Host::parse)
        .collect()
}

fn label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn parse_pins(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|&b| b == b'\n')
        .filter(|line| line.starts_with(b"/"))
        .map(|line| PathBuf::from(OsStr::from_bytes(line)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn labels(s: &Shortcuts) -> Vec<&str> {
        s.items.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn folders_exist_and_appear_once() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        fs::create_dir(home.join("Videos")).unwrap();
        fs::create_dir(home.join("Dokumenter")).unwrap();
        let folders = [
            (home.join("Dokumenter"), icons::DOCUMENTS),
            (home.to_path_buf(), icons::DESKTOP),
            (home.join("Missing"), icons::MUSIC),
            (home.join("Videos"), icons::VIDEOS),
        ];
        let s = Shortcuts::build(Some(home), folders.into_iter(), vec![], None);
        assert_eq!(labels(&s), ["Home", "Dokumenter", "Videos"]);
        assert_eq!(s.items[0].icon, icons::HOME);
    }

    #[test]
    fn pins_round_trip_through_the_store() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let store = home.join("state/tosk/shortcuts");
        fs::create_dir(home.join("proj")).unwrap();
        let mut s = Shortcuts::build(Some(home), [].into_iter(), vec![], Some(store.clone()));

        assert_eq!(s.toggle_pin(home).unwrap(), Pin::BuiltIn);
        assert_eq!(s.toggle_pin(&home.join("proj")).unwrap(), Pin::Pinned);
        assert_eq!(s.toggle_pin(Path::new("/mnt/usb")).unwrap(), Pin::Pinned);
        assert_eq!(labels(&s), ["Home", "proj", "usb"]);

        let saved = fs::read(&store).unwrap();
        let reloaded = Shortcuts::build(Some(home), [].into_iter(), parse_pins(&saved), None);
        assert_eq!(labels(&reloaded), ["Home", "proj", "usb"]);
        assert_eq!(reloaded.items[1].group, Group::Pinned);

        assert_eq!(s.toggle_pin(&home.join("proj")).unwrap(), Pin::Unpinned);
        assert_eq!(fs::read(&store).unwrap(), b"/mnt/usb\n");
    }

    #[test]
    fn pin_file_ignores_junk() {
        let pins = parse_pins(b"/a\n\nrelative\n/b c\n");
        assert_eq!(pins, [PathBuf::from("/a"), PathBuf::from("/b c")]);
    }

    #[test]
    fn hosts_from_config_and_added() {
        let tmp = TempDir::new().unwrap();
        let store = tmp.path().join("tosk/shortcuts");
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        fs::write(
            store.with_file_name("hosts"),
            "ssh://nas/srv/media\nnot a host\nssh://me@pi:2222\n",
        )
        .unwrap();
        let config = tmp.path().join("ssh_config");
        fs::write(&config, "Host nas box *.lan\n").unwrap();
        let tv = (Host::parse("tv").unwrap().0, Some(PathBuf::from("/media")));
        let mut s = Shortcuts::load(None, Some(store.clone()), Some(&config), vec![tv]);
        let net: Vec<_> = s
            .items
            .iter()
            .filter(|i| i.group == Group::Network)
            .map(|i| (i.label.as_str(), i.saved))
            .collect();
        assert_eq!(
            net,
            [
                ("tv:media", false),
                ("nas", false),
                ("box", false),
                ("nas:media", true),
                ("me@pi:2222", true)
            ]
        );
        let nas = Host::parse("nas").unwrap().0;
        // Config hosts stay; added ones come and go, saved as ssh:// lines.
        assert_eq!(s.toggle_pin_remote(&nas, None).unwrap(), Pin::BuiltIn);
        let media = Path::new("/srv/media");
        assert_eq!(
            s.toggle_pin_remote(&nas, Some(media)).unwrap(),
            Pin::Unpinned
        );
        assert_eq!(
            s.toggle_pin_remote(&nas, Some(Path::new("/srv/tv")))
                .unwrap(),
            Pin::Pinned
        );
        assert_eq!(
            fs::read_to_string(store.with_file_name("hosts")).unwrap(),
            "ssh://me@pi:2222\nssh://nas/srv/tv\n"
        );
        // The deepest shortcut on the same host is the one that's lit.
        let tv = s.find_remote(&nas, Some(Path::new("/srv/tv"))).unwrap();
        assert_eq!(
            s.deepest_match(Some(&nas), Path::new("/srv/tv/show")),
            Some(tv)
        );
        let home = s.find_remote(&nas, None).unwrap();
        assert_eq!(
            s.deepest_match(Some(&nas), Path::new("/home/me")),
            Some(home)
        );
        assert_eq!(s.deepest_match(None, Path::new("/srv/tv")), None);
        assert_eq!(s.unpin_at(tv).unwrap(), Pin::Unpinned);
    }

    #[test]
    fn deepest_shortcut_wins() {
        let s = Shortcuts {
            items: ["/home/u", "/home/u/Videos", "/home/u/code"]
                .into_iter()
                .map(|p| pinned(p.into()))
                .collect(),
            store: None,
            hosts_store: None,
        };
        let m = |d: &str| s.deepest_match(None, Path::new(d));
        assert_eq!(m("/home/u/Videos/show"), Some(1));
        assert_eq!(m("/home/u/Videos"), Some(1));
        assert_eq!(m("/home/u/Videosx"), Some(0));
        assert_eq!(m("/home/u"), Some(0));
        assert_eq!(m("/etc"), None);
    }
}
