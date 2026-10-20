//! The preview worker and the per-kind content it builds.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::SystemTime;

use ratatui::layout::Size;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::entry::Entry;
use crate::fs::Vfs;
use crate::sort::sort_entries;

/// How much of a file is read to decide whether it's text.
const SNIFF_BYTES: usize = 8 * 1024;
/// How much of a text file is read for its preview.
const TEXT_BYTES: u64 = 64 * 1024;
/// Longer lines are cut before highlighting; the pane can't show them anyway.
const MAX_LINE_BYTES: usize = 1000;
/// Folder previews count children for at most this many subfolders.
const MAX_COUNTED: usize = 500;
/// Cached previews; images are the heavy ones.
const CACHE_LIMIT: usize = 64;

const THEME: &[u8] = include_bytes!("../assets/catppuccin-mocha.tmTheme");

/// What a preview is of. The size is the preview body in cells, since text
/// and images are built to fit it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub path: PathBuf,
    pub mtime: Option<SystemTime>,
    pub size: (u16, u16),
}

pub enum Content {
    /// Sorted, hidden entries included.
    Folder(Vec<Entry>),
    Text(Vec<Line<'static>>),
    Image(Protocol),
    /// Type, size and date, for kinds without a previewer.
    Info {
        label: &'static str,
        size: u64,
        mtime: Option<SystemTime>,
    },
    Error(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Text,
    Image,
    Pdf,
    Video,
    Audio,
    Archive,
    Binary,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            FileKind::Text => "Text",
            FileKind::Image => "Image",
            FileKind::Pdf => "PDF document",
            FileKind::Video => "Video",
            FileKind::Audio => "Audio",
            FileKind::Archive => "Archive",
            FileKind::Binary => "Binary file",
        }
    }
}

/// By extension first; anything unknown is text unless it has a NUL byte
/// in its first 8 KB.
pub fn classify(path: &Path, head: &[u8]) -> FileKind {
    let by_ext = path
        .file_name()
        .and_then(|n| kind_by_extension(&n.to_string_lossy()));
    by_ext.unwrap_or_else(|| {
        let head = &head[..head.len().min(SNIFF_BYTES)];
        if head.contains(&0) {
            FileKind::Binary
        } else {
            FileKind::Text
        }
    })
}

/// The media kinds that are known by extension alone.
pub fn kind_by_extension(name: &str) -> Option<FileKind> {
    let (_, ext) = name.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico" | "tga"
        | "qoi" | "pnm" | "pbm" | "pgm" | "ppm" => Some(FileKind::Image),
        "pdf" => Some(FileKind::Pdf),
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "wmv" | "flv" => Some(FileKind::Video),
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" | "wma" => Some(FileKind::Audio),
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" => {
            Some(FileKind::Archive)
        }
        _ => None,
    }
}

pub struct Previewer {
    requests: Sender<Key>,
    results: Receiver<(Key, Content)>,
    cache: HashMap<Key, Content>,
    pending: Option<Key>,
}

impl Previewer {
    pub fn spawn(vfs: Arc<dyn Vfs>, picker: Option<Picker>) -> Previewer {
        let (requests, request_rx) = mpsc::channel::<Key>();
        let (result_tx, results) = mpsc::channel();
        thread::spawn(move || {
            let mut builder = Builder::new(vfs, picker);
            while let Ok(mut key) = request_rx.recv() {
                // Only the newest request matters when the cursor moves fast.
                while let Ok(newer) = request_rx.try_recv() {
                    key = newer;
                }
                let content = builder.build(&key);
                if result_tx.send((key, content)).is_err() {
                    break;
                }
            }
        });
        Previewer {
            requests,
            results,
            cache: HashMap::new(),
            pending: None,
        }
    }

    /// Ask for a preview unless it's cached or already on its way.
    pub fn want(&mut self, key: &Key) {
        if self.cache.contains_key(key) || self.pending.as_ref() == Some(key) {
            return;
        }
        self.pending = Some(key.clone());
        let _ = self.requests.send(key.clone());
    }

    pub fn get(&self, key: &Key) -> Option<&Content> {
        self.cache.get(key)
    }

    /// Takes finished previews; returns whether any arrived.
    pub fn drain(&mut self) -> bool {
        let mut any = false;
        while let Ok((key, content)) = self.results.try_recv() {
            if self.pending.as_ref() == Some(&key) {
                self.pending = None;
            }
            if self.cache.len() >= CACHE_LIMIT {
                self.cache.clear();
            }
            self.cache.insert(key, content);
            any = true;
        }
        any
    }
}

struct Builder {
    vfs: Arc<dyn Vfs>,
    picker: Option<Picker>,
    /// Loaded on the first text preview.
    syntax: Option<(SyntaxSet, Theme)>,
}

impl Builder {
    fn new(vfs: Arc<dyn Vfs>, picker: Option<Picker>) -> Builder {
        Builder {
            vfs,
            picker,
            syntax: None,
        }
    }

    fn build(&mut self, key: &Key) -> Content {
        let meta = match std::fs::metadata(&key.path) {
            Ok(m) => m,
            Err(e) => return Content::Error(e.to_string()),
        };
        if meta.is_dir() {
            return self.folder(&key.path);
        }
        let info = |kind: FileKind| Content::Info {
            label: kind.label(),
            size: meta.len(),
            mtime: meta.modified().ok(),
        };
        let mut head = Vec::new();
        let read = File::open(&key.path).and_then(|f| f.take(TEXT_BYTES).read_to_end(&mut head));
        if let Err(e) = read {
            return Content::Error(e.to_string());
        }
        let (w, h) = key.size;
        match classify(&key.path, &head) {
            FileKind::Text => Content::Text(self.highlight(&key.path, &head, h as usize)),
            FileKind::Image => match &self.picker {
                Some(picker) => image(picker, &key.path, Size::new(w, h)),
                None => info(FileKind::Image),
            },
            kind => info(kind),
        }
    }

    fn folder(&self, path: &Path) -> Content {
        let mut entries = match self.vfs.list(path) {
            Ok(v) => v,
            Err(e) => return Content::Error(e.to_string()),
        };
        sort_entries(&mut entries);
        for e in entries.iter_mut().filter(|e| e.is_dir()).take(MAX_COUNTED) {
            e.children = self.vfs.count_children(&path.join(&e.file_name)).ok();
        }
        Content::Folder(entries)
    }

    fn highlight(&mut self, path: &Path, bytes: &[u8], rows: usize) -> Vec<Line<'static>> {
        let (ss, theme) = self.syntax.get_or_insert_with(|| {
            let theme =
                ThemeSet::load_from_reader(&mut Cursor::new(THEME)).expect("bundled theme parses");
            (SyntaxSet::load_defaults_newlines(), theme)
        });
        let text = String::from_utf8_lossy(bytes);
        let first = text.lines().next().unwrap_or("");
        let by_name = |s: &str| ss.find_syntax_by_extension(s);
        let syntax = path
            .extension()
            .and_then(|e| by_name(&e.to_string_lossy()))
            .or_else(|| path.file_name().and_then(|n| by_name(&n.to_string_lossy())))
            .or_else(|| ss.find_syntax_by_first_line(first))
            .unwrap_or_else(|| ss.find_syntax_plain_text());
        let mut hl = HighlightLines::new(syntax, theme);
        LinesWithEndings::from(&text)
            .take(rows)
            .map(|line| {
                let line = cut(line, MAX_LINE_BYTES);
                let spans = match hl.highlight_line(line, ss) {
                    Ok(ranges) => ranges
                        .into_iter()
                        .map(|(style, s)| {
                            let c = style.foreground;
                            Span::styled(clean(s), Style::default().fg(Color::Rgb(c.r, c.g, c.b)))
                        })
                        .collect(),
                    Err(_) => vec![Span::raw(clean(line))],
                };
                Line::from(spans)
            })
            .collect()
    }
}

fn image(picker: &Picker, path: &Path, size: Size) -> Content {
    let decoded = image::ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| e.to_string())
        .and_then(|r| r.decode().map_err(|e| e.to_string()));
    match decoded.and_then(|img| {
        picker
            .new_protocol(img, size, Resize::Fit(None))
            .map_err(|e| e.to_string())
    }) {
        Ok(p) => Content::Image(p),
        Err(e) => Content::Error(e),
    }
}

fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Tabs become spaces; line endings and other control characters go.
fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\t' => out.push_str("    "),
            '\n' | '\r' => {}
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::local::Local;
    use ratatui_image::picker::ProtocolType;
    use std::fs;
    use std::time::Duration;
    use tempfile::TempDir;

    fn key(path: PathBuf) -> Key {
        Key {
            path,
            mtime: None,
            size: (40, 10),
        }
    }

    fn builder(picker: Option<Picker>) -> Builder {
        Builder::new(Arc::new(Local), picker)
    }

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn classifies() {
        let p = Path::new;
        assert_eq!(classify(p("a.PNG"), b""), FileKind::Image);
        assert_eq!(classify(p("a.mkv"), b""), FileKind::Video);
        assert_eq!(classify(p("a.tar"), b""), FileKind::Archive);
        assert_eq!(classify(p("notes"), b"hello\n"), FileKind::Text);
        assert_eq!(classify(p("blob.bin"), b"ab\0cd"), FileKind::Binary);
        let mut late_nul = vec![b'a'; SNIFF_BYTES];
        late_nul.push(0);
        assert_eq!(classify(p("x"), &late_nul), FileKind::Text);
    }

    #[test]
    fn text_is_highlighted_and_cut_to_rows() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("main.rs");
        let src: String = (0..30)
            .map(|i| format!("let x{i} = \"s\";\t// c\n"))
            .collect();
        fs::write(&path, src).unwrap();
        let Content::Text(lines) = builder(None).build(&key(path)) else {
            panic!("expected text");
        };
        assert_eq!(lines.len(), 10);
        assert_eq!(text_of(&lines[0]), "let x0 = \"s\";    // c");
        // More than one colour means the Rust syntax was used.
        let colours: std::collections::HashSet<_> =
            lines[0].spans.iter().map(|s| s.style.fg).collect();
        assert!(colours.len() > 1);
    }

    #[test]
    fn folder_lists_sorted_with_counts() {
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("b/inner")).unwrap();
        fs::write(r.join("a.txt"), "").unwrap();
        let Content::Folder(entries) = builder(None).build(&key(r.to_path_buf())) else {
            panic!("expected folder");
        };
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["b", "a.txt"]);
        assert_eq!(entries[0].children, Some(1));
    }

    #[test]
    fn binary_and_images() {
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("blob");
        fs::write(&bin, b"\x7fELF\0\0\0").unwrap();
        assert!(matches!(
            builder(None).build(&key(bin)),
            Content::Info {
                label: "Binary file",
                size: 7,
                ..
            }
        ));

        let png = tmp.path().join("dot.png");
        image::RgbImage::from_pixel(4, 4, image::Rgb([200, 100, 50]))
            .save(&png)
            .unwrap();
        // Without kitty graphics, images get the info card.
        assert!(matches!(
            builder(None).build(&key(png.clone())),
            Content::Info { label: "Image", .. }
        ));
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        assert!(matches!(
            builder(Some(picker)).build(&key(png)),
            Content::Image(_)
        ));

        let broken = tmp.path().join("broken.png");
        fs::write(&broken, "not a png").unwrap();
        assert!(matches!(
            builder(Some(Picker::halfblocks())).build(&key(broken)),
            Content::Error(_)
        ));
    }

    #[test]
    fn worker_answers_the_newest_request() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a"), "a").unwrap();
        let mut p = Previewer::spawn(Arc::new(Local), None);
        let k = key(tmp.path().join("a"));
        p.want(&k);
        for _ in 0..200 {
            if p.drain() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(p.get(&k), Some(Content::Text(_))));
        assert!(p.pending.is_none());
    }
}
