//! Editor scrollbars: the vertical one (draggable thumb plus proportional git
//! change marks) and the horizontal one under the text.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::{DiffRow, Model};
use crate::services::git::GutterKind;

/// Renders the editor scrollbar (rightmost column): a draggable thumb plus git
/// change marks (green line = addition, red line = deletion) at proportional rows.
/// In a diff tab with woven deletions the track spans the *display* rows, so the
/// thumb and marks line up with what the editor actually shows.
pub fn render_scrollbar(frame: &mut Frame, area: Rect, model: &Model) {
    let th = &model.theme;
    let h = area.height as usize;
    if h == 0 {
        return;
    }
    let Some(buf) = model.active_buffer() else {
        frame.render_widget(Paragraph::new("").style(Style::new().bg(th.bg_alt)), area);
        return;
    };
    let woven = model.has_inline_deletions();
    let display = model.diff_rows();
    let (n, top) = if woven {
        (
            display.len().max(1),
            model.diff_start(display, buf.scroll_y),
        )
    } else {
        (buf.line_count().max(1), buf.scroll_y)
    };

    // Thumb: the currently visible portion of the file.
    let (thumb_start, thumb_end) = if n > h {
        let ts = top * h / n;
        let tl = (h * h / n).max(1);
        (ts, (ts + tl).min(h))
    } else {
        (0, h)
    };

    // Project git change marks onto scrollbar rows.
    let mut mark_rows: HashMap<usize, GutterKind> = HashMap::new();
    let mut put = |pos: usize, kind: GutterKind| {
        let row = (pos * h / n).min(h - 1);
        // Deletions win over additions on a shared row so removals stay visible.
        mark_rows
            .entry(row)
            .and_modify(|k| {
                if kind == GutterKind::Deleted {
                    *k = kind;
                }
            })
            .or_insert(kind);
    };
    if model.git_gutter() && !model.active_git_marks.is_empty() {
        if woven {
            // One pass over the display rows maps each marked line to its row.
            for (i, r) in display.iter().enumerate() {
                if let DiffRow::Real(l) = r
                    && let Some(&kind) = model.active_git_marks.get(l)
                {
                    put(i, kind);
                }
            }
        } else {
            for (&ln, &kind) in &model.active_git_marks {
                put(ln, kind);
            }
        }
    }

    let dash = if model.ascii_icons { "-" } else { "─" };
    let mut lines: Vec<Line> = Vec::with_capacity(h);
    for y in 0..h {
        let in_thumb = y >= thumb_start && y < thumb_end;
        let track_bg = if in_thumb { th.fg_dim } else { th.bg_alt };
        let span = match mark_rows.get(&y) {
            Some(GutterKind::Added) => {
                Span::styled(dash, Style::new().fg(th.git_added).bg(track_bg))
            }
            Some(GutterKind::Deleted) => {
                Span::styled(dash, Style::new().fg(th.git_deleted).bg(track_bg))
            }
            None => Span::styled(" ", Style::new().bg(track_bg)),
        };
        lines.push(Line::from(span));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// Total columns the horizontal scrollbar spans for a `w`-cell text area: the
/// longest line plus the caret cell past its end, and never less than the
/// current view reaches. Shared with the mouse mapping (`update::mouse`).
pub fn hscroll_extent(model: &Model, w: usize) -> usize {
    let scroll_x = model.active_buffer().map_or(0, |b| b.scroll_x);
    (model.active_max_cols + 1).max(scroll_x + w).max(1)
}

/// Renders the horizontal scrollbar (row under the editor text), styled like the
/// vertical one: the thumb covers the visible columns of the longest line.
pub fn render_hscrollbar(frame: &mut Frame, area: Rect, model: &Model) {
    let th = &model.theme;
    let w = area.width as usize;
    if w == 0 {
        return;
    }
    let scroll_x = model.active_buffer().map_or(0, |b| b.scroll_x);
    let n = hscroll_extent(model, w);
    let start = scroll_x * w / n;
    let end = (start + (w * w / n).max(1)).min(w);
    let spans: Vec<Span> = (0..w)
        .map(|x| {
            let bg = if (start..end).contains(&x) {
                th.fg_dim
            } else {
                th.bg_alt
            };
            Span::styled(" ", Style::new().bg(bg))
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
