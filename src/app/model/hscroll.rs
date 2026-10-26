//! Width of the active tab's longest line, which sizes the editor's
//! horizontal scrollbar.

use super::Model;

impl Model {
    /// Refreshes `active_max_cols` before render. Cached by `(tab id, buffer
    /// version)`, so the full-buffer scan runs only after an edit or a tab
    /// switch. In a diff tab the woven deleted lines count too.
    pub fn refresh_max_line_width(&mut self) {
        let Some(tab) = self.active_tab.and_then(|i| self.tabs.get(i)) else {
            self.active_max_cols = 0;
            self.active_max_cols_key = None;
            return;
        };
        let key = (tab.id, tab.buffer.version, self.active_deleted.len());
        if self.active_max_cols_key == Some(key) {
            return;
        }
        let buf = &tab.buffer;
        let real = (0..buf.line_count()).map(|l| buf.line_len(l)).max();
        let deleted = self
            .active_deleted
            .iter()
            .flat_map(|(_, lines)| lines)
            .map(|l| l.chars().count())
            .max();
        self.active_max_cols = real.max(deleted).unwrap_or(0);
        self.active_max_cols_key = Some(key);
    }
}
