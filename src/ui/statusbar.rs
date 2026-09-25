//! Bottom status bar.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::model::Model;

pub fn render(frame: &mut Frame, area: Rect, model: &Model) {
    let base = Style::new()
        .fg(model.theme.statusbar_fg)
        .bg(model.theme.statusbar_bg);

    let mut left = format!(" Coder v{} ", env!("CARGO_PKG_VERSION"));
    if let Some(branch) = &model.sidebar.git.branch {
        left.push_str(&format!(" ⎇ {branch} "));
    }
    // A diagnostic under the cursor takes over the rest of the message area.
    if let Some(diag) = model.diagnostic_at_cursor() {
        let tag = match diag.severity {
            crate::services::lsp::Severity::Error => "error",
            crate::services::lsp::Severity::Warning => "warning",
            crate::services::lsp::Severity::Info => "info",
            crate::services::lsp::Severity::Hint => "hint",
        };
        left.push_str(&format!(" {tag}: {} ", diag.message.replace('\n', " ")));
    }

    // Error/warning counts for the active file — always shown, colored by severity.
    use crate::services::lsp::Severity;
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
    let leader_seg = if model.leader {
        let chord = model
            .keybindings
            .shortcut(crate::services::keybindings::Bindable::Leader);
        format!(" LEADER ON · {chord} → next key ")
    } else {
        String::new()
    };

    let total = area.width as usize;
    let rw = err_seg.chars().count()
        + warn_seg.chars().count()
        + leader_seg.chars().count()
        + right.chars().count();
    // The right block (diagnostic counts, cursor position, focus) always wins the
    // space it needs; the hints are cut to whatever is left.
    let left: String = left.chars().take(total.saturating_sub(rw)).collect();
    let lw = left.chars().count();
    let pad = total.saturating_sub(lw + rw);
    let line = Line::from(vec![
        Span::styled(left, base),
        Span::styled(" ".repeat(pad), base),
        Span::styled(
            err_seg,
            Style::new()
                .fg(model.theme.git_deleted)
                .bg(model.theme.bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            warn_seg,
            Style::new()
                .fg(model.theme.git_modified)
                .bg(model.theme.bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            leader_seg,
            Style::new()
                .fg(model.theme.statusbar_bg)
                .bg(model.theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(right, base),
    ]);
    frame.render_widget(Paragraph::new(line).style(base), area);
}
