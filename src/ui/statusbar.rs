//! Bottom status bar.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use unicode_width::UnicodeWidthStr;

use crate::app::model::Model;
use crate::services::lsp::Severity;

/// The bar's texts, left to right. Built once and shared by `render` and the
/// terminal button's hit-test, so the click target is where the button is drawn.
struct Bar {
    /// Version and branch.
    head: String,
    /// The terminal toggle button, right after the branch.
    terminal: String,
    /// The diagnostic under the cursor, if any.
    message: String,
    err_seg: String,
    warn_seg: String,
    leader_seg: &'static str,
    right: String,
}

impl Bar {
    fn new(model: &Model) -> Bar {
        let mut head = format!(" Coder v{} ", env!("CARGO_PKG_VERSION"));
        if let Some(branch) = &model.sidebar.git.branch {
            head.push_str(&format!(" ⎇ {branch} "));
        }
        let icon = crate::ui::terminal::icon(model);
        let terminal = format!(" {icon} Terminal ");

        // A diagnostic under the cursor takes over the rest of the message area.
        let message = model
            .diagnostic_at_cursor()
            .map(|diag| {
                let tag = match diag.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Info => "info",
                    Severity::Hint => "hint",
                };
                format!(" {tag}: {} ", diag.message.replace('\n', " "))
            })
            .unwrap_or_default();

        // Error/warning counts for the active file — always shown, colored by severity.
        let (errors, warnings) = model.active_diagnostic_counts();
        let err_icon = crate::ui::editor::severity_icon(Severity::Error, model.ascii_icons);
        let warn_icon = crate::ui::editor::severity_icon(Severity::Warning, model.ascii_icons);
        let err_seg = format!(" {err_icon} {errors}  ");
        let warn_seg = format!("{warn_icon} {warnings}  ");

        let mut right = String::new();
        if let Some(buf) = model.active_buffer() {
            right.push_str(&format!(
                "Ln {}, Col {}  ",
                buf.cursor.line + 1,
                buf.cursor.col + 1
            ));
            if buf.dirty {
                right.push_str("● ");
            }
            if let Some(n) = buf.selected_char_count() {
                right.push_str(&format!("Selected: {n}  "));
            }
        }
        let focus = match model.focus {
            crate::app::model::Focus::Editor => "EDITOR",
            crate::app::model::Focus::Terminal => "TERMINAL",
            crate::app::model::Focus::Sidebar => "SIDEBAR",
            crate::app::model::Focus::SearchInput => "SEARCH",
            crate::app::model::Focus::GitCommit => "COMMIT",
            crate::app::model::Focus::Find => "FIND",
        };
        right.push_str(focus);
        right.push(' ');

        // A visible badge while the leader (unlock) key is armed — without this,
        // pressing it gives no feedback at all, so there is no way to tell whether
        // the keypress even reached the app or whether a locked command is about
        // to fire (see `Action::Leader`).
        let leader_seg = if model.leader { " LEADER " } else { "" };

        Bar {
            head,
            terminal,
            message,
            err_seg,
            warn_seg,
            leader_seg,
            right,
        }
    }

    /// Cells of the right block (diagnostic counts, leader badge, cursor
    /// position, focus), which always wins the space it needs.
    fn right_width(&self) -> usize {
        self.err_seg.width() + self.warn_seg.width() + self.leader_seg.width() + self.right.width()
    }
}

/// `s` cut to at most `max` cells.
fn take_cells(s: &str, max: usize) -> String {
    let mut w = 0;
    s.chars()
        .take_while(|c| {
            w += unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0);
            w <= max
        })
        .collect()
}

/// The left block — head, terminal button, message — cut to what the right
/// block leaves free, in that priority.
fn left_parts(bar: &Bar, width: u16) -> (String, String, String) {
    let mut room = (width as usize).saturating_sub(bar.right_width());
    let mut part = |s: &str| {
        let cut = take_cells(s, room);
        room -= cut.width();
        cut
    };
    (part(&bar.head), part(&bar.terminal), part(&bar.message))
}

/// The clickable terminal toggle (same as Ctrl+J), right after the branch:
/// shared by `render` and the mouse hit-test.
pub fn terminal_button(model: &Model, area: Rect) -> Rect {
    let (head, terminal, _) = left_parts(&Bar::new(model), area.width);
    Rect {
        x: area.x + head.width() as u16,
        width: terminal.width() as u16,
        height: area.height.min(1),
        ..area
    }
}

pub fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let base = Style::new()
        .fg(model.theme.statusbar_fg)
        .bg(model.theme.statusbar_bg);
    // While the panel is open only the terminal button's background changes.
    let term_style = if model.layout.terminal_open {
        base.bg(model.theme.statusbar_active_bg())
    } else {
        base
    }
    .add_modifier(Modifier::BOLD);

    let bar = Bar::new(model);
    let (head, terminal, message) = left_parts(&bar, area.width);
    let lw = head.width() + terminal.width() + message.width();
    let pad = (area.width as usize).saturating_sub(lw + bar.right_width());
    let line = Line::from(vec![
        Span::styled(head, base),
        Span::styled(terminal, term_style),
        Span::styled(message, base),
        Span::styled(" ".repeat(pad), base),
        Span::styled(
            bar.err_seg,
            Style::new()
                .fg(model.theme.git_deleted)
                .bg(model.theme.bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            bar.warn_seg,
            Style::new()
                .fg(model.theme.git_modified)
                .bg(model.theme.bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            bar.leader_seg,
            Style::new()
                .fg(model.theme.statusbar_bg)
                .bg(model.theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(bar.right, base),
    ]);
    frame.render_widget(Paragraph::new(line).style(base), area);
}
