//! File-tree panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::Model;

use super::{content_rect, list_scroll, panel_area};

/// Columns reserved at the right edge of a directory row for the
/// "new file" / "new folder" buttons: `[icon][space][space][icon][space]`.
const ACTION_COLS: usize = 5;
/// Non-ASCII rows use two disclosure cells, a file icon and a separating space.
const ROW_PREFIX_WIDTH: usize = 4;
/// Minimum width that can hold the prefix, one name cell, and action buttons.
const MIN_ACTION_WIDTH: usize = ROW_PREFIX_WIDTH + 1 + ACTION_COLS;

/// Whether a directory row at tree `depth` has room for the right-edge
/// new-file / new-folder buttons: indent + tree prefix + at least one name
/// cell + the buttons. Shared by `render` and `file_hit`, so a button that is
/// not drawn (deep indent, narrow sidebar) is never clickable either.
fn row_prefix_width(ascii_icons: bool) -> usize {
    if ascii_icons { 2 } else { ROW_PREFIX_WIDTH }
}

fn dir_buttons_fit(width: usize, depth: usize, ascii_icons: bool) -> bool {
    let prefix_width = row_prefix_width(ascii_icons);
    width >= prefix_width + 1 + ACTION_COLS && width >= 2 * depth + prefix_width + 1 + ACTION_COLS
}

/// What a click in the file tree landed on.
pub enum FileHit {
    /// A tree row (open the file / toggle the directory).
    Row(usize),
    /// The "new file" button on the directory row at this index.
    NewFile(usize),
    /// The "new folder" button on the directory row at this index.
    NewFolder(usize),
    /// The "new file" button in the panel header (create in the workspace root).
    NewFileRoot,
    /// The "new folder" button in the panel header (create in the workspace root).
    NewFolderRoot,
}

/// (new file, new folder) button glyphs — codicons, or plus signs in ASCII mode.
fn action_icons(model: &Model) -> (&'static str, &'static str) {
    if model.ascii_icons {
        ("+", "+")
    } else {
        ("\u{ea7f}", "\u{ea80}") // new-file, new-folder
    }
}

pub(super) fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let rows = model.sidebar.files.visible_rows();
    let height = area.height as usize;
    let offset = list_scroll(model.sidebar.files.selected, rows.len(), height);

    // Path of the file open in the active tab, to mark its row in the tree.
    let active_path = model.active_buffer().and_then(|b| b.path.clone());

    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(offset).take(height) {
        let selected = i == model.sidebar.files.selected;
        let is_active = !row.is_dir && active_path.as_deref() == Some(row.path.as_path());
        let indent = "  ".repeat(row.depth);
        let disclosure = if row.is_dir {
            if row.expanded { "▾ " } else { "▸ " }
        } else {
            "  "
        };
        let name_style = if row.is_dir {
            Style::new().fg(model.theme.fg)
        } else {
            Style::new().fg(model.theme.fg_dim)
        };
        // Selected row and the open file both get the darkened selection bg.
        let line_style = if selected {
            Style::new().bg(model.theme.selected_bg())
        } else if is_active {
            Style::new()
                .fg(model.theme.fg)
                .bg(model.theme.selected_bg())
        } else {
            Style::new().bg(model.theme.bg_alt)
        };
        let mut spans = vec![
            Span::raw(indent.clone()),
            Span::styled(disclosure, Style::new().fg(model.theme.fg_dim)),
        ];
        if !model.ascii_icons {
            let file_icon = crate::core::icons::file(&row.name, row.is_dir, row.expanded);
            spans.push(Span::styled(file_icon, Style::new().fg(model.theme.fg_dim)));
            spans.push(Span::raw(" "));
        }
        // Directories get "new file" / "new folder" buttons pinned to the right edge.
        let width = area.width as usize;
        if row.is_dir && dir_buttons_fit(width, row.depth, model.ascii_icons) {
            let avail = width
                .saturating_sub(indent.len() + row_prefix_width(model.ascii_icons) + ACTION_COLS);
            let (new_file, new_folder) = action_icons(model);
            spans.push(Span::styled(
                format!("{:<avail$}", fit_name(&row.name, avail)),
                name_style,
            ));
            spans.push(Span::styled(new_file, Style::new().fg(model.theme.fg_dim)));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                new_folder,
                Style::new().fg(model.theme.fg_dim),
            ));
            spans.push(Span::raw(" "));
        } else {
            spans.push(Span::styled(row.name.clone(), name_style));
        }
        lines.push(Line::from(spans).style(line_style));
    }
    let p = Paragraph::new(lines).style(Style::new().bg(model.theme.bg_alt));
    frame.render_widget(p, area);
}

/// The workspace-root row of the Files panel: the row directly below the
/// "EXPLORER" title. Holds the root directory name and the root "new file"/"new
/// folder" buttons.
fn header_row(area: Rect) -> Rect {
    let inner = content_rect(area);
    Rect {
        y: inner.y + 1,
        height: 1,
        ..inner
    }
}

/// Draws the workspace-root directory name and its "new file"/"new folder"
/// buttons on the row directly below the "EXPLORER" title. The buttons are
/// pinned to the right edge: `[file][ ][folder][ ]`.
pub(super) fn render_header_actions(frame: &mut Frame, area: Rect, model: &Model) {
    let row = header_row(area);
    if (row.width as usize) < MIN_ACTION_WIDTH {
        return;
    }
    // Root directory name, filling the row (buttons are drawn on top, at right).
    let dir_name = model
        .root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| model.root.to_string_lossy().into_owned());
    let name = Paragraph::new(Line::from(Span::styled(
        dir_name,
        Style::new().fg(model.theme.fg),
    )));
    frame.render_widget(name, row);

    let (new_file, new_folder) = action_icons(model);
    let x = row.x + row.width - ACTION_COLS as u16;
    let target = Rect {
        x,
        width: ACTION_COLS as u16,
        ..row
    };
    let spans = vec![
        Span::styled(new_file, Style::new().fg(model.theme.fg_dim)),
        Span::raw("  "),
        Span::styled(new_folder, Style::new().fg(model.theme.fg_dim)),
        Span::raw(" "),
    ];
    let p = Paragraph::new(Line::from(spans)).style(Style::new().bg(model.theme.bg_alt));
    frame.render_widget(p, target);
}

/// Hit-test for the Files header buttons. `area` is the full sidebar rect (as
/// passed to `file_hit`). Mirrors `render_header_actions`' right-edge layout.
pub fn files_header_hit(area: Rect, x: u16, y: u16) -> Option<FileHit> {
    let row = header_row(area);
    let width = row.width as usize;
    if y != row.y || x < row.x || x >= row.x + row.width || width < MIN_ACTION_WIDTH {
        return None;
    }
    let col = x.saturating_sub(row.x) as usize;
    // Render is "[file][ ][ ][folder][ ]" in the last 5 cols: the file glyph sits
    // at width-ACTION_COLS, the folder glyph at width-2; the gap and trailing
    // space between/after them are inert.
    if col == width - 2 {
        return Some(FileHit::NewFolderRoot);
    }
    if col == width - ACTION_COLS {
        return Some(FileHit::NewFileRoot);
    }
    None
}

/// Truncates a name to `max` columns, marking the cut with `…`.
fn fit_name(name: &str, max: usize) -> String {
    let len = name.chars().count();
    if len <= max {
        return name.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let head: String = name.chars().take(max - 1).collect();
    format!("{head}…")
}

/// Returns the visible row index in the file tree based on the mouse y.
pub fn file_row_at(model: &Model, area: Rect, y: u16) -> Option<usize> {
    let body = panel_area(area);
    if y < body.y || y >= body.y + body.height {
        return None;
    }
    let rows_len = model.sidebar.files.visible_rows().len();
    let offset = list_scroll(model.sidebar.files.selected, rows_len, body.height as usize);
    let idx = offset + (y - body.y) as usize;
    if idx < rows_len { Some(idx) } else { None }
}

/// Converts a click into a file-tree target. Mirrors `render`: on a directory row
/// the last 5 columns are the new-file (width-5) and new-folder (width-2) buttons.
pub fn file_hit(model: &Model, area: Rect, x: u16, y: u16) -> Option<FileHit> {
    let idx = file_row_at(model, area, y)?;
    let body = panel_area(area);

    let rows = model.sidebar.files.visible_rows();
    let row = rows.get(idx)?;

    let width = body.width as usize;
    let col = x.saturating_sub(body.x) as usize;

    if row.is_dir && dir_buttons_fit(width, row.depth, model.ascii_icons) {
        // Exact glyph columns only (mirrors `render`): file at width-ACTION_COLS,
        // folder at width-2. Clicking the gap between them falls through to Row.
        if col == width - 2 {
            return Some(FileHit::NewFolder(idx));
        }
        if col == width - ACTION_COLS {
            return Some(FileHit::NewFile(idx));
        }
    }
    Some(FileHit::Row(idx))
}
