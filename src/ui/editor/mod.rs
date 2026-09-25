//! Code editor: line-number gutter + syntax highlight + selection + cursor.
//!
//! Submodules: `overlays` (diagnostic / selection / find cell overlays),
//! `commit_view` (non-code rows of a commit's diff view), `scrollbar`, and
//! `hscrollbar` (horizontal viewport control), and `diagnostics` (severity
//! glyph / color / rank).

mod commit_view;
mod diagnostics;
mod hscrollbar;
mod overlays;
mod scrollbar;

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::buffer::Buffer as CellBuffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::model::{Diagnostic, DiffRow, Focus, Model};
use crate::core::buffer::Buffer;
use crate::services::git::CommitRow;
use crate::services::git::GutterKind;
use crate::services::lsp::Severity;

use commit_view::commit_row_line;
use diagnostics::severity_rank;
use overlays::{overlay_diagnostics, overlay_find_matches, overlay_selection};

pub use diagnostics::{severity_color, severity_icon};
pub(crate) use hscrollbar::{horizontal_scroll_metrics, render_hscrollbar};
pub use scrollbar::render_scrollbar;

/// Widest source line shown by the editor, including removed lines woven into
/// a diff tab. The horizontal scrollbar and its input mapping share this value.
pub(crate) fn horizontal_content_len(model: &Model) -> usize {
    let real = model.active_buffer().map_or(0, Buffer::max_line_len);
    let deleted = model
        .active_deleted
        .iter()
        .flat_map(|(_, lines)| lines)
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    real.max(deleted)
}

/// Placeholder cell for characters that must not reach the terminal raw.
const PLACEHOLDER: char = '\u{FFFD}';

/// Maps a buffer character to the single cell drawn for it. Tabs become a space;
/// control characters and bidi embedding/override/isolate marks become a visible
/// placeholder so they neither move the terminal cursor nor reorder the line
/// ("Trojan Source"). Keeps the editor's 1 char = 1 cell invariant that the
/// cursor, selection and find overlays rely on.
pub(super) fn display_char(c: char) -> char {
    match c {
        '\t' => ' ',
        '\u{061C}'
        | '\u{200E}'
        | '\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' => PLACEHOLDER,
        c if c.is_control() => PLACEHOLDER,
        c => c,
    }
}

/// What the editor area shows this frame: geometry, horizontal scroll, and a
/// precomputed buffer-line -> screen-row map for the visible window, built once
/// per render (O(height)) so overlays never scan the whole display list.
pub(super) struct Viewport {
    area: Rect,
    gutter_w: u16,
    scroll_x: usize,
    text_w: usize,
    /// First buffer line with a row on screen.
    first_line: usize,
    /// `line_row[l - first_line]` = row offset of buffer line `l` from `area.y`.
    line_row: Vec<Option<u16>>,
}

impl Viewport {
    fn new(
        area: Rect,
        gutter_w: u16,
        buf: &Buffer,
        display: &[DiffRow],
        disp_start: usize,
    ) -> Self {
        let height = area.height as usize;
        let window = display.get(disp_start..).unwrap_or(&[]);
        let mut first_line: Option<usize> = None;
        let mut line_row: Vec<Option<u16>> = Vec::with_capacity(height);
        for (i, r) in window.iter().take(height).enumerate() {
            if let DiffRow::Real(l) = r {
                let first = *first_line.get_or_insert(*l);
                let Some(k) = l.checked_sub(first) else {
                    continue;
                };
                if k >= line_row.len() {
                    line_row.resize(k + 1, None);
                }
                // `i < height <= u16::MAX`, so the cast is lossless.
                line_row[k] = Some(i as u16);
            }
        }
        Viewport {
            area,
            gutter_w,
            scroll_x: buf.scroll_x,
            text_w: area.width.saturating_sub(gutter_w) as usize,
            first_line: first_line.unwrap_or(0),
            line_row,
        }
    }

    /// Inclusive `(first, last)` buffer lines on screen, or `None` if none are.
    fn visible_lines(&self) -> Option<(usize, usize)> {
        (!self.line_row.is_empty())
            .then(|| (self.first_line, self.first_line + self.line_row.len() - 1))
    }

    /// Absolute screen row of buffer line `line`, or `None` when it is off-screen.
    fn row_y(&self, line: usize) -> Option<u16> {
        let k = line.checked_sub(self.first_line)?;
        let off = (*self.line_row.get(k)?)?;
        Some(self.area.y + off)
    }

    /// Absolute screen column of char column `col`, or `None` when it is
    /// scrolled off horizontally.
    fn col_x(&self, col: usize) -> Option<u16> {
        let vis = col.checked_sub(self.scroll_x)?;
        // `vis < text_w <= u16::MAX`, so the cast is lossless.
        (vis < self.text_w).then(|| self.area.x + self.gutter_w + vis as u16)
    }

    /// Applies `style` to char columns `[from, to)` of screen row `y`, clipped to
    /// the horizontal scroll window (never iterates columns that are off-screen).
    fn paint_cols(&self, cells: &mut CellBuffer, y: u16, from: usize, to: usize, style: Style) {
        let from = from.max(self.scroll_x);
        let to = to.min(self.scroll_x + self.text_w);
        for col in from..to {
            if let Some(x) = self.col_x(col)
                && let Some(cell) = cells.cell_mut((x, y))
            {
                cell.set_style(style);
            }
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, model: &Model, gutter_w: u16) {
    frame.render_widget(
        Paragraph::new("").style(Style::new().bg(model.theme.bg)),
        area,
    );

    // A binary / unreadable file opens as a read-only tab: show the error
    // message centered in the editor area instead of an (empty) buffer.
    if let Some(notice) = model.active_notice() {
        render_notice(frame, area, model, notice);
        return;
    }

    let Some(buf) = model.active_buffer() else {
        let hint = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Select a file from the tree on the left to open it (Enter).",
                Style::new().fg(model.theme.fg_dim),
            )),
        ])
        .style(Style::new().bg(model.theme.bg));
        frame.render_widget(hint, area);
        return;
    };

    let height = area.height as usize;
    let text_w = area.width.saturating_sub(gutter_w) as usize;
    let top = buf.scroll_y;
    let scroll_x = buf.scroll_x;

    let selection = buf.selection_range();
    // Changed-line backgrounds are only drawn for diff-mode tabs (opened from Git).
    let diff_bg = model.active_is_diff();
    let git_on = model.git_gutter();

    // Visual rows: real buffer lines, with removed lines woven in for diff tabs.
    let display = model.diff_rows();
    let disp_start = model.diff_start(display, top);
    let vp = Viewport::new(area, gutter_w, buf, display, disp_start);

    // Diagnostics for this file (empty for files with no language server).
    let diags: &[Diagnostic] = buf
        .path
        .as_ref()
        .and_then(|p| model.diagnostics.get(p))
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    // Most-severe diagnostic per line: colors the line number, and (when
    // `inline_diagnostics` is on) its message trails the line.
    // Only lines on screen matter, so off-screen diagnostics are skipped.
    let mut best_by_line: HashMap<usize, &Diagnostic> = HashMap::new();
    for d in diags.iter().filter(|d| vp.row_y(d.line).is_some()) {
        best_by_line
            .entry(d.line)
            .and_modify(|cur| {
                if severity_rank(d.severity) < severity_rank(cur.severity) {
                    *cur = d;
                }
            })
            .or_insert(d);
    }

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    for i in 0..height {
        match display.get(disp_start + i) {
            None => lines.push(Line::from("")),
            Some(DiffRow::Real(row)) => {
                let row = *row;
                // A commit's diff view has rows that are not code: the header at
                // the top, a heading per file, and the gaps between hunks.
                if let Some(kind) = model.commit_row(row)
                    && !matches!(kind, CommitRow::Line(_))
                {
                    lines.push(commit_row_line(
                        model,
                        kind,
                        &buf.line_text(row),
                        gutter_w,
                        area.width as usize,
                    ));
                    continue;
                }
                let is_cursor_line = row == buf.cursor.line;
                // A diagnostic on this line recolors its line number by severity.
                let ln_style = if let Some(d) = best_by_line.get(&row) {
                    Style::new().fg(severity_color(&model.theme, d.severity))
                } else if is_cursor_line {
                    Style::new().fg(model.theme.fg)
                } else {
                    Style::new().fg(model.theme.line_number)
                };
                let mut spans: Vec<Span> = Vec::new();
                // Git change marker column (leftmost), when the file is tracked.
                let mark = if git_on {
                    model.active_git_marks.get(&row).copied()
                } else {
                    None
                };
                if git_on {
                    let (ch, color) = match mark {
                        Some(GutterKind::Added) => (
                            if model.ascii_icons { "|" } else { "▍" },
                            model.theme.git_added,
                        ),
                        Some(GutterKind::Deleted) => (
                            if model.ascii_icons { "_" } else { "▁" },
                            model.theme.git_deleted,
                        ),
                        None => (" ", model.theme.bg),
                    };
                    spans.push(Span::styled(ch.to_string(), Style::new().fg(color)));
                }
                let num_w = (gutter_w as usize).saturating_sub(if git_on { 2 } else { 1 });
                // A commit's diff view numbers its code by the file's own lines,
                // not by the rows of the view.
                let number = match model.commit_row(row) {
                    Some(CommitRow::Line(n)) => n,
                    _ => row + 1,
                };
                let gutter = format!("{number:>num_w$} ");
                spans.push(Span::styled(gutter, ln_style));

                // Highlighted text pieces (clipped by scroll_x).
                let hl_line = model.hl_line(row);
                append_text_spans(&mut spans, hl_line, buf, row, scroll_x, text_w, model);

                // Inline diagnostics: trail the line with the most-severe
                // error/warning message, colored by severity (red/yellow).
                if model.sidebar.settings.inline_diagnostics
                    && let Some(d) = best_by_line.get(&row)
                    && matches!(d.severity, Severity::Error | Severity::Warning)
                {
                    let msg: String = d
                        .message
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .map(display_char)
                        .collect();
                    if !msg.is_empty() {
                        // Only the message gets the severity background; the gap
                        // before it stays on the editor background. The bg is a
                        // translucent tint (like git diff rows), text stays bright.
                        // A leading icon marks the severity (error / warning).
                        let color = severity_color(&model.theme, d.severity);
                        let icon = severity_icon(d.severity, model.ascii_icons);
                        spans.push(Span::raw("  "));
                        spans.push(Span::styled(
                            format!(" {icon} {msg} "),
                            Style::new().fg(color).bg(model.theme.tint(color, 0.22)),
                        ));
                    }
                }

                let mut line = Line::from(spans);
                // Only diff-mode changed lines get a background; the cursor line is not filled.
                let bg = match mark {
                    Some(GutterKind::Added) if diff_bg => Some(model.theme.diff_add_bg),
                    Some(GutterKind::Deleted) if diff_bg => Some(model.theme.diff_del_bg),
                    _ => None,
                };
                if let Some(bg) = bg {
                    line = line.style(Style::new().bg(bg));
                }
                lines.push(line);
            }
            Some(DiffRow::Deleted(text)) => {
                lines.push(deleted_row(model, text, gutter_w, git_on, scroll_x, text_w));
            }
        }
    }

    let p = Paragraph::new(lines).style(Style::new().bg(model.theme.bg));
    frame.render_widget(p, area);

    // Overlay diagnostic squiggles, then the selection background per cell.
    if !diags.is_empty() {
        overlay_diagnostics(frame, &vp, &model.theme, diags);
    }
    if let Some((start, end)) = selection {
        overlay_selection(frame, &vp, model, buf, start, end);
    }
    // Find matches paint over the selection so the active match's color wins.
    // The Search panel's query is highlighted the same way while the panel is
    // shown (the in-editor find widget wins when both are active).
    if model.find.open && !model.find.matches.is_empty() {
        let (m, cur) = (&model.find.matches, model.find.current);
        overlay_find_matches(frame, &vp, model, buf, m, cur);
    } else if !model.search_marks.is_empty() {
        let (m, cur) = (&model.search_marks, model.current_search_mark());
        overlay_find_matches(frame, &vp, model, buf, m, cur);
    }

    // Draw our own block cursor (only when the editor is focused). A manual
    // block keeps the caret always white instead of the native terminal cursor,
    // which reverse-videos the cell and vanishes on gray comment text.
    if model.focus == Focus::Editor
        && let Some((cx, cy)) = cursor_screen_pos(model, area, gutter_w)
        && let Some(cell) = frame.buffer_mut().cell_mut((cx, cy))
    {
        cell.set_style(Style::new().bg(Color::White).fg(model.theme.bg));
    }
}

/// Renders a read-only notice (binary / unreadable file) centered in the editor.
fn render_notice(frame: &mut Frame, area: Rect, model: &Model, notice: &str) {
    frame.render_widget(
        Paragraph::new("").style(Style::new().bg(model.theme.bg)),
        area,
    );
    if area.height == 0 || area.width < 4 {
        return;
    }
    // Wrap width the message is laid out in (used to estimate its height).
    let wrap_w = area.width.saturating_sub(4).clamp(1, 70) as usize;
    let est_lines: u16 = notice
        .split('\n')
        .map(|para| {
            let len = para.chars().count();
            (len.div_ceil(wrap_w)).max(1) as u16
        })
        .sum();
    // Vertical centering: pad the top so the block sits in the middle.
    let top_pad = area.height.saturating_sub(est_lines) / 2;
    let mut lines: Vec<Line> = Vec::new();
    for _ in 0..top_pad {
        lines.push(Line::from(""));
    }
    for para in notice.split('\n') {
        lines.push(Line::from(Span::styled(
            para.to_string(),
            Style::new().fg(model.theme.fg_dim),
        )));
    }
    let p = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .style(Style::new().bg(model.theme.bg));
    frame.render_widget(p, area);
}

/// Screen cell of the active buffer's cursor within the editor area, or `None`
/// when it is scrolled off. Shared by the caret and the completion popup so they
/// never disagree (the same discipline as `compute_areas`).
pub fn cursor_screen_pos(model: &Model, area: Rect, gutter_w: u16) -> Option<(u16, u16)> {
    let buf = model.active_buffer()?;
    let display = model.diff_rows();
    let disp_start = model.diff_start(display, buf.scroll_y);
    // Row and column are bounds-checked in `usize` before any `u16` cast, so a
    // cursor far below / right of the viewport can never overflow.
    let vp = Viewport::new(area, gutter_w, buf, display, disp_start);
    let cy = vp.row_y(buf.cursor.line)?;
    let cx = vp.col_x(buf.cursor.col)?;
    Some((cx, cy))
}

/// A removed (red) diff row: blank line-number gutter, `-` change marker, and the
/// removed source text on the deletion background.
fn deleted_row(
    model: &Model,
    text: &str,
    gutter_w: u16,
    git_on: bool,
    scroll_x: usize,
    text_w: usize,
) -> Line<'static> {
    let th = &model.theme;
    let mut spans: Vec<Span> = Vec::new();
    if git_on {
        spans.push(Span::styled(
            (if model.ascii_icons { "-" } else { "▁" }).to_string(),
            Style::new().fg(th.git_deleted),
        ));
    }
    // Empty line-number column (the removed line has no number in the new file).
    let num_w = (gutter_w as usize).saturating_sub(if git_on { 2 } else { 1 });
    spans.push(Span::styled(
        format!("{:>num_w$} ", "-"),
        Style::new().fg(th.git_deleted),
    ));
    // Removed text, clipped to the horizontal scroll window — one span for the
    // whole visible slice, not one per character.
    let visible: String = text
        .trim_end_matches(['\n', '\r'])
        .chars()
        .skip(scroll_x)
        .take(text_w)
        .map(display_char)
        .collect();
    if !visible.is_empty() {
        spans.push(Span::styled(visible, Style::new().fg(th.fg)));
    }
    Line::from(spans).style(Style::new().bg(th.diff_del_bg))
}

/// Appends highlight pieces to spans within the scroll_x/width window.
fn append_text_spans(
    spans: &mut Vec<Span<'static>>,
    hl_line: Option<&crate::core::highlight::HlLine>,
    buf: &Buffer,
    row: usize,
    scroll_x: usize,
    width: usize,
    model: &Model,
) {
    if width == 0 {
        return;
    }
    let highlighted = hl_line.is_some_and(|p| !p.is_empty());
    // The plain-text path starts reading at `scroll_x` (see below).
    let plain_len = if highlighted { 0 } else { buf.line_len(row) };
    let plain_from = scroll_x.min(plain_len);
    let mut col = if highlighted { 0 } else { plain_from }; // source character column
    let mut taken = 0usize; // visible column

    // Collect the whole visible slice of this piece into one span instead of
    // one span per character — a full line was allocating a String + Span per
    // glyph every frame, the dominant render cost on long lines.
    let mut push_piece =
        |spans: &mut Vec<Span<'static>>, text: &mut dyn Iterator<Item = char>, color: Color| {
            let mut visible = String::new();
            for ch in text {
                if taken >= width {
                    break;
                }
                if col >= scroll_x {
                    visible.push(display_char(ch));
                    taken += 1;
                }
                col += 1;
            }
            if !visible.is_empty() {
                spans.push(Span::styled(visible, Style::new().fg(color)));
            }
            taken < width
        };

    match hl_line {
        Some(pieces) if highlighted => {
            for (color, text) in pieces {
                if !push_piece(spans, &mut text.chars(), *color) {
                    break;
                }
            }
        }
        _ => {
            // Plain text when there is no highlight: read only the visible
            // window straight out of the rope (no per-frame copy of the line).
            if row >= buf.rope.len_lines() {
                return;
            }
            let slice = buf.rope.line(row).slice(plain_from..plain_len);
            push_piece(spans, &mut slice.chars().take(width), model.theme.fg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PLACEHOLDER, display_char};

    #[test]
    fn control_and_bidi_chars_render_as_one_visible_cell() {
        assert_eq!(display_char('\t'), ' ');
        assert_eq!(display_char('\u{1b}'), PLACEHOLDER); // ESC
        assert_eq!(display_char('\u{7f}'), PLACEHOLDER); // DEL
        assert_eq!(display_char('\u{202E}'), PLACEHOLDER); // RLO
        assert_eq!(display_char('\u{2066}'), PLACEHOLDER); // LRI
        assert_eq!(display_char('a'), 'a');
        assert_eq!(display_char('é'), 'é');
    }
}
