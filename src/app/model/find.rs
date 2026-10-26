//! State of the in-editor find / replace widget.

use crate::core::text_input::TextInputState;

/// Which input field of the in-editor find widget is active.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindField {
    #[default]
    Query,
    Replace,
}

/// In-editor find / find-and-replace widget (floats over the top-right of the editor).
#[derive(Default)]
pub struct FindState {
    pub open: bool,
    /// Whether the replace row (input + Replace/Replace All buttons) is shown.
    pub replace_mode: bool,
    pub query: TextInputState,
    pub replace: TextInputState,
    pub field: FindField,
    /// Match ranges in the active buffer, as absolute [start, end) character indices.
    pub matches: Vec<(usize, usize)>,
    /// Index of the current match within `matches`.
    pub current: Option<usize>,
    /// `(Tab::id, buffer version)` the matches were computed for: any other
    /// tab/version makes them stale (see `update::find::sync_find`).
    pub matches_key: Option<(usize, u64)>,
    /// The query was submitted (Enter / next / prev): only then are `matches`
    /// computed. Typing in the query clears it, so nothing is searched or
    /// selected until the user asks for it.
    pub searched: bool,
}

impl FindState {
    /// "cur/total" indicator (1-based); "0/0" when there are no matches, "-/-"
    /// before the query has been submitted.
    pub fn count_label(&self) -> String {
        if !self.searched {
            return "-/-".to_string();
        }
        let total = self.matches.len();
        let cur = self.current.map(|i| i + 1).unwrap_or(0);
        format!("{cur}/{total}")
    }
}
