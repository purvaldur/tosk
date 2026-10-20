//! The sidebar: XDG folders, folders the user pinned, then network hosts
//! (milestone 9), each group separated by a divider.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::icons;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Folders,
    Pinned,
}

pub struct Shortcut {
    pub label: String,
    pub path: PathBuf,
    pub icon: char,
    pub group: Group,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Pin {
    Pinned,
    Unpinned,
    /// It's one of the XDG folders and can't be pinned or unpinned.
    BuiltIn,
}

pub struct Shortcuts {
    pub items: Vec<Shortcut>,
    /// Where pins are saved, one path per line. `None` keeps them in memory.
    store: Option<PathBuf>,
}

impl Shortcuts {
    pub fn load(home: Option<&Path>, store: Option<PathBuf>) -> Shortcuts {
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
        Shortcuts::build(home, folders, pins, store)
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
        };
        if let Some(home) = home {
            s.items.push(Shortcut {
                label: "Home".to_string(),
                path: home.to_path_buf(),
                icon: icons::HOME,
                group: Group::Folders,
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
        self.items.iter().position(|s| s.path == path)
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
        if let Some(dir) = store.parent() {
            fs::create_dir_all(dir)?;
        }
        // Write then rename, so a crash never leaves a half-written file.
        let tmp = store.with_extension("tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(tmp, store)
    }

    /// The shortcut containing `dir`, preferring the deepest:
    /// `~/Videos/show` highlights Videos, not Home.
    pub fn deepest_match(&self, dir: &Path) -> Option<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, s)| dir.starts_with(&s.path))
            .max_by_key(|(_, s)| s.path.components().count())
            .map(|(i, _)| i)
    }
}

fn pinned(path: PathBuf) -> Shortcut {
    Shortcut {
        label: label(&path),
        path,
        icon: icons::PINNED,
        group: Group::Pinned,
    }
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
    fn deepest_shortcut_wins() {
        let s = Shortcuts {
            items: ["/home/u", "/home/u/Videos", "/home/u/code"]
                .into_iter()
                .map(|p| pinned(p.into()))
                .collect(),
            store: None,
        };
        let m = |d: &str| s.deepest_match(Path::new(d));
        assert_eq!(m("/home/u/Videos/show"), Some(1));
        assert_eq!(m("/home/u/Videos"), Some(1));
        assert_eq!(m("/home/u/Videosx"), Some(0));
        assert_eq!(m("/home/u"), Some(0));
        assert_eq!(m("/etc"), None);
    }
}
