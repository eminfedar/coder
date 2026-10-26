//! In-editor find / replace widget logic.

use super::*;

/// Opens (or refocuses) the find widget. `replace` also shows the replace row.
pub(super) fn open_find(model: &mut Model, replace: bool) -> Vec<Cmd> {
    if model.active_tab.is_none() {
        return Vec::new();
    }
    model.find.open = true;
    if replace {
        model.find.replace_mode = true;
    }
    model.find.field = FindField::Query;
    // Prefill the query from a single-line selection (VSCode behavior).
    if let Some(sel) = model.active_buffer().and_then(|b| b.selected_text())
        && !sel.is_empty()
        && !sel.contains('\n')
        && sel != model.find.query.content()
    {
        model.find.query.set_content(sel);
        invalidate_find(model);
    }
    // Place the caret at the end of each field's current text.
    model.find.query.cursor_to_end();
    model.find.replace.cursor_to_end();
    model.focus = Focus::Find;
    // Nothing is searched until the query is submitted (`find_step`).
    Vec::new()
}

/// Closes the find widget and returns focus to the editor. Reopening it
/// starts unsearched again.
pub(super) fn close_find(model: &mut Model) {
    model.find.open = false;
    model.find.searched = false;
    model.focus = Focus::Editor;
}

/// The query text changed: drop the matches of the previous query. The new
/// one is searched only once it is submitted (Enter / next / prev).
pub(super) fn invalidate_find(model: &mut Model) {
    model.find.searched = false;
    model.find.matches.clear();
    model.find.current = None;
    model.find.matches_key = active_find_key(model);
}

/// Submits the query: computes its matches and selects the first one after
/// the cursor (`delta` > 0) or before it (`delta` < 0).
fn submit_find(model: &mut Model, delta: isize) {
    model.find.searched = true;
    compute_matches(model);
    if model.find.matches.is_empty() {
        model.find.current = None;
        return;
    }
    // Search from the start of a selection, so a query prefilled from the
    // selected text finds that occurrence first.
    let cur = model
        .active_buffer()
        .map(|b| b.selection_start_char_index())
        .unwrap_or(0);
    let m = &model.find.matches;
    let idx = if delta < 0 {
        m.iter().rposition(|(s, _)| *s < cur).unwrap_or(m.len() - 1)
    } else {
        m.iter().position(|(s, _)| *s >= cur).unwrap_or(0)
    };
    model.find.current = Some(idx);
    find_select_current(model);
}

/// Recomputes match positions for the submitted query and selects the match at
/// or after the cursor. Called after a replace rewrote the buffer.
pub(super) fn recompute_find(model: &mut Model) {
    compute_matches(model);
    if model.find.matches.is_empty() {
        model.find.current = None;
        if let Some(buf) = model.active_buffer_mut() {
            buf.clear_selection();
        }
        return;
    }
    let cur = model
        .active_buffer()
        .map(|b| b.cursor_char_index())
        .unwrap_or(0);
    let idx = model
        .find
        .matches
        .iter()
        .position(|(s, _)| *s >= cur)
        .unwrap_or(0);
    model.find.current = Some(idx);
    find_select_current(model);
}

/// Recomputes `find.matches` for the active buffer and records which
/// tab/version they belong to.
fn compute_matches(model: &mut Model) {
    let key = active_find_key(model);
    let matches = model
        .active_buffer()
        .filter(|_| model.find.searched)
        .map(|b| find_matches(&b.full_text(), model.find.query.content()))
        .unwrap_or_default();
    model.find.matches = matches;
    model.find.matches_key = key;
}

fn active_find_key(model: &Model) -> Option<(usize, u64)> {
    let t = model.tabs.get(model.active_tab?)?;
    Some((t.id, t.buffer.version))
}

/// Keeps the open find widget's matches in step with the active buffer after
/// any message: a tab switch, reload, format or cut leaves char ranges that
/// belong to other text (Replace would then edit arbitrary text). Recomputes
/// quietly — the current match is re-picked but the cursor is not moved.
pub(super) fn sync_find(model: &mut Model) {
    if model.find.matches_key == active_find_key(model) {
        return;
    }
    if !model.find.open {
        model.find.matches.clear();
        model.find.current = None;
        model.find.matches_key = None;
        return;
    }
    compute_matches(model);
    let cur = model
        .active_buffer()
        .map(|b| b.cursor_char_index())
        .unwrap_or(0);
    model.find.current = if model.find.matches.is_empty() {
        None
    } else {
        Some(
            model
                .find
                .matches
                .iter()
                .position(|(s, _)| *s >= cur)
                .unwrap_or(0),
        )
    };
}

/// Selects the current match in the buffer and scrolls it into view.
fn find_select_current(model: &mut Model) {
    let Some(i) = model.find.current else {
        return;
    };
    let Some(&(s, e)) = model.find.matches.get(i) else {
        return;
    };
    if let Some(buf) = model.active_buffer_mut() {
        buf.select_char_range(s, e);
    }
    // Center the found match in the viewport.
    center_cursor_in_view(model);
}

/// Moves to the next (delta=1) / previous (delta=-1) match, wrapping around.
/// The first step after the query changed submits it instead.
pub(super) fn find_step(model: &mut Model, delta: isize) {
    if !model.find.searched {
        submit_find(model, delta);
        return;
    }
    let n = model.find.matches.len();
    if n == 0 {
        return;
    }
    let cur = model.find.current.unwrap_or(0) as isize;
    model.find.current = Some((cur + delta).rem_euclid(n as isize) as usize);
    find_select_current(model);
}

/// Replaces the current match with the replacement text, then advances.
pub(super) fn find_replace_one(model: &mut Model) -> Vec<Cmd> {
    if model.active_read_only() {
        return Vec::new(); // generated content (a commit patch) is never rewritten
    }
    // Not searched yet: the first press finds (and selects) the match to replace.
    if !model.find.searched {
        submit_find(model, 1);
        return Vec::new();
    }
    let Some(i) = model.find.current else {
        return Vec::new();
    };
    let Some(&(s, e)) = model.find.matches.get(i) else {
        return Vec::new();
    };
    let rep = model.find.replace.content().to_string();
    if let Some(buf) = model.active_buffer_mut() {
        buf.select_char_range(s, e);
        buf.insert_str(&rep);
    }
    model.invalidate_highlight();
    // The cursor now sits just past the replacement; recompute selects the next match.
    recompute_find(model);
    Vec::new()
}

/// Replaces every match in the active buffer in a single undo step.
pub(super) fn find_replace_all(model: &mut Model) -> Vec<Cmd> {
    if model.active_read_only() {
        return Vec::new(); // generated content (a commit patch) is never rewritten
    }
    if model.find.query.is_empty() {
        return Vec::new();
    }
    let rep = model.find.replace.content().to_string();
    let Some((new_text, count)) = model
        .active_buffer()
        .map(|b| replace_all_text(&b.full_text(), model.find.query.content(), &rep))
    else {
        return Vec::new();
    };
    if count > 0 {
        if let Some(buf) = model.active_buffer_mut() {
            buf.replace_all(&new_text);
        }
        model.invalidate_highlight();
    }
    model.notify(format!("{count} replaced"));
    model.find.searched = true;
    recompute_find(model);
    Vec::new()
}

/// Case-insensitive (ASCII) literal match positions as [start, end) char indices.
/// Matches are non-overlapping.
fn find_matches(text: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return Vec::new();
    }
    // An escaped literal with ASCII-only case folding (`unicode(false)`): the
    // regex engine's substring search is linear, unlike a naive per-char scan,
    // and needs no `Vec<char>` copy of the whole file.
    let Ok(re) = regex::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .unicode(false)
        .build()
    else {
        return Vec::new();
    };
    let q_chars = query.chars().count();
    // Byte offsets -> char indices, counted incrementally between matches.
    let (mut byte, mut chars) = (0, 0);
    re.find_iter(text)
        .map(|m| {
            chars += text[byte..m.start()].chars().count();
            byte = m.end();
            let start = chars;
            chars += q_chars;
            (start, start + q_chars)
        })
        .collect()
}

/// Builds a new string with every match of `query` replaced by `rep`.
fn replace_all_text(text: &str, query: &str, rep: &str) -> (String, usize) {
    let matches = find_matches(text, query);
    if matches.is_empty() {
        return (text.to_string(), 0);
    }
    let t: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut last = 0;
    for &(s, e) in &matches {
        out.extend(&t[last..s]);
        out.push_str(rep);
        last = e;
    }
    out.extend(&t[last..]);
    (out, matches.len())
}

#[cfg(test)]
mod find_tests {
    use super::{find_matches, replace_all_text};

    #[test]
    fn matches_are_case_insensitive_and_non_overlapping() {
        assert_eq!(find_matches("aXaXa", "x"), vec![(1, 2), (3, 4)]);
        assert_eq!(find_matches("aaaa", "aa"), vec![(0, 2), (2, 4)]);
        assert!(find_matches("abc", "").is_empty());
        assert!(find_matches("abc", "abcd").is_empty());
    }

    #[test]
    fn matches_count_chars_not_bytes() {
        // Multi-byte text before/inside the match: indices are char offsets.
        assert_eq!(find_matches("çé X é", "é"), vec![(1, 2), (5, 6)]);
        assert_eq!(find_matches("😀ab😀AB", "ab"), vec![(1, 3), (4, 6)]);
        // Case folding is ASCII-only, like before.
        assert_eq!(find_matches("É é", "é"), vec![(2, 3)]);
    }

    #[test]
    fn replace_all_rebuilds_text() {
        assert_eq!(
            replace_all_text("foo Foo", "foo", "bar"),
            ("bar bar".to_string(), 2)
        );
        assert_eq!(replace_all_text("abc", "x", "y"), ("abc".to_string(), 0));
    }
}

#[cfg(test)]
mod find_flow_tests {
    use super::*;
    use crate::app::model::Tab;
    use crossterm::event::KeyModifiers;

    fn model_with(text: &str) -> Model {
        let mut model = Model::new(std::env::temp_dir());
        model
            .tabs
            .push(Tab::new(Buffer::new(Some("/w/a.rs".into()), text)));
        model.active_tab = Some(0);
        model
    }

    fn press(model: &mut Model, code: KeyCode) {
        super::super::input::key(model, KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn typing_the_query_neither_searches_nor_selects_until_enter() {
        let mut model = model_with("foo bar foo\n");
        open_find(&mut model, false);
        for c in "foo".chars() {
            press(&mut model, KeyCode::Char(c));
        }
        assert!(model.find.matches.is_empty());
        assert!(model.active_buffer().unwrap().selection_range().is_none());
        assert_eq!(model.find.count_label(), "-/-");

        press(&mut model, KeyCode::Enter);
        assert_eq!(model.find.matches, vec![(0, 3), (8, 11)]);
        assert_eq!(model.find.current, Some(0));
        assert!(model.active_buffer().unwrap().selection_range().is_some());

        press(&mut model, KeyCode::Enter);
        assert_eq!(model.find.current, Some(1));

        // Enter in the replace field replaces the current match, then the next.
        open_find(&mut model, true);
        model.find.field = FindField::Replace;
        press(&mut model, KeyCode::Char('Z'));
        press(&mut model, KeyCode::Enter);
        assert_eq!(model.active_buffer().unwrap().full_text(), "foo bar Z\n");
        assert_eq!(model.find.matches, vec![(0, 3)]);
        assert_eq!(model.find.current, Some(0));
        press(&mut model, KeyCode::Enter);
        assert_eq!(model.active_buffer().unwrap().full_text(), "Z bar Z\n");
        // Down only steps, it never replaces.
        model.find.field = FindField::Query;
        press(&mut model, KeyCode::Enter);
        model.find.field = FindField::Replace;
        press(&mut model, KeyCode::Down);
        assert_eq!(model.active_buffer().unwrap().full_text(), "Z bar Z\n");

        // Editing the query drops the old matches until the next Enter.
        model.find.field = FindField::Query;
        press(&mut model, KeyCode::Char('x'));
        assert!(model.find.matches.is_empty());
        assert_eq!(model.find.count_label(), "-/-");
    }
}
