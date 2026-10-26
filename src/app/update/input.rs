//! Input routing: which overlay / text field / keymap a key, mouse event or
//! bracketed paste goes to.

use super::*;

/// A key press: the topmost overlay captures it, then the completion popup,
/// then a focused text input, then the keymap.
pub(super) fn key(model: &mut Model, key: KeyEvent) -> Vec<Cmd> {
    // The quickbar (command palette) is the topmost overlay: it captures
    // every key while open.
    if model.quickbar.is_some() {
        return quickbar_key(model, key);
    }
    // If a modal dialog is open it captures all keyboard input.
    if model.dialog.is_some() {
        return dialog_key(model, key);
    }
    // The file-tree context menu captures input the same way.
    if model.context_menu.is_some() {
        return menu_key(model, key);
    }
    // The completion popup (editor sub-mode) gets first refusal on keys.
    if model.completion.is_some()
        && let Some(cmds) = lsp::completion_key(model, key)
    {
        return cmds;
    }
    // A focused text input handles its own editing/motion keys first and
    // reports back what it did; only keys it ignores fall through to the
    // keymap (Enter/Tab/Esc, Ctrl-shortcuts, match/result navigation).
    if let Some((input, multiline)) = focused_input(model) {
        match input.handle_key(key, multiline) {
            InputOutcome::Ignored => {}
            InputOutcome::Moved => return Vec::new(),
            InputOutcome::Changed => {
                // A changed find query waits for Enter before searching.
                if model.focus == Focus::Find && model.find.field == FindField::Query {
                    invalidate_find(model);
                }
                return Vec::new();
            }
        }
    }
    if let Some(action) = keymap::resolve(&model.keybindings, key, model.focus, model.leader) {
        // Consume the leader latch once a command has fired (it may have just
        // been used to unlock a locked command). The Leader key itself re-arms it.
        if model.leader && !matches!(action, Action::Leader) {
            model.leader = false;
        }

        return apply_action(model, action);
    }
    Vec::new()
}

/// A mouse event: an open overlay captures it, else normal hit-testing.
pub(super) fn mouse(model: &mut Model, m: MouseEvent) -> Vec<Cmd> {
    if model.quickbar.is_some() {
        return quickbar_mouse(model, m);
    }
    if model.dialog.is_some() {
        return dialog_mouse(model, m);
    }
    if model.context_menu.is_some() {
        return menu_mouse(model, m);
    }
    // The completion popup is keyboard-only: any click dismisses it (else it
    // would follow the caret to the clicked spot), and drops a pending
    // auto-trigger so it can't reopen there either.
    if matches!(m.kind, MouseEventKind::Down(_)) {
        model.completion = None;
        model.lsp.completion_request = None;
        model.cancel_autocomplete();
    }
    handle_mouse(model, m)
}

/// A terminal bracketed paste, routed the same way `Msg::Key` cascades
/// through overlays: whichever one currently owns input gets the text.
/// Falling through to nothing (e.g. `Focus::Sidebar`) is deliberate — it
/// is also what keeps a stray paste from firing single-letter shortcuts
/// (Git panel `a`/`r` stage/revert) one keystroke at a time, which is
/// what happened before bracketed paste existed.
pub(super) fn paste(model: &mut Model, text: String) -> Vec<Cmd> {
    if model.quickbar.is_some() {
        return quickbar_paste(model, &text);
    }
    if model.dialog.is_some() {
        return dialog_paste(model, &text);
    }
    if model.context_menu.is_some() {
        return Vec::new();
    }
    if let Some((input, multiline)) = focused_input(model) {
        input.insert_paste(&text, multiline);
        if model.focus == Focus::Find && model.find.field == FindField::Query {
            invalidate_find(model);
        }
        return Vec::new();
    }
    if model.focus == Focus::Terminal {
        return paste_into_terminal(model, &text);
    }
    paste_into_editor(model, &text)
}

/// The text input the current focus routes keys to, and whether it is multi-line.
/// `None` when focus is not on a text field.
fn focused_input(model: &mut Model) -> Option<(&mut TextInputState, bool)> {
    match model.focus {
        Focus::Find => {
            let f = match model.find.field {
                FindField::Query => &mut model.find.query,
                FindField::Replace => &mut model.find.replace,
            };
            Some((f, false))
        }
        Focus::SearchInput => {
            let s = match model.sidebar.search.field {
                SearchField::Query => &mut model.sidebar.search.query,
                SearchField::Replace => &mut model.sidebar.search.replace,
                // A checkbox has keyboard focus: no text field takes the key.
                _ => return None,
            };
            Some((s, false))
        }
        Focus::GitCommit => Some((&mut model.sidebar.git.commit, true)),
        _ => None,
    }
}
