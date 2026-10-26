//! Embedded terminal: tabs, sizing, paste, and the PTY session lifecycle / output.

use super::*;
use crate::app::model::TerminalState;

/// Forwards a bracketed paste straight to the PTY as raw bytes, in one write
/// instead of one `Action::PtyInput` per character — the shell/program inside
/// doesn't care about auto-indent the way the code editor does, so no
/// reindenting is needed here, just delivering it atomically.
pub(super) fn paste_into_terminal(model: &mut Model, text: &str) -> Vec<Cmd> {
    let term = model.terminals.active_mut();
    if let Some(session) = term.session.as_mut() {
        session.write(text.as_bytes());
        term.scroll_to(0);
    }
    Vec::new()
}

/// Propagates the terminal area size to every terminal tab's vt100 parser and
/// PTY (they all share the panel).
pub(super) fn sync_terminal_size(model: &mut Model) {
    if !model.layout.terminal_open {
        return;
    }
    let area = full_rect(model);
    let a = ui::compute_areas(model, area);
    // the terminal area includes the top border / tab row (1 row); the
    // rightmost inner column is reserved for the scrollbar.
    let rows = a.terminal.height.saturating_sub(1).max(1);
    let cols = a.terminal.width.saturating_sub(1).max(1);
    for term in &mut model.terminals.tabs {
        term.resize(rows, cols);
    }
}

/// Spawns the active terminal's shell unless it has (or is getting) one.
pub(super) fn spawn_active_terminal(model: &mut Model) -> Vec<Cmd> {
    let term = model.terminals.active_mut();
    if term.session.is_some() || term.spawn_requested {
        return Vec::new();
    }
    term.spawn_requested = true;
    vec![Cmd::SpawnPty {
        id: term.id,
        rows: term.rows,
        cols: term.cols,
    }]
}

/// Shows the terminal panel and focuses the active terminal, starting its
/// shell if needed.
pub(super) fn open_terminal_panel(model: &mut Model) -> Vec<Cmd> {
    model.layout.terminal_open = true;
    model.focus = Focus::Terminal;
    sync_terminal_size(model);
    spawn_active_terminal(model)
}

/// Opens a new terminal tab (`Action::NewTerminal`). A terminal that never
/// got a shell (the panel was not opened yet) is reused instead.
pub(super) fn new_terminal(model: &mut Model) -> Vec<Cmd> {
    let term = model.terminals.active();
    if term.session.is_some() || term.spawn_requested {
        model.terminals.push();
    }
    open_terminal_panel(model)
}

/// Makes terminal tab `i` the active one.
pub(super) fn select_terminal(model: &mut Model, i: usize) -> Vec<Cmd> {
    if i < model.terminals.tabs.len() {
        model.terminals.active = i;
        model.focus = Focus::Terminal;
    }
    Vec::new()
}

/// Closes terminal tab `i`, ending its shell. Closing the last one hides the
/// panel (a fresh terminal is started the next time it opens).
pub(super) fn close_terminal(model: &mut Model, i: usize) -> Vec<Cmd> {
    if model.terminals.remove(i) {
        model.layout.terminal_open = false;
        if model.focus == Focus::Terminal {
            model.focus = Focus::Editor;
        }
    }
    Vec::new()
}

/// The PTY session of terminal `id` is ready (`Msg::PtyReady`).
pub(super) fn pty_ready(
    model: &mut Model,
    id: u64,
    session: crate::services::pty::PtySession,
) -> Vec<Cmd> {
    // The tab may have been closed while its shell was starting: dropping the
    // session ends that shell.
    let Some(term) = model.terminals.by_id_mut(id) else {
        return Vec::new();
    };
    term.session = Some(session);
    term.spawn_requested = false;
    // Output may have been announced (and buffered) before the session arrived.
    drain_pty_output(term);
    sync_terminal_size(model);
    Vec::new()
}

/// New PTY output is buffered in the session (`Msg::PtyOutput` is only a
/// wake-up): drain it into vt100.
pub(super) fn pty_output(model: &mut Model, id: u64) -> Vec<Cmd> {
    if let Some(term) = model.terminals.by_id_mut(id) {
        drain_pty_output(term);
    }
    Vec::new()
}

/// The shell of terminal `id` exited (`Msg::PtyExited`): its tab closes. A tab
/// the user already closed is gone, so its shell's exit is ignored.
pub(super) fn pty_exited(model: &mut Model, id: u64) -> Vec<Cmd> {
    let Some(i) = model.terminals.index_of(id) else {
        return Vec::new();
    };
    model.notify("Terminal closed".to_string());
    close_terminal(model, i)
}

/// Feeds everything the PTY buffered since the last drain to the vt100 parser,
/// answering the terminal queries in it.
fn drain_pty_output(term: &mut TerminalState) {
    let bytes = term
        .session
        .as_ref()
        .map(|s| s.take_output())
        .unwrap_or_default();
    let replies = term.process_output(&bytes);
    if !replies.is_empty()
        && let Some(session) = term.session.as_mut()
    {
        session.write(&replies);
    }
    // Refresh scrollback bounds and re-anchor the view after vt100 has
    // appended any new rows.
    term.sync_scroll_bounds();
}
