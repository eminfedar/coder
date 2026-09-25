//! Rope-based text buffer: cursor, selection, undo/redo, dirty flag.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;

use ropey::Rope;

/// A position within the text (line, column) — both 0-based, in character units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
}

/// A single undoable edit: replaces the `before` text with `after` at
/// position `char_idx`. Undo applies these in reverse.
#[derive(Clone, Debug)]
struct Edit {
    char_idx: usize,
    before: String,
    after: String,
    cursor_before: Cursor,
    cursor_after: Cursor,
    /// Time of the last change, used to group consecutive typing.
    stamp: Instant,
    /// Is this edit a pure "typing" edit (consecutive character insertion)?
    typing: bool,
}

pub struct Buffer {
    pub path: Option<PathBuf>,
    pub rope: Rope,
    pub cursor: Cursor,
    /// Selection anchor. When `Some`, the range between it and the cursor is selected.
    pub anchor: Option<Cursor>,
    /// Topmost visible line of the editor viewport.
    pub scroll_y: usize,
    pub scroll_x: usize,
    pub dirty: bool,
    /// Version that increments on every edit; used to invalidate the highlight cache.
    pub version: u64,
    /// Lowest line index touched since the highlight cache last consumed it.
    /// The highlighter only re-highlights from here down, reusing the cached
    /// prefix. `usize::MAX` means "no edit since last highlight".
    dirty_from: usize,
    /// Whether any edit since the last highlight spanned more than one line (its
    /// removed or inserted text contained a newline). A single-line edit lets the
    /// highlighter stop as soon as the per-line parse state reconverges; a wide
    /// edit may have shifted lines, so the whole viewport tail is re-highlighted.
    dirty_wide: bool,
    /// Cached longest source line; edits invalidate it through `version`.
    max_line_len_cache: Cell<Option<(u64, usize)>>,
    /// Cached document line-ending label; edits invalidate it through `version`.
    line_ending_cache: Cell<Option<(u64, &'static str)>>,
    undo_stack: Vec<Edit>,
    redo_stack: Vec<Edit>,
}

impl Buffer {
    pub fn new(path: Option<PathBuf>, text: &str) -> Self {
        Buffer {
            path,
            rope: Rope::from_str(text),
            cursor: Cursor::default(),
            anchor: None,
            scroll_y: 0,
            scroll_x: 0,
            dirty: false,
            version: 0,
            dirty_from: 0,
            dirty_wide: false,
            max_line_len_cache: Cell::new(None),
            line_ending_cache: Cell::new(None),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Records that line `at` (and everything below it) may need re-highlighting.
    /// `wide` is true when the edit's removed or inserted text crossed a line
    /// boundary, so the highlighter cannot assume line indices stayed aligned.
    fn mark_dirty_from(&mut self, char_idx: usize, wide: bool) {
        let idx = char_idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        self.dirty_from = self.dirty_from.min(line);
        self.dirty_wide |= wide;
    }

    /// Returns `(lowest changed line, wide?)` since the last call, then resets to
    /// "clean". `usize::MAX` means nothing changed (only a scroll may have extended
    /// the needed range). `wide` is true if any edit crossed a line boundary.
    pub fn take_dirty(&mut self) -> (usize, bool) {
        let from = std::mem::replace(&mut self.dirty_from, usize::MAX);
        let wide = std::mem::replace(&mut self.dirty_wide, false);
        (from, wide)
    }

    /// Records a mutation at `char_idx`: bumps the version, marks the buffer
    /// dirty, and extends the re-highlight range. Every code path that changes
    /// the rope must go through here so the highlight cache never keeps a stale
    /// prefix — the version bump and `mark_dirty_from` are two halves of one act.
    /// `wide` = the edit's before/after text crossed a line boundary.
    fn bump_version(&mut self, char_idx: usize, wide: bool) {
        self.dirty = true;
        self.version += 1;
        self.mark_dirty_from(char_idx, wide);
    }

    #[cfg(test)]
    pub fn scratch() -> Self {
        Buffer::new(None, "")
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string())
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines().max(1)
    }

    /// Character length of the given line (excluding the line ending).
    pub fn line_len(&self, line: usize) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let slice = self.rope.line(line);
        let mut len = slice.len_chars();
        // Exclude line-ending characters from the count.
        while len > 0 {
            let c = slice.char(len - 1);
            if c == '\n' || c == '\r' {
                len -= 1;
            } else {
                break;
            }
        }
        len
    }

    /// Character length of the longest line, excluding line endings.
    pub fn max_line_len(&self) -> usize {
        if let Some((version, len)) = self.max_line_len_cache.get()
            && version == self.version
        {
            return len;
        }
        let len = (0..self.line_count())
            .map(|line| self.line_len(line))
            .max()
            .unwrap_or(0);
        self.max_line_len_cache.set(Some((self.version, len)));
        len
    }

    pub fn line_text(&self, line: usize) -> String {
        if line >= self.rope.len_lines() {
            return String::new();
        }
        let slice = self.rope.line(line);
        let mut s: String = slice.chars().collect();
        while s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        s
    }

    pub fn full_text(&self) -> String {
        self.rope.to_string()
    }

    /// Returns the document's line-ending style, or an empty string when the
    /// document has no line breaks. The result is cached for the current buffer
    /// version so rendering the status bar does not repeatedly scan the file.
    pub fn line_ending_label(&self) -> &'static str {
        if let Some((version, label)) = self.line_ending_cache.get()
            && version == self.version
        {
            return label;
        }

        let mut detected = None;
        for line in 0..self.rope.len_lines() {
            let ending = self.line_ending(line);
            if ending.is_empty() {
                continue;
            }
            let label = if ending == "\r\n" { "CRLF" } else { "LF" };
            if detected.is_some_and(|previous| previous != label) {
                self.line_ending_cache.set(Some((self.version, "Mixed")));
                return "Mixed";
            }
            detected = Some(label);
        }

        let label = detected.unwrap_or("");
        self.line_ending_cache.set(Some((self.version, label)));
        label
    }

    /// The terminator that ends `line` in the rope: `"\r\n"`, `"\n"`, or `""`
    /// for the final line (or one past the end).
    fn line_ending(&self, line: usize) -> &'static str {
        if line >= self.rope.len_lines() {
            return "";
        }
        let slice = self.rope.line(line);
        let n = slice.len_chars();
        if n == 0 || slice.char(n - 1) != '\n' {
            ""
        } else if n >= 2 && slice.char(n - 2) == '\r' {
            "\r\n"
        } else {
            "\n"
        }
    }

    /// The line break to insert on `line`: its own terminator, or — for the
    /// unterminated last line — the one of the line above, so a CRLF file stays
    /// CRLF. Falls back to `"\n"`.
    fn newline_for(&self, line: usize) -> &'static str {
        match self.line_ending(line) {
            "" => match line.checked_sub(1).map(|l| self.line_ending(l)) {
                Some("\r\n") => "\r\n",
                _ => "\n",
            },
            nl => nl,
        }
    }

    /// Absolute character index of the cursor (for find/replace positioning).
    pub fn cursor_char_index(&self) -> usize {
        self.cursor_to_char(self.cursor)
    }

    fn cursor_to_char(&self, c: Cursor) -> usize {
        let line = c.line.min(self.rope.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_char(line);
        let max_col = self.line_len(line);
        line_start + c.col.min(max_col)
    }

    fn char_to_cursor(&self, idx: usize) -> Cursor {
        let idx = idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        let line_start = self.rope.line_to_char(line);
        Cursor {
            line,
            col: idx - line_start,
        }
    }

    // ----- LSP position conversion (UTF-16 code units <-> char columns) -----
    //
    // LSP `Position.character` counts UTF-16 code units within a line, while the
    // rope (and `Cursor.col`) uses Unicode-scalar (char) indices. These differ
    // for any non-BMP character (emoji, some CJK), so every LSP boundary must
    // convert. Do this only here (rope is the source of truth), never in async.

    /// Char column within a line -> UTF-16 code-unit offset (outgoing to LSP).
    pub fn char_col_to_utf16(&self, line: usize, col: usize) -> u32 {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let slice = self.rope.line(line);
        let col = col.min(self.line_len(line));
        slice.char_to_utf16_cu(col) as u32
    }

    /// UTF-16 code-unit offset within a line -> char column (incoming from LSP),
    /// clamped to the line's content length.
    pub fn utf16_to_char_col(&self, line: usize, utf16: u32) -> usize {
        if line >= self.rope.len_lines() {
            return 0;
        }
        let slice = self.rope.line(line);
        let max_col = self.line_len(line);
        let max_u16 = slice.char_to_utf16_cu(max_col);
        let u = (utf16 as usize).min(max_u16);
        slice.utf16_cu_to_char(u).min(max_col)
    }

    /// Absolute char index of an LSP position (line + UTF-16 character). Clamps a
    /// past-the-end line to the document end (LSP edits can target EOF).
    pub fn lsp_pos_to_char(&self, line: usize, utf16: u32) -> usize {
        if line >= self.rope.len_lines() {
            return self.rope.len_chars();
        }
        self.rope.line_to_char(line) + self.utf16_to_char_col(line, utf16)
    }

    // ----- Selection -----

    /// The selection's (start, end) cursors in sorted order.
    pub fn selection_range(&self) -> Option<(Cursor, Cursor)> {
        let a = self.anchor?;
        if a == self.cursor {
            return None;
        }
        if a < self.cursor {
            Some((a, self.cursor))
        } else {
            Some((self.cursor, a))
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        let s = self.cursor_to_char(start);
        let e = self.cursor_to_char(end);
        Some(self.rope.slice(s..e).to_string())
    }

    /// Number of chars in the selection, without copying it out.
    pub fn selected_char_count(&self) -> Option<usize> {
        let (start, end) = self.selection_range()?;
        Some(self.cursor_to_char(end) - self.cursor_to_char(start))
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Selection handling before a move: when `extend` is true the anchor is kept.
    fn pre_move(&mut self, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
    }

    // ----- Cursor movements -----

    pub fn move_left(&mut self, extend: bool) {
        self.pre_move(extend);
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.line_len(self.cursor.line);
        }
    }

    pub fn move_right(&mut self, extend: bool) {
        self.pre_move(extend);
        let len = self.line_len(self.cursor.line);
        if self.cursor.col < len {
            self.cursor.col += 1;
        } else if self.cursor.line + 1 < self.line_count() {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
    }

    pub fn move_up(&mut self, extend: bool) {
        self.pre_move(extend);
        if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.cursor.col.min(self.line_len(self.cursor.line));
        } else {
            self.cursor.col = 0;
        }
    }

    pub fn move_down(&mut self, extend: bool) {
        self.pre_move(extend);
        if self.cursor.line + 1 < self.line_count() {
            self.cursor.line += 1;
            self.cursor.col = self.cursor.col.min(self.line_len(self.cursor.line));
        } else {
            self.cursor.col = self.line_len(self.cursor.line);
        }
    }

    /// Smart Home: jumps to the first non-whitespace character. If already there
    /// (or the line has no indent), toggles to column 0.
    pub fn move_home(&mut self, extend: bool) {
        self.pre_move(extend);
        let first: usize = self
            .line_text(self.cursor.line)
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .count();
        self.cursor.col = if self.cursor.col == first { 0 } else { first };
    }

    pub fn move_end(&mut self, extend: bool) {
        self.pre_move(extend);
        self.cursor.col = self.line_len(self.cursor.line);
    }

    /// Moves the cursor left to the previous word boundary (Ctrl+Left).
    /// Skips whitespace, then a run of same-class characters (word vs. symbol).
    pub fn move_word_left(&mut self, extend: bool) {
        self.pre_move(extend);
        let mut i = self.cursor_to_char(self.cursor);
        let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
        while i > 0 && self.rope.char(i - 1).is_whitespace() {
            i -= 1;
        }
        if i > 0 {
            let word = is_word(self.rope.char(i - 1));
            while i > 0 {
                let c = self.rope.char(i - 1);
                if c.is_whitespace() || is_word(c) != word {
                    break;
                }
                i -= 1;
            }
        }
        self.cursor = self.char_to_cursor(i);
    }

    /// Moves the cursor right to the next word boundary (Ctrl+Right).
    pub fn move_word_right(&mut self, extend: bool) {
        self.pre_move(extend);
        let len = self.rope.len_chars();
        let mut i = self.cursor_to_char(self.cursor);
        let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
        while i < len && self.rope.char(i).is_whitespace() {
            i += 1;
        }
        if i < len {
            let word = is_word(self.rope.char(i));
            while i < len {
                let c = self.rope.char(i);
                if c.is_whitespace() || is_word(c) != word {
                    break;
                }
                i += 1;
            }
        }
        self.cursor = self.char_to_cursor(i);
    }

    pub fn move_page(&mut self, delta: isize, extend: bool) {
        self.pre_move(extend);
        let target = (self.cursor.line as isize + delta)
            .clamp(0, self.line_count().saturating_sub(1) as isize) as usize;
        self.cursor.line = target;
        self.cursor.col = self.cursor.col.min(self.line_len(self.cursor.line));
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(Cursor { line: 0, col: 0 });
        let last = self.line_count().saturating_sub(1);
        self.cursor = Cursor {
            line: last,
            col: self.line_len(last),
        };
    }

    /// Selects the word (identifier run) at the given position. No-op when there
    /// is no word character to select there. Used by editor double-click.
    pub fn select_word_at(&mut self, c: Cursor) {
        let line = c.line.min(self.line_count().saturating_sub(1));
        let text: Vec<char> = self.line_text(line).chars().collect();
        let len = text.len();
        let col = c.col.min(len);
        let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
        if !text.get(col).copied().map(is_word).unwrap_or(false) {
            return; // not on a word character
        }
        let (mut s, mut e) = (col, col);
        while s > 0 && is_word(text[s - 1]) {
            s -= 1;
        }
        while e < len && is_word(text[e]) {
            e += 1;
        }
        if s == e {
            return;
        }
        self.anchor = Some(Cursor { line, col: s });
        self.cursor = Cursor { line, col: e };
    }

    /// Selects the character range [start, end) given in absolute character
    /// indices (used to highlight a find match). Clamps into the text.
    pub fn select_char_range(&mut self, start: usize, end: usize) {
        self.anchor = Some(self.char_to_cursor(start));
        self.cursor = self.char_to_cursor(end);
    }

    pub fn set_cursor(&mut self, c: Cursor, extend: bool) {
        self.pre_move(extend);
        let line = c.line.min(self.line_count().saturating_sub(1));
        self.cursor = Cursor {
            line,
            col: c.col.min(self.line_len(line)),
        };
    }

    // ----- Editing -----

    /// Deletes the selection (if any) and records the edit (merged into a single undo step).
    fn delete_selection_internal(&mut self) -> bool {
        if let Some((start, end)) = self.selection_range() {
            let s = self.cursor_to_char(start);
            let e = self.cursor_to_char(end);
            let removed = self.rope.slice(s..e).to_string();
            let cursor_before = self.cursor;
            self.rope.remove(s..e);
            self.anchor = None;
            self.cursor = start;
            self.push_edit(Edit {
                char_idx: s,
                before: removed,
                after: String::new(),
                cursor_before,
                cursor_after: self.cursor,
                stamp: Instant::now(),
                typing: false,
            });
            true
        } else {
            false
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        self.delete_selection_internal();
        let idx = self.cursor_to_char(self.cursor);
        let cursor_before = self.cursor;
        self.rope.insert_char(idx, ch);
        self.cursor = self.char_to_cursor(idx + 1);
        let is_word = !ch.is_whitespace();
        self.push_edit(Edit {
            char_idx: idx,
            before: String::new(),
            after: ch.to_string(),
            cursor_before,
            cursor_after: self.cursor,
            stamp: Instant::now(),
            typing: is_word,
        });
    }

    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.delete_selection_internal();
        let idx = self.cursor_to_char(self.cursor);
        let cursor_before = self.cursor;
        self.rope.insert(idx, text);
        let char_len = text.chars().count();
        self.cursor = self.char_to_cursor(idx + char_len);
        self.push_edit(Edit {
            char_idx: idx,
            before: String::new(),
            after: text.to_string(),
            cursor_before,
            cursor_after: self.cursor,
            stamp: Instant::now(),
            typing: false,
        });
    }

    /// The current line's leading whitespace, clipped at the cursor so pressing
    /// Enter *inside* the indent only carries the part the cursor is past.
    fn indent_at_cursor(&self) -> String {
        self.line_text(self.cursor.line)
            .chars()
            .take(self.cursor.col)
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    }

    /// Inserts a line break, carrying the current line's indentation onto the
    /// new line. The break and the indent are one edit, so undo takes both and
    /// the typing group ends here.
    pub fn insert_newline(&mut self) {
        self.delete_selection_internal();
        let indent = self.indent_at_cursor();
        let nl = self.newline_for(self.cursor.line);
        self.insert_str(&format!("{nl}{indent}"));
    }

    /// Inserts pasted text, re-indenting the continuation lines so the block
    /// aligns to the cursor's current indentation instead of keeping whatever
    /// (often deeper) leading whitespace it was copied with. Single-line pastes
    /// insert verbatim — unless the whole paste is a JSON object/array, which is
    /// pretty-printed first (see `pretty_print_json`), so a minified blob (e.g.
    /// copied from a browser's network tab) lands readable instead of as one
    /// giant line.
    pub fn insert_paste(&mut self, text: &str) {
        let owned;
        let text = match pretty_print_json(text) {
            Some(pretty) => {
                owned = pretty;
                owned.as_str()
            }
            None => text,
        };
        if !text.contains('\n') {
            self.insert_str(text);
            return;
        }
        // Drop the selection first so the base indent is read from the line the
        // paste actually lands on.
        self.delete_selection_internal();
        let base = self.indent_at_cursor();
        let reindented = reindent_paste(text, &base);
        self.insert_str(&reindented);
    }

    /// Moves the line(s) touched by the selection (or just the cursor line) up
    /// (`delta < 0`) or down (`delta > 0`) by one, carrying the cursor and
    /// selection with them. Recorded as a single undo step.
    pub fn move_lines(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        let last = self.line_count().saturating_sub(1);
        let (sel_start, sel_end) = self.selection_range().unwrap_or((self.cursor, self.cursor));
        let start = sel_start.line;
        // A selection ending at column 0 doesn't visually include that last line.
        let end = if sel_end.line > start && sel_end.col == 0 {
            sel_end.line - 1
        } else {
            sel_end.line
        };
        if delta < 0 && start == 0 {
            return;
        }
        if delta > 0 && end >= last {
            return;
        }

        // Region of physical lines to rewrite, and their new order.
        let (region_start_line, region_end_line, order): (usize, usize, Vec<usize>) = if delta < 0 {
            (
                start - 1,
                end,
                (start..=end).chain(std::iter::once(start - 1)).collect(),
            )
        } else {
            (
                start,
                end + 1,
                std::iter::once(end + 1).chain(start..=end).collect(),
            )
        };

        let region_start = self.rope.line_to_char(region_start_line);
        let region_end = if region_end_line + 1 < self.rope.len_lines() {
            self.rope.line_to_char(region_end_line + 1)
        } else {
            self.rope.len_chars()
        };
        let old = self.rope.slice(region_start..region_end).to_string();

        // Terminators stay with their slot (position in the region), so the
        // region's last line keeps having none at EOF and CRLF stays CRLF.
        let mut new_text = String::new();
        for (i, &ln) in order.iter().enumerate() {
            new_text.push_str(&self.line_text(ln));
            new_text.push_str(self.line_ending(region_start_line + i));
        }

        let cursor_before = self.cursor;
        self.rope.remove(region_start..region_end);
        self.rope.insert(region_start, &new_text);

        // Cursor and selection ride along with the block by one line.
        self.cursor.line = (self.cursor.line as isize + delta) as usize;
        if let Some(a) = self.anchor.as_mut() {
            a.line = (a.line as isize + delta) as usize;
        }

        self.push_edit(Edit {
            char_idx: region_start,
            before: old,
            after: new_text,
            cursor_before,
            cursor_after: self.cursor,
            stamp: Instant::now(),
            typing: false,
        });
    }

    /// The inclusive line range the selection touches, or `None` with no
    /// selection. A selection ending at column 0 does not include that final
    /// line — nothing on it is actually selected.
    fn selected_line_span(&self) -> Option<(usize, usize)> {
        let (s, e) = self.selection_range()?;
        let end = if e.line > s.line && e.col == 0 {
            e.line - 1
        } else {
            e.line
        };
        Some((s.line, end))
    }

    /// True when the selection spans more than one line — the case where Tab
    /// indents the block instead of inserting a tab.
    pub fn selection_is_multiline(&self) -> bool {
        self.selected_line_span().is_some_and(|(s, e)| e > s)
    }

    /// Indents every selected line by one unit (four spaces). Blank lines are
    /// left untouched so no trailing whitespace is created.
    pub fn indent_selection(&mut self) {
        if let Some((start, end)) = self.selected_line_span() {
            self.shift_lines(start, end, true);
        }
    }

    /// Removes up to one indent unit (four leading spaces, or a leading tab) from
    /// every selected line.
    pub fn dedent_selection(&mut self) {
        if let Some((start, end)) = self.selected_line_span() {
            self.shift_lines(start, end, false);
        }
    }

    /// Re-indents lines `start..=end` as a single undo step, carrying the cursor
    /// and selection anchor along by however much their own line shifted. A no-op
    /// (dedenting lines with no leading whitespace) makes no edit.
    fn shift_lines(&mut self, start: usize, end: usize, indent: bool) {
        const UNIT: &str = "    ";

        // Per-line column shift, computed from the original text.
        let delta = |line: usize| -> isize {
            let t = self.line_text(line);
            if indent {
                if t.is_empty() { 0 } else { UNIT.len() as isize }
            } else {
                -(leading_indent_width(&t) as isize)
            }
        };

        // Build the replacement text line by line, preserving line endings.
        let region_start = self.rope.line_to_char(start);
        let region_end = if end + 1 < self.rope.len_lines() {
            self.rope.line_to_char(end + 1)
        } else {
            self.rope.len_chars()
        };
        let old = self.rope.slice(region_start..region_end).to_string();
        let mut new_text = String::new();
        for ln in start..=end {
            let t = self.line_text(ln);
            if indent {
                if !t.is_empty() {
                    new_text.push_str(UNIT);
                }
                new_text.push_str(&t);
            } else {
                let drop = leading_indent_width(&t);
                new_text.extend(t.chars().skip(drop));
            }
            new_text.push_str(self.line_ending(ln));
        }
        // Dedenting lines with no indent changes nothing — skip the edit so it
        // does not bump the version or leave an empty undo step.
        if new_text == old {
            return;
        }

        // Shift the cursor / anchor by their line's delta before mutating.
        let recol = |c: Cursor| -> Cursor {
            if c.line < start || c.line > end {
                return c;
            }
            let col = (c.col as isize + delta(c.line)).max(0) as usize;
            Cursor { line: c.line, col }
        };
        let cursor_before = self.cursor;
        let new_cursor = recol(self.cursor);
        let new_anchor = self.anchor.map(recol);

        self.rope.remove(region_start..region_end);
        self.rope.insert(region_start, &new_text);
        self.cursor = new_cursor;
        self.anchor = new_anchor;
        self.push_edit(Edit {
            char_idx: region_start,
            before: old,
            after: new_text,
            cursor_before,
            cursor_after: self.cursor,
            stamp: Instant::now(),
            typing: false,
        });
    }

    pub fn backspace(&mut self) {
        if self.delete_selection_internal() {
            return;
        }
        let idx = self.cursor_to_char(self.cursor);
        if idx == 0 {
            return;
        }
        // A CRLF line break is one unit: joining lines removes both chars.
        let start =
            if idx >= 2 && self.rope.char(idx - 1) == '\n' && self.rope.char(idx - 2) == '\r' {
                idx - 2
            } else {
                idx - 1
            };
        let removed: String = self.rope.slice(start..idx).to_string();
        let cursor_before = self.cursor;
        self.rope.remove(start..idx);
        self.cursor = self.char_to_cursor(start);
        self.push_edit(Edit {
            char_idx: start,
            before: removed,
            after: String::new(),
            cursor_before,
            cursor_after: self.cursor,
            stamp: Instant::now(),
            typing: false,
        });
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection_internal() {
            return;
        }
        let idx = self.cursor_to_char(self.cursor);
        if idx >= self.rope.len_chars() {
            return;
        }
        // At the end of a CRLF line the cursor sits on the '\r': delete the
        // whole break, not half of it.
        let end = if self.rope.char(idx) == '\r'
            && idx + 1 < self.rope.len_chars()
            && self.rope.char(idx + 1) == '\n'
        {
            idx + 2
        } else {
            idx + 1
        };
        let removed: String = self.rope.slice(idx..end).to_string();
        let cursor_before = self.cursor;
        self.rope.remove(idx..end);
        self.push_edit(Edit {
            char_idx: idx,
            before: removed,
            after: String::new(),
            cursor_before,
            cursor_after: self.cursor,
            stamp: Instant::now(),
            typing: false,
        });
    }

    fn push_edit(&mut self, edit: Edit) {
        let wide = edit.before.contains('\n') || edit.after.contains('\n');
        self.bump_version(edit.char_idx, wide);
        self.redo_stack.clear();

        // Merge consecutive typed characters into a single undo step.
        if edit.typing
            && let Some(last) = self.undo_stack.last_mut()
        {
            let contiguous = last.typing
                && last.before.is_empty()
                && edit.before.is_empty()
                && last.char_idx + last.after.chars().count() == edit.char_idx
                && edit.stamp.duration_since(last.stamp).as_millis() < 600;
            if contiguous {
                last.after.push_str(&edit.after);
                last.cursor_after = edit.cursor_after;
                last.stamp = edit.stamp;
                return;
            }
        }
        self.undo_stack.push(edit);
    }

    pub fn undo(&mut self) {
        if let Some(edit) = self.undo_stack.pop() {
            let start = edit.char_idx;
            let after_len = edit.after.chars().count();
            // Remove the `after` text and restore the `before` text.
            self.rope.remove(start..start + after_len);
            if !edit.before.is_empty() {
                self.rope.insert(start, &edit.before);
            }
            self.cursor = edit.cursor_before;
            self.anchor = None;
            let wide = edit.before.contains('\n') || edit.after.contains('\n');
            self.bump_version(start, wide);
            self.redo_stack.push(edit);
        }
    }

    pub fn redo(&mut self) {
        if let Some(edit) = self.redo_stack.pop() {
            let start = edit.char_idx;
            let before_len = edit.before.chars().count();
            self.rope.remove(start..start + before_len);
            if !edit.after.is_empty() {
                self.rope.insert(start, &edit.after);
            }
            self.cursor = edit.cursor_after;
            self.anchor = None;
            let wide = edit.before.contains('\n') || edit.after.contains('\n');
            self.bump_version(start, wide);
            self.undo_stack.push(edit);
        }
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Replaces the entire buffer content, recorded as a single undo step.
    /// The cursor is clamped into the new text. No-op if the text is unchanged.
    pub fn replace_all(&mut self, text: &str) {
        let old = self.rope.to_string();
        if old == text {
            return;
        }
        let cursor_before = self.cursor;
        self.rope = Rope::from_str(text);
        let line = self.cursor.line.min(self.line_count().saturating_sub(1));
        self.cursor = Cursor {
            line,
            col: self.cursor.col.min(self.line_len(line)),
        };
        self.anchor = None;
        self.push_edit(Edit {
            char_idx: 0,
            before: old,
            after: text.to_string(),
            cursor_before,
            cursor_after: self.cursor,
            stamp: Instant::now(),
            typing: false,
        });
    }

    /// Deletes the selected text (recorded as a single undo operation). Returns false if there is no selection.
    pub fn delete_selection(&mut self) -> bool {
        self.delete_selection_internal()
    }

    /// Moves the cursor to the start of the given line (for search results / goto).
    pub fn goto_line(&mut self, line: usize) {
        self.anchor = None;
        let line = line.min(self.line_count().saturating_sub(1));
        self.cursor = Cursor { line, col: 0 };
    }

    /// Scrolls so the cursor line sits (roughly) in the vertical center.
    pub fn center_cursor(&mut self, height: usize) {
        self.scroll_y = self.cursor.line.saturating_sub(height / 2);
    }

    /// Adjusts scroll to keep the cursor visible given the viewport height.
    pub fn ensure_visible(&mut self, height: usize, width: usize) {
        if height == 0 {
            return;
        }
        if self.cursor.line < self.scroll_y {
            self.scroll_y = self.cursor.line;
        } else if self.cursor.line >= self.scroll_y + height {
            self.scroll_y = self.cursor.line + 1 - height;
        }
        if width > 0 {
            if self.cursor.col < self.scroll_x {
                self.scroll_x = self.cursor.col;
            } else if self.cursor.col >= self.scroll_x + width {
                self.scroll_x = self.cursor.col + 1 - width;
            }
        }
    }
}

/// Number of leading space/tab characters on a line.
fn leading_ws(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// How many leading characters one dedent removes: a single leading tab, or up
/// to four leading spaces. Zero when the line has no leading whitespace.
fn leading_indent_width(line: &str) -> usize {
    let mut chars = line.chars();
    match chars.next() {
        Some('\t') => 1,
        Some(' ') => 1 + chars.take(3).take_while(|c| *c == ' ').count(),
        _ => 0,
    }
}

/// Pretty-prints a pasted minified JSON blob (e.g. copied from a browser's
/// network tab) so it lands readable instead of as one giant line. Fires only
/// when the paste is, as a whole:
/// - a single line of at least [`MIN_JSON_PASTE`] chars — short snippets like
///   `{"a":1}` or `[1, 2, 3]` typed into code stay verbatim;
/// - a JSON object, or a non-empty array of objects (never a list of scalars);
/// - losslessly representable: re-serializing the parsed value compactly must
///   reproduce the input minus insignificant whitespace, so duplicate keys,
///   escape spellings (`\u00e9`) or number formats are never silently changed.
///   (serde_json's `preserve_order` + `arbitrary_precision` keep key order and
///   big/precise numbers intact.)
///
/// Deliberately unconditional (not behind `format_on_paste`): it only ever
/// fires on strictly valid, round-trippable JSON.
fn pretty_print_json(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.contains('\n')
        || trimmed.chars().count() < MIN_JSON_PASTE
        || !(trimmed.starts_with('{') || trimmed.starts_with('['))
    {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let shape_ok = match &value {
        serde_json::Value::Object(_) => true,
        serde_json::Value::Array(items) => {
            !items.is_empty() && items.iter().all(serde_json::Value::is_object)
        }
        _ => false,
    };
    if !shape_ok || serde_json::to_string(&value).ok()? != strip_json_whitespace(trimmed) {
        return None;
    }
    serde_json::to_string_pretty(&value).ok()
}

/// Shortest paste [`pretty_print_json`] will reformat.
const MIN_JSON_PASTE: usize = 64;

/// `text` with every whitespace char outside JSON string literals removed.
fn strip_json_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut in_str, mut escaped) = (false, false);
    for c in text.chars() {
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if !c.is_whitespace() {
            out.push(c);
        }
    }
    out
}

/// Re-indents a multi-line paste. The first line is left verbatim (the cursor
/// already supplies its indent). Every continuation line has the block's shared
/// minimum indentation stripped and `base` — the indentation of the line the
/// paste lands on — prefixed instead. Blank lines stay empty.
fn reindent_paste(text: &str, base: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    // Shared indentation is measured across the continuation lines only; the
    // first line often has none (selection started mid-line) and would wrongly
    // pin the minimum to zero.
    let common = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| leading_ws(l))
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if i == 0 {
            out.push_str(line); // verbatim
        } else if line.trim().is_empty() {
            continue; // keep blank lines empty (no trailing indent)
        } else {
            out.push_str(base);
            out.extend(line.chars().skip(common)); // drop shared indent
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_offsets_handle_emoji() {
        // "a😀b": 'a'=1 utf16, '😀'=2 utf16 (surrogate pair), 'b'=1 utf16.
        let b = Buffer::new(None, "a😀b");
        // char col -> utf16
        assert_eq!(b.char_col_to_utf16(0, 0), 0);
        assert_eq!(b.char_col_to_utf16(0, 1), 1); // after 'a'
        assert_eq!(b.char_col_to_utf16(0, 2), 3); // after emoji (1 + 2)
        assert_eq!(b.char_col_to_utf16(0, 3), 4); // after 'b'
        // utf16 -> char col (round trip)
        assert_eq!(b.utf16_to_char_col(0, 0), 0);
        assert_eq!(b.utf16_to_char_col(0, 1), 1);
        assert_eq!(b.utf16_to_char_col(0, 3), 2);
        assert_eq!(b.utf16_to_char_col(0, 4), 3);
        // Out-of-range utf16 clamps to line end.
        assert_eq!(b.utf16_to_char_col(0, 99), 3);
    }

    #[test]
    fn detects_and_caches_document_line_endings() {
        let crlf = Buffer::new(None, "one\r\ntwo\r\n");
        assert_eq!(crlf.line_ending_label(), "CRLF");

        let lf = Buffer::new(None, "one\ntwo\n");
        assert_eq!(lf.line_ending_label(), "LF");

        let mixed = Buffer::new(None, "one\r\ntwo\n");
        assert_eq!(mixed.line_ending_label(), "Mixed");

        let no_breaks = Buffer::new(None, "one");
        assert_eq!(no_breaks.line_ending_label(), "");
    }

    #[test]
    fn line_ending_label_refreshes_after_edit() {
        let mut buffer = Buffer::new(None, "one\r\ntwo\r\n");
        assert_eq!(buffer.line_ending_label(), "CRLF");
        buffer.replace_all("one\ntwo\n");
        assert_eq!(buffer.line_ending_label(), "LF");
    }

    #[test]
    fn anchor_selection_replaces_prefix() {
        // Completion accept: select the typed prefix, then insert replaces it.
        let mut b = Buffer::new(None, "prin");
        b.cursor = Cursor { line: 0, col: 4 };
        b.anchor = Some(Cursor { line: 0, col: 0 });
        b.insert_str("println!");
        assert_eq!(b.full_text(), "println!");
        assert_eq!(b.cursor.col, 8);
    }

    #[test]
    fn insert_and_undo() {
        let mut b = Buffer::scratch();
        for c in "hello".chars() {
            b.insert_char(c);
        }
        assert_eq!(b.full_text(), "hello");
        b.undo();
        assert_eq!(b.full_text(), "");
        b.redo();
        assert_eq!(b.full_text(), "hello");
    }

    #[test]
    fn newline_keeps_indentation() {
        let mut b = Buffer::new(None, "    let x = 1;");
        b.cursor = Cursor { line: 0, col: 14 }; // end of line
        b.insert_newline();
        assert_eq!(b.full_text(), "    let x = 1;\n    ");
        assert_eq!(b.cursor, Cursor { line: 1, col: 4 });
    }

    #[test]
    fn newline_indent_splits_line_at_cursor() {
        let mut b = Buffer::new(None, "\tfoobar");
        b.cursor = Cursor { line: 0, col: 4 }; // between "foo" and "bar"
        b.insert_newline();
        assert_eq!(b.full_text(), "\tfoo\n\tbar");
    }

    #[test]
    fn newline_inside_indent_carries_only_what_cursor_passed() {
        let mut b = Buffer::new(None, "        x");
        b.cursor = Cursor { line: 0, col: 4 }; // inside the 8-space indent
        b.insert_newline();
        assert_eq!(b.full_text(), "    \n        x");
    }

    #[test]
    fn newline_on_unindented_line_adds_nothing() {
        let mut b = Buffer::new(None, "x");
        b.cursor = Cursor { line: 0, col: 1 };
        b.insert_newline();
        assert_eq!(b.full_text(), "x\n");
    }

    #[test]
    fn newline_indent_undoes_as_one_step() {
        let mut b = Buffer::new(None, "    ab");
        b.cursor = Cursor { line: 0, col: 6 };
        b.insert_newline();
        assert_eq!(b.full_text(), "    ab\n    ");
        b.undo();
        assert_eq!(
            b.full_text(),
            "    ab",
            "the break and its indent undo together"
        );
    }

    #[test]
    fn indent_selection_indents_all_spanned_lines() {
        let mut b = Buffer::new(None, "one\ntwo\nthree\n");
        b.cursor = Cursor { line: 1, col: 3 };
        b.anchor = Some(Cursor { line: 0, col: 0 }); // select lines 0..=1
        assert!(b.selection_is_multiline());
        b.indent_selection();
        assert_eq!(b.full_text(), "    one\n    two\nthree\n");
        // Cursor rides along by one indent unit.
        assert_eq!(b.cursor, Cursor { line: 1, col: 7 });
        // Undoes as a single step.
        b.undo();
        assert_eq!(b.full_text(), "one\ntwo\nthree\n");
    }

    #[test]
    fn indent_selection_skips_blank_lines() {
        let mut b = Buffer::new(None, "one\n\ntwo\n");
        b.cursor = Cursor { line: 2, col: 3 }; // through the end of "two"
        b.anchor = Some(Cursor { line: 0, col: 0 });
        b.indent_selection();
        // The blank middle line gains no trailing whitespace.
        assert_eq!(b.full_text(), "    one\n\n    two\n");
    }

    #[test]
    fn dedent_selection_removes_one_unit() {
        let mut b = Buffer::new(None, "        a\n    b\n\tc\n");
        b.cursor = Cursor { line: 2, col: 2 }; // through the end of "\tc"
        b.anchor = Some(Cursor { line: 0, col: 0 });
        b.dedent_selection();
        // 4 spaces removed, 4 spaces removed (down to zero), one tab removed.
        assert_eq!(b.full_text(), "    a\nb\nc\n");
    }

    #[test]
    fn dedent_selection_no_leading_ws_is_noop() {
        let mut b = Buffer::new(None, "a\nb\n");
        b.cursor = Cursor { line: 1, col: 1 };
        b.anchor = Some(Cursor { line: 0, col: 0 });
        let before = b.version;
        b.dedent_selection();
        assert_eq!(b.full_text(), "a\nb\n");
        assert_eq!(b.version, before, "no edit, no version bump");
    }

    #[test]
    fn single_line_selection_is_not_multiline() {
        let mut b = Buffer::new(None, "hello\n");
        b.cursor = Cursor { line: 0, col: 5 };
        b.anchor = Some(Cursor { line: 0, col: 0 });
        assert!(!b.selection_is_multiline());
    }

    #[test]
    fn selection_ending_at_col_zero_excludes_last_line() {
        let mut b = Buffer::new(None, "one\ntwo\nthree\n");
        b.cursor = Cursor { line: 2, col: 0 }; // caret at start of line 2
        b.anchor = Some(Cursor { line: 0, col: 0 });
        b.indent_selection();
        // Line 2 is not visually selected, so it is left alone.
        assert_eq!(b.full_text(), "    one\n    two\nthree\n");
    }

    #[test]
    fn newline_replaces_selection_then_indents() {
        let mut b = Buffer::new(None, "    abcd");
        b.cursor = Cursor { line: 0, col: 8 };
        b.anchor = Some(Cursor { line: 0, col: 6 }); // select "cd"
        b.insert_newline();
        assert_eq!(b.full_text(), "    ab\n    ");
    }

    #[test]
    fn newline_splits_undo_groups() {
        let mut b = Buffer::scratch();
        for c in "ab".chars() {
            b.insert_char(c);
        }
        b.insert_newline();
        for c in "cd".chars() {
            b.insert_char(c);
        }
        assert_eq!(b.full_text(), "ab\ncd");
        b.undo(); // cd
        assert_eq!(b.full_text(), "ab\n");
        b.undo(); // newline
        assert_eq!(b.full_text(), "ab");
        b.undo(); // ab
        assert_eq!(b.full_text(), "");
    }

    #[test]
    fn replace_all_is_undoable() {
        let mut b = Buffer::new(None, "a  \nb\n");
        b.replace_all("a\nb\n");
        assert_eq!(b.full_text(), "a\nb\n");
        assert!(b.dirty);
        b.undo();
        assert_eq!(b.full_text(), "a  \nb\n");
        // No-op when unchanged: no new undo step.
        b.replace_all("a  \nb\n");
        b.undo();
        assert_eq!(b.full_text(), "a  \nb\n");
    }

    #[test]
    fn double_click_selects_word() {
        let mut b = Buffer::new(None, "foo bar_baz qux");
        b.select_word_at(Cursor { line: 0, col: 5 }); // inside "bar_baz"
        assert_eq!(b.selected_text().as_deref(), Some("bar_baz"));
        // Clicking on whitespace selects nothing.
        b.clear_selection();
        b.select_word_at(Cursor { line: 0, col: 3 });
        assert!(b.selected_text().is_none());
    }

    #[test]
    fn select_char_range_spans_lines() {
        let b0 = Buffer::new(None, "abc\ndef");
        let mut b = b0;
        b.select_char_range(1, 5); // "bc\nd"
        assert_eq!(b.selected_text().as_deref(), Some("bc\nd"));
    }

    #[test]
    fn word_motion() {
        let mut b = Buffer::new(None, "foo bar_baz  qux");
        b.move_word_right(false); // start -> after "foo"
        assert_eq!(b.cursor, Cursor { line: 0, col: 3 });
        b.move_word_right(false); // -> after "bar_baz"
        assert_eq!(b.cursor, Cursor { line: 0, col: 11 });
        b.move_word_left(false); // back to start of "bar_baz"
        assert_eq!(b.cursor, Cursor { line: 0, col: 4 });
        // Shift extends: anchor stays put.
        b.move_word_right(true);
        assert_eq!(b.selected_text().as_deref(), Some("bar_baz"));
    }

    #[test]
    fn smart_home_toggles() {
        let mut b = Buffer::new(None, "    foo");
        b.cursor = Cursor { line: 0, col: 7 }; // end of line
        b.move_home(false); // -> first non-ws
        assert_eq!(b.cursor.col, 4);
        b.move_home(false); // already there -> column 0
        assert_eq!(b.cursor.col, 0);
        b.move_home(false); // back to first non-ws
        assert_eq!(b.cursor.col, 4);
        // A line without indent just goes to 0.
        let mut b2 = Buffer::new(None, "bar");
        b2.cursor = Cursor { line: 0, col: 2 };
        b2.move_home(false);
        assert_eq!(b2.cursor.col, 0);
    }

    #[test]
    fn move_line_down_and_up() {
        let mut b = Buffer::new(None, "a\nb\nc");
        b.cursor = Cursor { line: 0, col: 0 }; // on "a"
        b.move_lines(1);
        assert_eq!(b.full_text(), "b\na\nc");
        assert_eq!(b.cursor.line, 1); // cursor followed the line
        b.move_lines(-1);
        assert_eq!(b.full_text(), "a\nb\nc");
        assert_eq!(b.cursor.line, 0);
    }

    #[test]
    fn move_line_edges_are_noops() {
        let mut b = Buffer::new(None, "a\nb");
        b.cursor = Cursor { line: 0, col: 0 };
        b.move_lines(-1); // already at top
        assert_eq!(b.full_text(), "a\nb");
        b.cursor = Cursor { line: 1, col: 0 };
        b.move_lines(1); // already at bottom
        assert_eq!(b.full_text(), "a\nb");
    }

    #[test]
    fn move_line_undoes_as_one_step() {
        let mut b = Buffer::new(None, "a\nb\nc");
        b.cursor = Cursor { line: 1, col: 0 };
        b.move_lines(1);
        assert_eq!(b.full_text(), "a\nc\nb");
        b.undo();
        assert_eq!(b.full_text(), "a\nb\nc");
    }

    #[test]
    fn move_selected_block() {
        let mut b = Buffer::new(None, "a\nb\nc\nd");
        // Select lines "a" and "b" (anchor on line 0, cursor at start of line 2).
        b.anchor = Some(Cursor { line: 0, col: 0 });
        b.cursor = Cursor { line: 2, col: 0 };
        b.move_lines(1);
        // Trailing col-0 line (2) is excluded; only a,b move down past c.
        assert_eq!(b.full_text(), "c\na\nb\nd");
    }

    #[test]
    fn paste_reindents_block_to_cursor() {
        // Pasting a block copied at 8-space indent onto a 4-space line.
        let mut b = Buffer::new(None, "    ");
        b.cursor = Cursor { line: 0, col: 4 }; // after the 4-space indent
        b.insert_paste("foo();\n        bar();\n        baz();");
        assert_eq!(b.full_text(), "    foo();\n    bar();\n    baz();");
    }

    #[test]
    fn paste_single_line_is_verbatim() {
        let mut b = Buffer::new(None, "    ");
        b.cursor = Cursor { line: 0, col: 4 };
        b.insert_paste("    x");
        assert_eq!(b.full_text(), "        x");
    }

    #[test]
    fn paste_keeps_blank_lines_empty() {
        let mut b = Buffer::new(None, "  ");
        b.cursor = Cursor { line: 0, col: 2 };
        b.insert_paste("a\n\n    b");
        assert_eq!(b.full_text(), "  a\n\n  b");
    }

    #[test]
    fn paste_pretty_prints_minified_json_object() {
        // A minified JSON blob (e.g. copied from a browser's network tab) is a
        // single line with no newline — it must still get pretty-printed rather
        // than land as one giant line. Key order and a number beyond u64 survive.
        let mut b = Buffer::new(None, "");
        b.insert_paste(
            r#"{"zeta":"coder","alpha":"0.2.0","tags":["editor","tui"],"big":123456789012345678901234567890}"#,
        );
        assert_eq!(
            b.full_text(),
            "{\n  \"zeta\": \"coder\",\n  \"alpha\": \"0.2.0\",\n  \"tags\": [\n    \"editor\",\n    \"tui\"\n  ],\n  \"big\": 123456789012345678901234567890\n}"
        );
    }

    #[test]
    fn paste_pretty_printed_json_reindents_to_cursor() {
        // The pretty-print result then goes through the normal reindent path,
        // so it still lands at the cursor's indentation, not column 0.
        let mut b = Buffer::new(None, "    ");
        b.cursor = Cursor { line: 0, col: 4 };
        b.insert_paste(r#"{"a":"0123456789012345678901234567890123456789","b":"0123456789"}"#);
        assert_eq!(
            b.full_text(),
            "    {\n      \"a\": \"0123456789012345678901234567890123456789\",\n      \"b\": \"0123456789\"\n    }"
        );
    }

    #[test]
    fn paste_leaves_short_scalar_lists_and_lossy_json_alone() {
        let long_list = format!(
            "[{}]",
            (0..40)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for text in [
            r#"{"a":1,"b":[2,3]}"#.to_string(), // too short to be a "blob"
            long_list,                          // array of scalars
            // Duplicate key: parsing would drop one.
            r#"{"key":"aaaaaaaaaaaaaaaaaaaaaaaa","key":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#
                .to_string(),
            // Escape spelling would change (é -> é).
            r#"{"name":"café café café café","other":"xxxxxxxxxxxxxxxx"}"#.to_string(),
        ] {
            let mut b = Buffer::new(None, "");
            b.insert_paste(&text);
            assert_eq!(b.full_text(), text);
        }
    }

    #[test]
    fn only_line_feed_breaks_lines() {
        // Lone CR, form feed, NEL and U+2028 are ordinary chars, matching
        // str::lines, syntect and LSP line numbering.
        let b = Buffer::new(None, "a\rb\u{c}c\u{85}d\u{2028}e\nf");
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line_text(1), "f");
        let crlf = Buffer::new(None, "one\r\ntwo\r\n");
        assert_eq!(crlf.line_count(), 3);
        assert_eq!(crlf.line_text(0), "one");
        assert_eq!(crlf.line_len(0), 3);
    }

    #[test]
    fn crlf_break_is_one_unit_for_backspace_and_delete() {
        let mut b = Buffer::new(None, "ab\r\ncd");
        b.cursor = Cursor { line: 1, col: 0 };
        b.backspace();
        assert_eq!(b.full_text(), "abcd");
        assert_eq!(b.cursor, Cursor { line: 0, col: 2 });

        let mut b = Buffer::new(None, "ab\r\ncd");
        b.cursor = Cursor { line: 0, col: 2 };
        b.delete_forward();
        assert_eq!(b.full_text(), "abcd");
        b.undo();
        assert_eq!(b.full_text(), "ab\r\ncd");
    }

    #[test]
    fn crlf_files_stay_crlf_when_editing_lines() {
        let mut b = Buffer::new(None, "  a\r\nb\r\nc");
        b.cursor = Cursor { line: 0, col: 3 };
        b.insert_newline();
        assert_eq!(b.full_text(), "  a\r\n  \r\nb\r\nc");

        // The unterminated last line borrows the line above's CRLF.
        let mut b = Buffer::new(None, "a\r\nb");
        b.cursor = Cursor { line: 1, col: 1 };
        b.insert_newline();
        assert_eq!(b.full_text(), "a\r\nb\r\n");

        let mut b = Buffer::new(None, "a\r\nb\r\nc");
        b.cursor = Cursor { line: 2, col: 0 };
        b.move_lines(-1);
        assert_eq!(b.full_text(), "a\r\nc\r\nb");
        b.move_lines(-1);
        assert_eq!(b.full_text(), "c\r\na\r\nb");

        let mut b = Buffer::new(None, "a\r\nb\r\n");
        b.select_all();
        b.indent_selection();
        assert_eq!(b.full_text(), "    a\r\n    b\r\n");
        b.dedent_selection();
        assert_eq!(b.full_text(), "a\r\nb\r\n");
    }

    #[test]
    fn paste_does_not_reformat_non_json_braces() {
        // Text that merely starts with '{' but isn't valid JSON (unquoted key,
        // trailing semicolon — a Rust/JS block) must be left completely alone.
        let mut b = Buffer::new(None, "");
        b.insert_paste("{ let x = 1; }");
        assert_eq!(b.full_text(), "{ let x = 1; }");
    }

    #[test]
    fn paste_does_not_reformat_bare_json_scalars() {
        // A bare word that happens to parse as a JSON scalar (not an object/
        // array) is left verbatim — pretty-printing a lone `true` would be a
        // silent no-op anyway, but this confirms the '{'/'[' gate skips it.
        let mut b = Buffer::new(None, "");
        b.insert_paste("null");
        assert_eq!(b.full_text(), "null");
    }

    #[test]
    fn selection_delete() {
        let mut b = Buffer::new(None, "hello world");
        b.move_right(false);
        b.move_right(true);
        b.move_right(true);
        assert_eq!(b.selected_text().as_deref(), Some("el"));
        b.backspace();
        assert_eq!(b.full_text(), "hlo world");
    }
}
