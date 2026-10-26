//! Embedded terminal: sizing, paste, and the PTY session lifecycle / output.

use super::*;

/// Forwards a bracketed paste straight to the PTY as raw bytes, in one write
/// instead of one `Action::PtyInput` per character — the shell/program inside
/// doesn't care about auto-indent the way the code editor does, so no
/// reindenting is needed here, just delivering it atomically.
pub(super) fn paste_into_terminal(model: &mut Model, text: &str) -> Vec<Cmd> {
    if let Some(session) = model.terminal.session.as_mut() {
        session.write(text.as_bytes());
        model.terminal.scroll_to(0);
    }
    Vec::new()
}

/// Propagates the terminal area size to the vt100 parser and the PTY.
pub(super) fn sync_terminal_size(model: &mut Model) {
    if !model.layout.terminal_open {
        return;
    }
    let area = full_rect(model);
    let a = ui::compute_areas(model, area);
    // the terminal area includes the top border (1 row); the rightmost inner
    // column is reserved for the scrollbar.
    let rows = a.terminal.height.saturating_sub(1).max(1);
    let cols = a.terminal.width.saturating_sub(1).max(1);
    model.terminal.resize(rows, cols);
}

/// The PTY session is ready (`Msg::PtyReady`).
pub(super) fn pty_ready(model: &mut Model, session: crate::services::pty::PtySession) -> Vec<Cmd> {
    model.terminal.session = Some(session);
    model.terminal.spawn_requested = false;
    // Output may have been announced (and buffered) before the session arrived.
    drain_pty_output(model);
    sync_terminal_size(model);
    Vec::new()
}

/// New PTY output is buffered in the session (`Msg::PtyOutput` is only a
/// wake-up): drain it into vt100.
pub(super) fn pty_output(model: &mut Model) -> Vec<Cmd> {
    drain_pty_output(model);
    Vec::new()
}

/// The shell exited (`Msg::PtyExited`).
pub(super) fn pty_exited(model: &mut Model) -> Vec<Cmd> {
    model.terminal.session = None;
    model.terminal.spawn_requested = false;
    model.notify("Terminal closed".to_string());
    Vec::new()
}

/// Feeds everything the PTY buffered since the last drain to the vt100 parser,
/// answering the terminal queries in it.
fn drain_pty_output(model: &mut Model) {
    let bytes = model
        .terminal
        .session
        .as_ref()
        .map(|s| s.take_output())
        .unwrap_or_default();
    let replies = model.terminal.process_output(&bytes);
    if !replies.is_empty()
        && let Some(session) = model.terminal.session.as_mut()
    {
        session.write(&replies);
    }
    // Refresh scrollback bounds and re-anchor the view after vt100 has
    // appended any new rows.
    model.terminal.sync_scroll_bounds();
}
