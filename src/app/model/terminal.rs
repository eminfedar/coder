//! State of the embedded terminals: one vt100 parser + PTY session +
//! scrollback per terminal tab.

use crate::services::pty::PtySession;

/// The terminal tabs of the bottom panel. Never empty: closing the last tab
/// leaves a fresh, not-yet-spawned terminal behind, so `active()` always works.
pub struct Terminals {
    pub tabs: Vec<TerminalState>,
    pub active: usize,
    next_id: u64,
}

impl Terminals {
    pub(super) fn new() -> Self {
        Terminals {
            tabs: vec![TerminalState::new(0)],
            active: 0,
            next_id: 1,
        }
    }

    pub fn active(&self) -> &TerminalState {
        &self.tabs[self.active]
    }

    pub fn active_mut(&mut self) -> &mut TerminalState {
        &mut self.tabs[self.active]
    }

    /// The terminal a PTY message belongs to; `None` once its tab was closed.
    pub fn by_id_mut(&mut self, id: u64) -> Option<&mut TerminalState> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Appends a new (not yet spawned) terminal, makes it active and returns its
    /// index. It starts at the active terminal's size.
    pub fn push(&mut self) -> usize {
        let (rows, cols) = (self.active().rows, self.active().cols);
        let mut t = TerminalState::new(self.next_id);
        self.next_id += 1;
        t.resize(rows, cols);
        self.tabs.push(t);
        self.active = self.tabs.len() - 1;
        self.active
    }

    /// Removes tab `i` (dropping its session kills the shell). Returns `true`
    /// when it was the last one, which is replaced by a fresh terminal.
    pub fn remove(&mut self, i: usize) -> bool {
        if i >= self.tabs.len() {
            return false;
        }
        let removed = self.tabs.remove(i);
        if self.active > i || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        if self.tabs.is_empty() {
            let mut t = TerminalState::new(self.next_id);
            self.next_id += 1;
            t.resize(removed.rows, removed.cols);
            self.tabs.push(t);
            self.active = 0;
            return true;
        }
        false
    }
}

pub struct TerminalState {
    /// Stable id: PTY messages carry it, since a tab index shifts on close.
    pub id: u64,
    pub parser: vt100::Parser,
    pub session: Option<PtySession>,
    pub rows: u16,
    pub cols: u16,
    /// The PTY spawn Cmd was sent but the session is not ready yet.
    pub spawn_requested: bool,
    /// Scrollback view offset from the live bottom (0 = following the bottom,
    /// higher = further back in history). Mirrors vt100's internal position.
    pub scroll_offset: usize,
    /// Total scrollback rows currently held by vt100 (the max scroll offset).
    /// Used to size the scrollbar thumb.
    pub scrollback_lines: usize,
    /// Active text selection in visible-grid coordinates:
    /// (start_row, start_col, end_row, end_col).
    pub selection: Option<(u16, u16, u16, u16)>,
    /// Tail of the last output chunk that may be the start of a terminal query
    /// split across reads; held back until the next chunk completes it.
    pending_query: Vec<u8>,
}

/// Terminal queries vt100 does not answer itself. Programs that send one block
/// until the reply arrives — fzf's Ctrl+R history widget asks for the cursor
/// position (`CSI 6n`) before drawing, and otherwise waits on a blank screen,
/// eating the next keystrokes as the "reply".
const QUERIES: [&[u8]; 5] = [b"\x1b[6n", b"\x1b[?6n", b"\x1b[5n", b"\x1b[c", b"\x1b[0c"];

impl TerminalState {
    pub(super) fn new(id: u64) -> Self {
        TerminalState {
            id,
            parser: vt100::Parser::new(24, 80, 2000),
            session: None,
            rows: 24,
            cols: 80,
            spawn_requested: false,
            scroll_offset: 0,
            scrollback_lines: 0,
            selection: None,
            pending_query: Vec::new(),
        }
    }

    /// Feeds shell output to vt100 and returns the replies to the terminal
    /// queries in it (cursor position, status, device attributes), to be
    /// written back to the PTY. Each reply reflects the screen right at its
    /// query, so the output before it is processed first.
    pub fn process_output(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut data = std::mem::take(&mut self.pending_query);
        data.extend_from_slice(bytes);
        let mut replies = Vec::new();
        let mut start = 0;
        let mut i = 0;
        while i < data.len() {
            if data[i] != 0x1b {
                i += 1;
                continue;
            }
            let rest = &data[i..];
            if let Some(q) = QUERIES.iter().find(|q| rest.starts_with(q)) {
                self.parser.process(&data[start..i]);
                replies.extend(self.query_reply(q));
                i += q.len();
                start = i;
                continue;
            }
            // An incomplete query at the very end: keep it for the next chunk.
            if QUERIES
                .iter()
                .any(|q| q.len() > rest.len() && q.starts_with(rest))
            {
                self.parser.process(&data[start..i]);
                self.pending_query = rest.to_vec();
                return replies;
            }
            i += 1;
        }
        self.parser.process(&data[start..]);
        replies
    }

    fn query_reply(&self, query: &[u8]) -> Vec<u8> {
        let (row, col) = self.parser.screen().cursor_position();
        match query {
            b"\x1b[6n" => format!("\x1b[{};{}R", row + 1, col + 1).into_bytes(),
            b"\x1b[?6n" => format!("\x1b[?{};{}R", row + 1, col + 1).into_bytes(),
            b"\x1b[5n" => b"\x1b[0n".to_vec(),
            // Primary device attributes: a VT102-class terminal.
            _ => b"\x1b[?6c".to_vec(),
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 {
            return;
        }
        if rows != self.rows || cols != self.cols {
            self.rows = rows;
            self.cols = cols;
            self.parser.set_size(rows, cols);
            if let Some(s) = self.session.as_ref() {
                s.resize(rows, cols);
            }
        }
        self.sync_scroll_bounds();
    }

    /// Re-reads the total scrollback vt100 holds and re-applies the view
    /// position. Call after every `parser.process()`. When the user is scrolled
    /// back into history, the view stays anchored to the same rows as new output
    /// pushes older rows further up; when following the bottom it keeps
    /// following. vt100 clamps the offset, so the read-back value is authoritative.
    pub fn sync_scroll_bounds(&mut self) {
        let prev_total = self.scrollback_lines;
        // Probe the maximum offset (== total scrollback rows).
        self.parser.set_scrollback(usize::MAX);
        self.scrollback_lines = self.parser.screen().scrollback();
        // Keep the same history in view as new rows are appended.
        let mut target = self.scroll_offset;
        if target > 0 {
            target += self.scrollback_lines.saturating_sub(prev_total);
        }
        self.parser.set_scrollback(target);
        self.scroll_offset = self.parser.screen().scrollback();
    }

    /// Scrolls the view by `delta` rows (positive = back into history). Syncs
    /// vt100 and stores the clamped offset.
    pub fn scroll_by(&mut self, delta: isize) {
        let new = (self.scroll_offset as isize + delta).max(0) as usize;
        self.parser.set_scrollback(new);
        self.scroll_offset = self.parser.screen().scrollback();
    }

    /// Jumps the view to an absolute scrollback offset (0 = live bottom).
    pub fn scroll_to(&mut self, offset: usize) {
        self.parser.set_scrollback(offset);
        self.scroll_offset = self.parser.screen().scrollback();
    }

    /// Extracts the selected text from the currently visible grid.
    pub fn selected_text(&self) -> String {
        let Some((r1, c1, r2, c2)) = self.selection else {
            return String::new();
        };
        // Order by row, then column, so a bottom-up drag copies in reading order.
        let ((min_r, min_c), (max_r, max_c)) = if (r1, c1) <= (r2, c2) {
            ((r1, c1), (r2, c2))
        } else {
            ((r2, c2), (r1, c1))
        };
        self.parser
            .screen()
            .contents_between(min_r, min_c, max_r, max_c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_position_query_is_answered_at_the_cursor() {
        let mut t = TerminalState::new(0);
        // fzf: clear, then ask where the cursor is.
        let reply = t.process_output(b"ab\r\ncd\x1b[J\x1b[6n");
        assert_eq!(reply, b"\x1b[2;3R");
        assert!(t.process_output(b"plain text").is_empty());
    }

    #[test]
    fn query_split_across_reads_is_still_answered() {
        let mut t = TerminalState::new(0);
        assert!(t.process_output(b"xy\x1b[").is_empty());
        assert_eq!(t.process_output(b"6nz"), b"\x1b[1;3R");
        assert_eq!(t.parser.screen().contents(), "xyz");
    }

    #[test]
    fn closing_tabs_keeps_one_terminal_and_a_valid_active_index() {
        let mut terms = Terminals::new();
        terms.push();
        terms.push();
        assert_eq!(terms.active, 2);
        assert!(!terms.remove(2));
        assert_eq!(terms.active, 1);
        assert!(!terms.remove(0));
        assert_eq!(terms.active, 0);
        let last = terms.active().id;
        assert!(terms.remove(0), "removing the last tab reports it");
        assert_eq!(terms.tabs.len(), 1);
        assert_ne!(terms.active().id, last, "replaced by a fresh terminal");
    }
}
