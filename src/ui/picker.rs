//! The file picker (`f`) and content search (`F`): a big centred box with
//! the query on top, results on the left and a preview on the right.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Widget};

use super::{input_line, list::window_start, preview, put};
use crate::app::App;
use crate::grep::ContentSearch;
use crate::picker::FilePicker;
use crate::text_input::TextInput;
use crate::theme::*;

struct Layout {
    outer: Rect,
    input: Rect,
    results: Rect,
    preview: Rect,
}

/// About 90% wide and 80% tall, centred.
fn layout(area: Rect) -> Option<Layout> {
    let w = (area.width * 9 / 10).max(40.min(area.width));
    let h = (area.height * 8 / 10).max(10.min(area.height));
    let outer = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    // Inside the border: two columns of padding at the sides.
    let inner = Rect {
        x: outer.x + 2,
        y: outer.y + 1,
        width: outer.width.checked_sub(4)?,
        height: outer.height.checked_sub(2)?,
    };
    if inner.height < 3 || inner.width < 10 {
        return None;
    }
    let input = Rect { height: 1, ..inner };
    let body = Rect {
        y: inner.y + 2,
        height: inner.height - 2,
        ..inner
    };
    let left = body.width / 2;
    let results = Rect {
        width: left,
        ..body
    };
    let preview = Rect {
        x: body.x + left + 2,
        width: body.width.saturating_sub(left + 2),
        ..body
    };
    Some(Layout {
        outer,
        input,
        results,
        preview,
    })
}

/// The size of the picker's preview, for the preview worker.
pub fn preview_size(area: Rect) -> Option<(u16, u16)> {
    layout(area)
        .map(|l| (l.preview.width, l.preview.height))
        .filter(|&(w, h)| w > 0 && h > 0)
}

/// The box, its titles, the query line, the divider and the preview.
fn frame(
    app: &App,
    buf: &mut Buffer,
    area: Rect,
    (title, right): (&str, &str),
    input: &TextInput,
) -> Option<Layout> {
    let l = layout(area)?;
    Clear.render(l.outer, buf);
    Block::bordered()
        .border_style(Style::default().fg(POPUP_BORDER))
        .style(Style::default().bg(POPUP_BG).fg(TEXT))
        .title(Line::from(format!(" {title} ")).left_aligned())
        .title(
            Line::from(format!(" {right} "))
                .right_aligned()
                .style(Style::default().fg(POPUP_DIM)),
        )
        .title_bottom(
            Line::from(" Enter go to file · Shift-Enter edit in Helix ")
                .right_aligned()
                .style(Style::default().fg(POPUP_DIM)),
        )
        .render(l.outer, buf);
    put(
        buf,
        l.input.x,
        l.input.y,
        "›",
        1,
        Style::default().fg(ACCENT),
    );
    let w = l.input.width.saturating_sub(2);
    input_line(
        buf,
        l.input.x + 2,
        l.input.y,
        w,
        input,
        Style::default().fg(TEXT),
    );
    let x = l.results.right();
    for y in l.results.top()..l.results.bottom() {
        buf[(x, y)]
            .set_symbol("│")
            .set_style(Style::default().fg(OVERLAY0));
    }
    preview::body(app, buf, l.preview);
    Some(l)
}

/// One result row: the cursor bar, then `parts` of text, each with the char
/// indices to highlight. Returns nothing; cuts at the row's end.
fn row(buf: &mut Buffer, r: Rect, on: bool, parts: &[(&str, Color, &[u32])]) {
    if on {
        buf.set_style(r, Style::default().bg(ACCENT));
    }
    let mut col = r.x;
    for &(text, fg, marks) in parts {
        for (i, c) in text.chars().enumerate() {
            if col >= r.right() {
                return;
            }
            let marked = marks.binary_search(&(i as u32)).is_ok();
            let style = match (on, marked) {
                (true, m) => {
                    let s = Style::default().fg(ON_ACCENT);
                    if m { s.add_modifier(Modifier::BOLD) } else { s }
                }
                (false, true) => Style::default().fg(PEACH).add_modifier(Modifier::BOLD),
                (false, false) => Style::default().fg(fg),
            };
            let mut s = [0; 4];
            col += put(buf, col, r.y, c.encode_utf8(&mut s), r.right() - col, style);
        }
    }
}

fn empty(buf: &mut Buffer, r: Rect, text: &str) {
    put(buf, r.x, r.y, text, r.width, Style::default().fg(POPUP_DIM));
}

pub fn render(app: &App, buf: &mut Buffer, area: Rect, p: &FilePicker) {
    let (matched, total) = p.counts();
    let title = format!("Files · {}", app.display_path(&p.root));
    let counts = format!("{matched}/{total}");
    let Some(l) = frame(app, buf, area, (&title, &counts), &p.input) else {
        return;
    };
    let rows = l.results.height as usize;
    if matched == 0 {
        empty(
            buf,
            l.results,
            if total == 0 {
                "looking…"
            } else {
                "no files match"
            },
        );
    }
    let start = window_start(p.cursor, rows, matched);
    for (n, hit) in p.hits(start, rows).into_iter().enumerate() {
        let r = Rect {
            y: l.results.y + n as u16,
            height: 1,
            ..l.results
        };
        row(
            buf,
            r,
            start + n == p.cursor,
            &[(&hit.path, TEXT, &hit.matched)],
        );
    }
}

pub fn render_search(app: &App, buf: &mut Buffer, area: Rect, s: &ContentSearch) {
    let (matched, total) = s.counts();
    let title = format!("Search · {}", app.display_path(&s.root));
    let counts = if s.searching() {
        format!("{matched}/{total} …")
    } else {
        format!("{matched}/{total}")
    };
    let Some(l) = frame(app, buf, area, (&title, &counts), &s.input) else {
        return;
    };
    if s.input.text.is_empty() {
        empty(buf, l.results, "type to search · %p filters paths");
        return;
    }
    if matched == 0 {
        empty(
            buf,
            l.results,
            if s.searching() {
                "searching…"
            } else {
                "no matches"
            },
        );
    }
    let rows = l.results.height as usize;
    let start = window_start(s.cursor, rows, matched);
    s.with_hits(start, rows, |hits| {
        for (n, hit) in hits.iter().enumerate() {
            let r = Rect {
                y: l.results.y + n as u16,
                height: 1,
                ..l.results
            };
            let place = format!(":{}", hit.line.line);
            let parts: &[(&str, Color, &[u32])] = &[
                (&hit.line.path, TEXT, &hit.path_matched),
                (&place, POPUP_DIM, &[]),
            ];
            row(buf, r, start + n == s.cursor, parts);
        }
    });
}
