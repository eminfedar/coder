//! Draws the embedded PTY terminal from the vt100 screen into ratatui cells,
//! with the terminal tab strip on the panel's top border row.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders};

use unicode_width::UnicodeWidthStr;

use crate::app::model::{Focus, Model};

/// One piece of the tab strip, left to right.
enum Segment {
    Title,
    /// Terminal tab `i`; its last two cells are the "✕ " close button.
    Tab(usize),
    /// The "+" new-terminal button.
    New,
}

/// Target of a click on the terminal tab strip.
pub enum TerminalHit {
    Select(usize),
    Close(usize),
    New,
}

/// The terminal icon (Nerd Font glyph, `>_` in ASCII mode): the strip's title
/// and the status bar's Terminal button.
pub fn icon(model: &Model) -> &'static str {
    if model.ascii_icons { ">_" } else { "\u{f489}" }
}

fn title(model: &Model) -> String {
    format!(" {} ", icon(model))
}
const NEW: &str = " + ";

fn tab_label(i: usize) -> String {
    format!(" {} {} ", crate::services::pty::shell_name(), i + 1)
}

/// The strip's segments with their widths in cells: the single layout both
/// `render` and `hit` walk, so clicks land on what is drawn.
fn segments(model: &Model) -> Vec<(Segment, usize)> {
    let mut out = vec![(Segment::Title, title(model).width())];
    for i in 0..model.terminals.tabs.len() {
        // label + "✕ " + "│"
        out.push((Segment::Tab(i), tab_label(i).width() + 3));
    }
    out.push((Segment::New, NEW.width()));
    out
}

/// Which tab / button of the strip (the panel's top row) is at (x, y).
pub fn hit(model: &Model, area: Rect, x: u16, y: u16) -> Option<TerminalHit> {
    if y != area.y || x < area.x || x >= area.x + area.width {
        return None;
    }
    let x = x as usize;
    let mut cursor = area.x as usize;
    for (seg, w) in segments(model) {
        if x >= cursor && x < cursor + w {
            return match seg {
                Segment::Title => None,
                // "✕ " sits right before the trailing "│".
                Segment::Tab(i) if x + 3 >= cursor + w && x + 1 < cursor + w => {
                    Some(TerminalHit::Close(i))
                }
                Segment::Tab(i) => Some(TerminalHit::Select(i)),
                Segment::New => Some(TerminalHit::New),
            };
        }
        cursor += w;
    }
    None
}

/// Draws the tab strip over the top border row, on the sidebar background;
/// the active tab has the active-tab background.
fn render_tabs(buf: &mut ratatui::buffer::Buffer, area: Rect, model: &Model, focused: bool) {
    let th = &model.theme;
    let bg = th.bg_alt;
    let right = area.x + area.width;
    let mut x = area.x;
    for (seg, _) in segments(model) {
        let parts: Vec<(String, Style)> = match seg {
            Segment::Title => vec![(title(model), Style::new().fg(th.fg_dim).bg(bg))],
            Segment::Tab(i) => {
                let active = i == model.terminals.active;
                // The active tab gets the editor's active-tab background.
                let bg = if active { th.tab_active_bg } else { bg };
                let mut label = Style::new().fg(th.fg).bg(bg);
                if active {
                    label = label.add_modifier(Modifier::BOLD);
                    if focused {
                        label = label.fg(th.accent);
                    }
                }
                vec![
                    (tab_label(i), label),
                    ("✕ ".to_string(), Style::new().fg(th.fg_dim).bg(bg)),
                    ("│".to_string(), Style::new().fg(th.border).bg(bg)),
                ]
            }
            Segment::New => vec![(NEW.to_string(), Style::new().fg(th.fg_dim).bg(bg))],
        };
        for (text, style) in parts {
            if x >= right {
                return;
            }
            let (end, _) = buf.set_stringn(x, area.y, &text, (right - x) as usize, style);
            x = end;
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let focused = model.focus == Focus::Terminal;
    let border_color = if focused {
        model.theme.accent
    } else {
        model.theme.border
    };
    let block = Block::new()
        .borders(Borders::TOP)
        // The top row (border + tab strip) uses the sidebar background.
        .border_style(Style::new().fg(border_color).bg(model.theme.bg_alt))
        .style(Style::new().bg(model.theme.bg_alt));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_tabs(frame.buffer_mut(), area, model, focused);

    // vt100's visible grid already reflects the scrollback view position
    // (set via parser.set_scrollback in the update layer), so cell()/rows()/
    // cursor_position() all operate on what should be shown right now.
    let screen = model.terminals.active().parser.screen();
    let (rows, cols) = screen.size();
    let selection = model.terminals.active().selection;

    // Reserve the rightmost inner column for the scrollbar. The PTY grid width
    // is kept one column narrower (see sync_terminal_size) so nothing is hidden.
    let has_bar = inner.width > 1;
    let text_w = if has_bar {
        inner.width - 1
    } else {
        inner.width
    };

    let buf = frame.buffer_mut();

    for r in 0..inner.height.min(rows) {
        for c in 0..text_w.min(cols) {
            let x = inner.x + c;
            let y = inner.y + r;
            let Some(cell) = screen.cell(r, c) else {
                continue;
            };
            let Some(out) = buf.cell_mut((x, y)) else {
                continue;
            };
            let contents = cell.contents();
            if contents.is_empty() {
                out.set_char(' ');
            } else {
                out.set_symbol(&contents);
            }
            let mut style = Style::new()
                .fg(conv_color(cell.fgcolor(), model.theme.fg))
                .bg(conv_color(cell.bgcolor(), model.theme.bg_alt));
            if cell.bold() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if cell.italic() {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if cell.underline() {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if cell.inverse() {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if selection_contains(&selection, r, c) {
                style = style.bg(model.theme.selection);
            }
            out.set_style(style);
        }
    }

    if has_bar {
        render_scrollbar(buf, inner, model);
    }

    // Cursor — only while following the live bottom; when scrolled back into
    // history it would land on the wrong row.
    if focused && model.terminals.active().scroll_offset == 0 && !screen.hide_cursor() {
        let (cr, cc) = screen.cursor_position();
        let x = inner.x + cc;
        let y = inner.y + cr;
        if x < inner.x + text_w && y < inner.y + inner.height {
            frame.set_cursor_position((x, y));
        }
    }
}

/// Draws the scrollback scrollbar in the rightmost inner column. The thumb
/// covers the visible window's proportion of the whole scrollback + screen.
fn render_scrollbar(buf: &mut ratatui::buffer::Buffer, inner: Rect, model: &Model) {
    let th = &model.theme;
    let h = inner.height as usize;
    if h == 0 {
        return;
    }
    let x = inner.x + inner.width - 1;
    let rows = model.terminals.active().rows as usize;
    let total = model.terminals.active().scrollback_lines + rows;
    let offset = model.terminals.active().scroll_offset;

    // How many history rows sit above the top of the viewport.
    let above = model
        .terminals
        .active()
        .scrollback_lines
        .saturating_sub(offset);
    let (thumb_start, thumb_end) = if total > rows && total > 0 {
        let ts = above * h / total;
        let tl = (rows * h / total).max(1);
        (ts, (ts + tl).min(h))
    } else {
        (0, h)
    };

    for y in 0..h {
        let in_thumb = y >= thumb_start && y < thumb_end;
        let bg = if in_thumb { th.fg_dim } else { th.bg_alt };
        if let Some(out) = buf.cell_mut((x, inner.y + y as u16)) {
            out.set_char(' ');
            out.set_style(Style::new().bg(bg));
        }
    }
}

/// Returns true if the given visible-grid (row, col) falls within the selection.
fn selection_contains(selection: &Option<(u16, u16, u16, u16)>, row: u16, col: u16) -> bool {
    let Some((r1, c1, r2, c2)) = *selection else {
        return false;
    };
    // Linear (logical) selection matching contents_between: fully-covered rows
    // between the endpoints, partial first/last rows.
    let ((sr, sc), (er, ec)) = if (r1, c1) <= (r2, c2) {
        ((r1, c1), (r2, c2))
    } else {
        ((r2, c2), (r1, c1))
    };
    if row < sr || row > er {
        return false;
    }
    if sr == er {
        return col >= sc && col <= ec;
    }
    if row == sr {
        return col >= sc;
    }
    if row == er {
        return col <= ec;
    }
    true
}

fn conv_color(c: vt100::Color, default: Color) -> Color {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
