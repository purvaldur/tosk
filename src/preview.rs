//! The preview worker and the per-kind content it builds.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

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
use crate::sort::{Sort, sort_entries};
use crate::tool::{self, ToolError};

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
/// Limits for the helper programs: rendering can take a while on a big
/// file, reading tags shouldn't.
const RENDER_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

const THEME: &[u8] = include_bytes!("../assets/catppuccin-mocha.tmTheme");

/// What a preview is of. The size is the preview body in cells, since text
/// and images are built to fit it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub path: PathBuf,
    pub mtime: Option<SystemTime>,
    pub size: (u16, u16),
    /// Show text around this 1-based line instead of from the top.
    pub line: Option<u64>,
}

pub enum Content {
    /// Sorted, hidden entries included.
    Folder(Vec<Entry>),
    Text {
        /// The 1-based number of the first line.
        first: u64,
        /// The line asked for by `Key::line`, to mark.
        focus: Option<u64>,
        lines: Vec<Line<'static>>,
    },
    Image(Protocol),
    /// A PDF page, video thumbnail or album cover (with kitty graphics),
    /// and facts underneath: pages, duration, tags.
    Media {
        image: Option<Protocol>,
        facts: Vec<(&'static str, String)>,
    },
    /// The first entries of an archive, and whether there are more.
    Archive {
        entries: Vec<String>,
        more: bool,
    },
    /// Type, size and date, for kinds without a previewer, and why.
    Info {
        label: &'static str,
        size: u64,
        mtime: Option<SystemTime>,
        note: String,
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
        let noted = |kind: FileKind, note: String| Content::Info {
            label: kind.label(),
            size: meta.len(),
            mtime: meta.modified().ok(),
            note,
        };
        let info = |kind: FileKind| noted(kind, "no previewer".into());
        let mut head = Vec::new();
        let read = File::open(&key.path).and_then(|f| f.take(TEXT_BYTES).read_to_end(&mut head));
        if let Err(e) = read {
            return Content::Error(e.to_string());
        }
        let (w, h) = key.size;
        match classify(&key.path, &head) {
            FileKind::Text => self.text(key, &head),
            FileKind::Image => match &self.picker {
                Some(picker) => image(picker, &key.path, Size::new(w, h)),
                None => info(FileKind::Image),
            },
            kind @ (FileKind::Pdf | FileKind::Video | FileKind::Audio | FileKind::Archive) => {
                let built = match kind {
                    FileKind::Pdf => self.pdf(key),
                    FileKind::Archive => archive(key),
                    _ => self.media(key, kind),
                };
                built.unwrap_or_else(|e| {
                    let note = match e {
                        ToolError::Missing(p) => format!("no previewer: {p} isn't installed"),
                        e => e.message(),
                    };
                    noted(kind, note)
                })
            }
            FileKind::Binary => info(FileKind::Binary),
        }
    }

    /// The first page, and the page count from `pdfinfo` if it's there.
    fn pdf(&self, key: &Key) -> Result<Content, ToolError> {
        let path = key.path.as_os_str();
        let mut facts = Vec::new();
        if let Ok(out) = tool::output("pdfinfo", &[path], None, &[], PROBE_TIMEOUT) {
            let out = String::from_utf8_lossy(&out);
            let field = |name: &str| {
                out.lines()
                    .find_map(|l| l.strip_prefix(name))
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            };
            if let Some(title) = field("Title:") {
                facts.push(("Title", title));
            }
            if let Some(pages) = field("Pages:") {
                facts.push(("Pages", pages));
            }
        }
        let Some(picker) = &self.picker else {
            return with_facts(None, facts);
        };
        let size = image_cells(key.size, facts.len());
        let px = pixels(picker, size).to_string();
        let args = [
            "-png",
            "-f",
            "1",
            "-l",
            "1",
            "-singlefile",
            "-scale-to",
            &px,
        ];
        let mut argv: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        argv.push(path);
        let png = tool::output("pdftoppm", &argv, None, &[], RENDER_TIMEOUT)?;
        let image = protocol_from_png(picker, &png, size)?;
        with_facts(Some(image), facts)
    }

    /// Videos get a thumbnail, audio its embedded cover; both get facts
    /// from `ffprobe`.
    fn media(&self, key: &Key, kind: FileKind) -> Result<Content, ToolError> {
        let path = key.path.as_os_str();
        let entries = "format=duration:format_tags:stream=codec_type,codec_name,width,height,sample_rate,channels";
        let flags = ["-v", "error", "-show_entries", entries, "-of", "compact"];
        let args: Vec<&OsStr> = flags.iter().map(OsStr::new).chain([path]).collect();
        let probe = tool::output("ffprobe", &args, None, &[], PROBE_TIMEOUT)
            .map(|out| parse_probe(&String::from_utf8_lossy(&out)));
        let facts = match (&probe, kind) {
            (Ok(p), FileKind::Video) => video_facts(p),
            (Ok(p), _) => audio_facts(p),
            (Err(_), _) => Vec::new(),
        };
        // Audio only has a picture when there's a cover stream.
        let has_picture = match (&probe, kind) {
            (_, FileKind::Video) => true,
            (Ok(p), _) => p
                .streams
                .iter()
                .any(|s| s.get("codec_type").map(String::as_str) == Some("video")),
            (Err(_), _) => false,
        };
        let image = match &self.picker {
            Some(picker) if has_picture => {
                let size = image_cells(key.size, facts.len());
                let px = pixels(picker, size).to_string();
                let flags = ["-o", "-", "-c", "png", "-s", &px];
                let mut argv: Vec<&OsStr> = vec![OsStr::new("-i"), path];
                argv.extend(flags.iter().map(OsStr::new));
                if kind == FileKind::Audio {
                    // Prefer the embedded cover.
                    argv.push(OsStr::new("-m"));
                }
                tool::output("ffmpegthumbnailer", &argv, None, &[], RENDER_TIMEOUT)
                    .and_then(|png| protocol_from_png(picker, &png, size))
            }
            _ => Err(ToolError::Failed("no picture".into())),
        };
        match (image, probe) {
            (Ok(image), _) => with_facts(Some(image), facts),
            (Err(_), Ok(_)) => with_facts(None, facts),
            // Neither worked: say why ffprobe didn't.
            (Err(_), Err(e)) => Err(e),
        }
    }

    fn folder(&self, path: &Path) -> Content {
        let mut entries = match self.vfs.list(path) {
            Ok(v) => v,
            Err(e) => return Content::Error(e.to_string()),
        };
        sort_entries(&mut entries, Sort::default());
        for e in entries.iter_mut().filter(|e| e.is_dir()).take(MAX_COUNTED) {
            e.children = self.vfs.count_children(&path.join(&e.file_name)).ok();
        }
        Content::Folder(entries)
    }

    /// From the top, or around `key.line` with it a third of the way down.
    fn text(&mut self, key: &Key, head: &[u8]) -> Content {
        let rows = key.size.1 as usize;
        let first_line = head.split(|&b| b == b'\n').next().unwrap_or_default();
        let first_line = String::from_utf8_lossy(first_line).into_owned();
        let Some(focus) = key.line else {
            let lines = self.highlight(&key.path, head, rows, &first_line);
            return Content::Text {
                first: 1,
                focus: None,
                lines,
            };
        };
        let first = focus.saturating_sub(rows as u64 / 3).max(1);
        let window = match read_lines(&key.path, first, rows) {
            Ok(w) => w,
            Err(e) => return Content::Error(e.to_string()),
        };
        // Highlighting starts fresh at the window, which is close enough for
        // a preview and doesn't need the whole file parsed.
        let lines = self.highlight(&key.path, &window, rows, &first_line);
        Content::Text {
            first,
            focus: Some(focus),
            lines,
        }
    }

    fn highlight(
        &mut self,
        path: &Path,
        bytes: &[u8],
        rows: usize,
        first: &str,
    ) -> Vec<Line<'static>> {
        let (ss, theme) = self.syntax.get_or_insert_with(|| {
            let theme =
                ThemeSet::load_from_reader(&mut Cursor::new(THEME)).expect("bundled theme parses");
            (SyntaxSet::load_defaults_newlines(), theme)
        });
        let text = String::from_utf8_lossy(bytes);
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

/// `rows` lines starting at 1-based line `first`, as bytes.
fn read_lines(path: &Path, first: u64, rows: usize) -> std::io::Result<Vec<u8>> {
    let mut reader = std::io::BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    for _ in 1..first {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
    }
    let mut out = Vec::new();
    for _ in 0..rows {
        if reader.read_until(b'\n', &mut out)? == 0 {
            break;
        }
    }
    Ok(out)
}

/// Rows the facts take under a media preview, a blank row included.
pub fn facts_rows(n: usize) -> u16 {
    if n == 0 { 0 } else { n as u16 + 1 }
}

/// The cells left for the picture above `n` facts.
fn image_cells((w, h): (u16, u16), n: usize) -> Size {
    Size::new(w, h.saturating_sub(facts_rows(n)))
}

/// The longest side in pixels for an image filling `cells`, so it's
/// rendered sharp but no bigger than needed.
fn pixels(picker: &Picker, cells: Size) -> u32 {
    let font = picker.font_size();
    let w = u32::from(cells.width) * u32::from(font.width);
    let h = u32::from(cells.height) * u32::from(font.height);
    w.max(h).clamp(200, 2000)
}

fn protocol_from_png(picker: &Picker, png: &[u8], size: Size) -> Result<Protocol, ToolError> {
    if size.height == 0 || size.width == 0 {
        return Err(ToolError::Failed("no room".into()));
    }
    let img = image::load_from_memory(png).map_err(|e| ToolError::Failed(e.to_string()))?;
    picker
        .new_protocol(img, size, Resize::Fit(None))
        .map_err(|e| ToolError::Failed(e.to_string()))
}

/// A media preview, unless there's nothing to show at all.
fn with_facts(
    image: Option<Protocol>,
    facts: Vec<(&'static str, String)>,
) -> Result<Content, ToolError> {
    if image.is_none() && facts.is_empty() {
        return Err(ToolError::Failed("no previewer".into()));
    }
    Ok(Content::Media { image, facts })
}

/// The first entries `bsdtar` lists, a row left for "…and more".
fn archive(key: &Key) -> Result<Content, ToolError> {
    let rows = (key.size.1 as usize).saturating_sub(1).max(1);
    let args = [OsStr::new("-tf"), key.path.as_os_str()];
    let (entries, more) = tool::first_lines("bsdtar", &args, rows, PROBE_TIMEOUT)?;
    Ok(Content::Archive { entries, more })
}

/// `ffprobe -of compact` output: one line per section, `name|k=v|k=v`,
/// with `|` inside values escaped as `\|`.
#[derive(Debug, Default)]
struct Probe {
    format: HashMap<String, String>,
    streams: Vec<HashMap<String, String>>,
}

fn parse_probe(out: &str) -> Probe {
    let mut probe = Probe::default();
    for line in out.lines() {
        let mut fields = Vec::new();
        let mut cur = String::new();
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => cur.extend(chars.next()),
                '|' => fields.push(std::mem::take(&mut cur)),
                c => cur.push(c),
            }
        }
        fields.push(cur);
        let mut fields = fields.into_iter();
        let section = fields.next().unwrap_or_default();
        // Tag names vary in case between formats (TITLE in FLAC).
        let map: HashMap<String, String> = fields
            .filter_map(|f| {
                let (k, v) = f.split_once('=')?;
                Some((k.to_lowercase(), v.to_string()))
            })
            .collect();
        match section.as_str() {
            "format" => probe.format = map,
            "stream" => probe.streams.push(map),
            _ => {}
        }
    }
    probe
}

/// `3:05` or `1:02:03`.
fn clock(secs: f64) -> String {
    let s = secs.round() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn duration(p: &Probe) -> Option<String> {
    p.format
        .get("duration")
        .and_then(|d| d.parse::<f64>().ok())
        .map(clock)
}

fn stream<'a>(p: &'a Probe, kind: &str) -> Option<&'a HashMap<String, String>> {
    p.streams
        .iter()
        .find(|s| s.get("codec_type").map(String::as_str) == Some(kind))
}

fn video_facts(p: &Probe) -> Vec<(&'static str, String)> {
    let mut facts = Vec::new();
    if let Some(d) = duration(p) {
        facts.push(("Duration", d));
    }
    if let Some(v) = stream(p, "video")
        && let (Some(w), Some(h)) = (v.get("width"), v.get("height"))
    {
        facts.push(("Size", format!("{w}×{h}")));
    }
    let codecs: Vec<&str> = ["video", "audio"]
        .iter()
        .filter_map(|k| stream(p, k)?.get("codec_name").map(String::as_str))
        .collect();
    if !codecs.is_empty() {
        facts.push(("Codecs", codecs.join(" · ")));
    }
    facts
}

fn audio_facts(p: &Probe) -> Vec<(&'static str, String)> {
    let mut facts = Vec::new();
    for (label, tag) in [
        ("Title", "tag:title"),
        ("Artist", "tag:artist"),
        ("Album", "tag:album"),
        ("Date", "tag:date"),
        ("Track", "tag:track"),
        ("Genre", "tag:genre"),
    ] {
        if let Some(v) = p.format.get(tag).filter(|v| !v.trim().is_empty()) {
            facts.push((label, v.trim().to_string()));
        }
    }
    if let Some(d) = duration(p) {
        facts.push(("Duration", d));
    }
    if let Some(a) = stream(p, "audio") {
        let mut parts: Vec<String> = a.get("codec_name").cloned().into_iter().collect();
        if let Some(rate) = a.get("sample_rate").and_then(|r| r.parse::<f64>().ok()) {
            parts.push(format!("{} kHz", rate / 1000.0));
        }
        match a.get("channels").map(String::as_str) {
            Some("1") => parts.push("mono".into()),
            Some("2") => parts.push("stereo".into()),
            Some(n) => parts.push(format!("{n} channels")),
            None => {}
        }
        if !parts.is_empty() {
            facts.push(("Format", parts.join(" · ")));
        }
    }
    facts
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
            line: None,
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
        let Content::Text {
            lines,
            first: 1,
            focus: None,
        } = builder(None).build(&key(path.clone()))
        else {
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
        assert!(matches!(p.get(&k), Some(Content::Text { .. })));
        assert!(p.pending.is_none());
    }

    #[test]
    fn parses_ffprobe_output() {
        let out = "stream|codec_name=h264|codec_type=video|width=1920|height=1080\n\
                   stream|codec_name=flac|codec_type=audio|sample_rate=44100|channels=2\n\
                   format|duration=245.4|tag:TITLE=A \\| B|tag:artist=Band|tag:genre=\n";
        let p = parse_probe(out);
        assert_eq!(p.streams.len(), 2);
        assert_eq!(p.format["tag:title"], "A | B");
        assert_eq!(
            video_facts(&p),
            [
                ("Duration", "4:05".to_string()),
                ("Size", "1920×1080".to_string()),
                ("Codecs", "h264 · flac".to_string()),
            ]
        );
        assert_eq!(
            audio_facts(&p),
            [
                ("Title", "A | B".to_string()),
                ("Artist", "Band".to_string()),
                ("Duration", "4:05".to_string()),
                ("Format", "flac · 44.1 kHz · stereo".to_string()),
            ]
        );
        assert_eq!(clock(3723.4), "1:02:03");
        assert_eq!(clock(0.4), "0:00");
    }

    fn installed(program: &str) -> bool {
        std::process::Command::new("sh")
            .args(["-c", &format!("command -v {program}")])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn kitty_picker() -> Picker {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        picker
    }

    #[test]
    fn archives_list_their_first_entries() {
        if !installed("bsdtar") {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("arc/sub")).unwrap();
        for i in 0..20 {
            fs::write(r.join(format!("arc/sub/f{i:02}")), "x").unwrap();
        }
        let ok = std::process::Command::new("bsdtar")
            .args(["-czf", "a.tar.gz", "arc"])
            .current_dir(r)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let Content::Archive { entries, more } = builder(None).build(&key(r.join("a.tar.gz")))
        else {
            panic!("expected an archive listing");
        };
        // 10 rows: 9 entries and "…and more".
        assert_eq!(entries.len(), 9);
        assert_eq!(entries[0], "arc/");
        assert!(more);

        fs::write(r.join("fake.zip"), "not a zip").unwrap();
        let Content::Info { label, note, .. } = builder(None).build(&key(r.join("fake.zip")))
        else {
            panic!("expected info");
        };
        assert_eq!(label, "Archive");
        assert!(!note.is_empty());
    }

    #[test]
    fn audio_and_video_get_facts_and_pictures() {
        if !installed("ffmpeg") || !installed("ffprobe") {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let r = tmp.path();
        let ffmpeg = |args: &[&str]| {
            let ok = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y"])
                .args(args)
                .current_dir(r)
                .status()
                .unwrap()
                .success();
            assert!(ok, "ffmpeg {args:?}");
        };
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "sine=duration=3",
            "-metadata",
            "title=Song",
            "s.mp3",
        ]);
        let Content::Media { image, facts } = builder(None).build(&key(r.join("s.mp3"))) else {
            panic!("expected media");
        };
        assert!(image.is_none());
        assert_eq!(facts[0], ("Title", "Song".to_string()));
        assert!(facts.contains(&("Duration", "0:03".to_string())));

        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=320x240:rate=10",
            "v.mp4",
        ]);
        let Content::Media { image, facts } =
            builder(Some(kitty_picker())).build(&key(r.join("v.mp4")))
        else {
            panic!("expected media");
        };
        assert!(facts.contains(&("Size", "320×240".to_string())));
        assert_eq!(image.is_some(), installed("ffmpegthumbnailer"));
    }

    /// A one-page PDF; poppler rebuilds the missing cross-reference table.
    const PDF: &str = "%PDF-1.4\n\
        1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
        2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj\n\
        trailer << /Root 1 0 R >>\n%%EOF\n";

    #[test]
    fn pdfs_show_their_first_page() {
        if !installed("pdftoppm") || !installed("pdfinfo") {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("doc.pdf");
        fs::write(&pdf, PDF).unwrap();
        let Content::Media { image, facts } =
            builder(Some(kitty_picker())).build(&key(pdf.clone()))
        else {
            panic!("expected media");
        };
        assert!(image.is_some());
        assert_eq!(facts, [("Pages", "1".to_string())]);
        // Without kitty graphics, just the facts.
        let Content::Media { image, .. } = builder(None).build(&key(pdf)) else {
            panic!("expected media");
        };
        assert!(image.is_none());
    }

    #[test]
    fn text_around_a_line() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("long.txt");
        let src: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        fs::write(&path, src).unwrap();
        let mut k = key(path);
        k.line = Some(50);
        let Content::Text {
            first,
            focus,
            lines,
        } = builder(None).build(&k)
        else {
            panic!("expected text");
        };
        // A third of the 10 rows above the match.
        assert_eq!((first, focus), (47, Some(50)));
        assert_eq!(text_of(&lines[0]), "line 47");
        assert_eq!(text_of(&lines[3]), "line 50");
        assert_eq!(lines.len(), 10);

        k.line = Some(2);
        let Content::Text { first, lines, .. } = builder(None).build(&k) else {
            panic!("expected text");
        };
        assert_eq!((first, text_of(&lines[0])), (1, "line 1".to_string()));
        k.line = Some(99);
        let Content::Text { lines, .. } = builder(None).build(&k) else {
            panic!("expected text");
        };
        assert_eq!(text_of(lines.last().unwrap()), "line 100");
    }
}
