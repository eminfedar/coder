//! Editor mutation helpers, viewport geometry, and format-on-save.

use super::*;

/// Applies an editor mutation and keeps the cursor visible.
pub(super) fn edit(model: &mut Model, f: impl FnOnce(&mut Buffer)) -> Vec<Cmd> {
    // Typing, motion and clipboard edits belong to the editor alone. While focus
    // sits in the sidebar, terminal, find widget or commit box, a key that the
    // focused widget ignored must never fall through into the buffer.
    if model.focus != Focus::Editor {
        return Vec::new();
    }
    // Read-only notice tabs (binary / unreadable files) never accept edits.
    if model.active_notice().is_some() {
        return Vec::new();
    }
    let before = model.active_buffer().map(|b| b.version);
    if let Some(buf) = model.active_buffer_mut() {
        f(buf);
    }
    ensure_cursor_visible(model);
    // Only tell the language server when the text actually changed (skip motions).
    if before != model.active_buffer().map(|b| b.version) {
        // Editing invalidates any find matches: their char ranges were computed
        // against the old text and would index past the new (shorter) rope,
        // panicking char_to_line at render time. Recompute if the widget is
        // open (quietly: typing must not jump to a match), otherwise drop them.
        if model.find.open {
            super::find::sync_find(model);
        } else if !model.find.matches.is_empty() {
            model.find.matches.clear();
            model.find.current = None;
        }
        // The change-gutter git diff is intentionally NOT refreshed here: it stays
        // frozen at its last state while editing and only recomputes on save /
        // reload / disk change (see `Model::refresh_git_marks`).
        // Debounced session checkpoint, so a crash (or a kill) never loses more
        // than a couple seconds of typing — see `Model::schedule_session_save`.
        model.schedule_session_save();
        super::lsp::notify_change(model)
    } else {
        Vec::new()
    }
}

/// Like [`edit`], but for changes to the *text*: refused on read-only tabs
/// (generated content such as a commit patch). Cursor motion still goes through
/// `edit`, so a read-only tab stays fully navigable — it just cannot be typed in.
pub(super) fn mutate(model: &mut Model, f: impl FnOnce(&mut Buffer)) -> Vec<Cmd> {
    if model.active_read_only() {
        return Vec::new();
    }
    // Editing a preview tab keeps it: the next preview opens its own tab.
    if let Some(i) = model.active_tab {
        model.tabs[i].preview = false;
    }
    edit(model, f)
}

/// Applies the enabled format-on-save actions to the active buffer (before writing).
pub(super) fn apply_format_on_save(model: &mut Model) {
    let s = &model.sidebar.settings;
    if !s.format_on_save {
        return;
    }
    let trim = s.trim_trailing_whitespace;
    let final_nl = s.insert_final_newline;
    if let Some(buf) = model.active_buffer_mut() {
        let text = buf.full_text();
        let formatted = format_text(&text, trim, final_nl);
        // replace_all is a no-op when unchanged and bumps the version so the
        // highlight cache refreshes on its own.
        buf.replace_all(&formatted);
    }
    ensure_cursor_visible(model);
}

/// Trims trailing whitespace per line and/or ensures a single final newline.
pub(super) fn format_text(text: &str, trim: bool, final_nl: bool) -> String {
    let mut result = if trim {
        text.split('\n')
            .map(|l| l.trim_end_matches([' ', '\t']))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        text.to_string()
    };
    if final_nl && !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

pub(super) fn apply_motion(b: &mut Buffer, motion: Motion, extend: bool, page: usize) {
    match motion {
        Motion::Left => b.move_left(extend),
        Motion::Right => b.move_right(extend),
        Motion::Up => b.move_up(extend),
        Motion::Down => b.move_down(extend),
        Motion::Home => b.move_home(extend),
        Motion::End => b.move_end(extend),
        Motion::PageUp => b.move_page(-(page as isize), extend),
        Motion::PageDown => b.move_page(page as isize, extend),
        Motion::WordLeft => b.move_word_left(extend),
        Motion::WordRight => b.move_word_right(extend),
    }
}

/// Pastes `text` into the active buffer (a no-op outside `Focus::Editor` or on
/// a read-only tab — see `mutate`), then runs the language formatter over the
/// whole document when format-on-paste is enabled. Shared by `Action::Paste`
/// (Ctrl+V, reads the system/OSC-52 clipboard) and `Msg::Paste` (a terminal
/// bracketed paste, which already carries the text) so both behave identically.
pub(super) fn paste_into_editor(model: &mut Model, text: &str) -> Vec<Cmd> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut cmds = mutate(model, |b| b.insert_paste(text));
    if model.sidebar.settings.format_on_paste {
        cmds.extend(super::lsp::request_format(model, false));
    }
    cmds
}

/// Computes the editor viewport size (height, text width).
pub(super) fn editor_viewport(model: &Model) -> (usize, usize) {
    let area = full_rect(model);
    let a = ui::compute_areas(model, area);
    let gutter = a.gutter_w;
    (
        a.editor.height as usize,
        a.editor.width.saturating_sub(gutter) as usize,
    )
}

pub(super) fn full_rect(model: &Model) -> Rect {
    Rect {
        x: 0,
        y: 0,
        width: model.term_size.0,
        height: model.term_size.1,
    }
}

pub(super) fn ensure_cursor_visible(model: &mut Model) {
    let (h, w) = editor_viewport(model);
    if let Some(buf) = model.active_buffer_mut() {
        buf.ensure_visible(h, w);
    }
}

/// Scrolls the active buffer so the cursor line is vertically centered.
pub(super) fn center_cursor_in_view(model: &mut Model) {
    let (h, w) = editor_viewport(model);
    if let Some(buf) = model.active_buffer_mut() {
        buf.center_cursor(h);
        buf.ensure_visible(h, w);
    }
}
