//! VSCode-style keyboard mapping: (KeyEvent, Focus) -> Action.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::model::{Focus, Panel};
use crate::services::keybindings::Keybindings;

/// High-level action to be applied by `update`.
pub enum Action {
    Quit,
    ToggleSidebar,
    ToggleTerminal,
    SelectPanel(Panel),
    Save,
    CloseTab,
    NextTab,
    PrevTab,

    // Editor
    Insert(char),
    Newline,
    InsertTab,
    Backspace,
    Delete,
    Move(Motion, bool), // (direction, extend selection)
    /// Alt+Up / Alt+Down: move the current line (or selected lines) up/down.
    MoveLineUp,
    MoveLineDown,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    /// Explicitly request a completion popup (Ctrl+Space).
    TriggerCompletion,
    /// Format the active buffer via its language server / formatter (Ctrl+Alt+F).
    Format,
    /// Open the editable keybindings file in the editor (Alt+7).
    ShowShortcuts,
    /// Open a fresh "Untitled-N" scratch buffer (Ctrl+N from the editor) — no
    /// backing file until it is saved. Distinct from `NewFile` below, which
    /// creates a real file on disk from the sidebar's file tree.
    NewUntitledFile,

    // Sidebar navigation
    NavUp,
    NavDown,
    Activate,

    // Git panel (Source Control)
    /// Tab / Shift+Tab: move the keyboard between the commit box, the buttons
    /// and the change list. The `isize` is the step through the zone order.
    GitCycleZone(isize),
    /// `a`: stage the selected change, or unstage it when it is already staged.
    GitToggleStage,
    /// `r`: revert the selected change back to its committed state.
    GitRevertEntry,

    /// `←` in the Files panel: collapse the selected folder, or jump to the
    /// parent folder's row.
    CollapseOrParent,

    // File tree entry management (Files panel)
    NewFile,
    NewFolder,
    RenameEntry,
    DeleteEntry,

    // Search input (text editing is handled by the focused input widget itself)
    SearchSubmit,
    /// Tab / Shift+Tab in the search panel: step through the inputs and the
    /// option checkboxes. The `isize` is the step through the Tab order.
    SearchCycleField(isize),
    SearchToggleRegex,

    // In-editor find / replace widget (text editing handled by the input widget)
    OpenFind,
    OpenFindReplace,

    // Command palette / quickbar overlay
    OpenQuickbar,
    FindNext,
    FindPrev,
    FindToggleField,

    // Terminal raw input
    PtyInput(Vec<u8>),

    /// Toggle the "leader" state: the next locked command fires directly instead
    /// of waiting for an explicit unlock (a nested/leader-key command mode).
    Leader,
    Escape,
}

#[derive(Clone, Copy)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    WordLeft,
    WordRight,
}

/// A character typed through AltGr. Windows (and some terminals) report AltGr
/// as Ctrl+Alt, so e.g. AltGr+Q on a German layout arrives as Ctrl+Alt+'@'.
/// Only non-alphanumeric, visible chars count: Ctrl+Alt+<letter/digit> stays a
/// shortcut (Ctrl+Alt+F = format). Callers consult this only after the user
/// keybindings had no match, so an explicit binding still wins.
pub fn altgr_char(key: &KeyEvent) -> Option<char> {
    let m = key.modifiers;
    match key.code {
        KeyCode::Char(c)
            if m.contains(KeyModifiers::CONTROL)
                && m.contains(KeyModifiers::ALT)
                && !c.is_ascii_alphanumeric()
                && !c.is_control()
                && !c.is_whitespace() =>
        {
            Some(c)
        }
        _ => None,
    }
}

pub fn resolve(keys: &Keybindings, key: KeyEvent, focus: Focus, unlock: bool) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    // User-editable command shortcuts (quit, save, copy, new file, ...) win first.
    // A locked shortcut is ignored until the leader is armed. In the editor,
    // an unmodified printable character then falls through as normal text input.
    if let Some(full) = keys.resolve_full(key, focus) {
        if !full.locked || unlock {
            return Some(full.action);
        }
        let printable_editor_key = focus == Focus::Editor
            && !ctrl
            && !key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Char(c) if !c.is_control());
        if !printable_editor_key {
            return None;
        }
    }

    match focus {
        Focus::Terminal => resolve_terminal(key, ctrl, shift),
        Focus::Editor => resolve_editor(key, ctrl, shift),
        Focus::Sidebar => resolve_sidebar(key, ctrl, shift),
        Focus::SearchInput => resolve_search(key),
        Focus::GitCommit => resolve_git_commit(key),
        Focus::Find => resolve_find(key, shift),
    }
}

fn resolve_find(key: KeyEvent, shift: bool) -> Option<Action> {
    // Editing/motion keys are consumed by the focused input widget upstream; only
    // find-specific keys reach here.
    match key.code {
        KeyCode::Tab => Some(Action::FindToggleField), // query <-> replace
        KeyCode::Enter => Some(if shift {
            Action::FindPrev
        } else {
            Action::FindNext
        }),
        // Up/Down step through matches (the inputs are single-line).
        KeyCode::Down => Some(Action::FindNext),
        KeyCode::Up => Some(Action::FindPrev),
        KeyCode::Esc => Some(Action::Escape),
        _ => None,
    }
}

fn resolve_git_commit(key: KeyEvent) -> Option<Action> {
    // The commit box is multi-line: the input widget consumes typing, motion, and
    // Enter (newline). Committing is button-only; Esc blurs back to the sidebar.
    match key.code {
        // Tab moves on to the Fetch button (and the rest of the panel).
        KeyCode::Tab => Some(Action::GitCycleZone(1)),
        KeyCode::BackTab => Some(Action::GitCycleZone(-1)),
        KeyCode::Esc => Some(Action::Escape),
        _ => None,
    }
}

fn resolve_editor(key: KeyEvent, ctrl: bool, shift: bool) -> Option<Action> {
    // Command shortcuts (copy/cut/paste/undo/format/move-line/…) are handled by
    // the user keybindings before we get here. What remains is the fixed set:
    // Ctrl+Left/Right word motion, plus typing / cursor motion / structural keys.
    if let Some(c) = altgr_char(&key) {
        return Some(Action::Insert(c));
    }
    if ctrl {
        return match key.code {
            KeyCode::Left => Some(Action::Move(Motion::WordLeft, shift)),
            KeyCode::Right => Some(Action::Move(Motion::WordRight, shift)),
            _ => None,
        };
    }
    // Alt+char is a shortcut (panel switching, …), not text: an unbound one is
    // dropped rather than typed into the buffer.
    if key.modifiers.contains(KeyModifiers::ALT) && matches!(key.code, KeyCode::Char(_)) {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(Action::Insert(c)),
        KeyCode::Enter => Some(Action::Newline),
        KeyCode::Tab => Some(Action::InsertTab),
        KeyCode::Backspace => Some(Action::Backspace),
        KeyCode::Delete => Some(Action::Delete),
        KeyCode::Left => Some(Action::Move(Motion::Left, shift)),
        KeyCode::Right => Some(Action::Move(Motion::Right, shift)),
        KeyCode::Up => Some(Action::Move(Motion::Up, shift)),
        KeyCode::Down => Some(Action::Move(Motion::Down, shift)),
        KeyCode::Home => Some(Action::Move(Motion::Home, shift)),
        KeyCode::End => Some(Action::Move(Motion::End, shift)),
        KeyCode::PageUp => Some(Action::Move(Motion::PageUp, shift)),
        KeyCode::PageDown => Some(Action::Move(Motion::PageDown, shift)),
        KeyCode::Esc => Some(Action::Escape),
        _ => None,
    }
}

fn resolve_sidebar(key: KeyEvent, ctrl: bool, _shift: bool) -> Option<Action> {
    // Entry management (new file/folder, delete, rename) is handled by the user
    // keybindings; only the fixed tree navigation keys remain here.
    // A bare letter only: Ctrl+A / Alt+A are shortcuts, not panel commands.
    let bare = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Up => Some(Action::NavUp),
        KeyCode::Down => Some(Action::NavDown),
        KeyCode::Enter | KeyCode::Right => Some(Action::Activate),
        KeyCode::Left => Some(Action::CollapseOrParent),
        // Files panel letter shortcuts: new file / new folder next to the
        // selected row. No-ops in the other panels (`on_selected_row`).
        KeyCode::Char('f') if bare => Some(Action::NewFile),
        KeyCode::Char('d') if bare => Some(Action::NewFolder),
        // Git panel letter shortcuts. They are no-ops in the other panels (see
        // `apply_action`), which have no letter keys of their own.
        KeyCode::Char('a') if bare => Some(Action::GitToggleStage),
        KeyCode::Char('r') if bare => Some(Action::GitRevertEntry),
        KeyCode::Tab if bare => Some(Action::GitCycleZone(1)),
        KeyCode::BackTab => Some(Action::GitCycleZone(-1)),
        KeyCode::Esc => Some(Action::Escape),
        _ => None,
    }
}

fn resolve_search(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl {
        // Ctrl+R: toggle regex.
        return match key.code {
            KeyCode::Char('r') => Some(Action::SearchToggleRegex),
            _ => None,
        };
    }
    // Typing/motion is consumed by the focused input widget upstream.
    match key.code {
        KeyCode::Tab => Some(Action::SearchCycleField(1)),
        KeyCode::BackTab => Some(Action::SearchCycleField(-1)),
        // Space only gets here on a checkbox: the text inputs consume it.
        KeyCode::Enter | KeyCode::Char(' ') => Some(Action::SearchSubmit),
        KeyCode::Up => Some(Action::NavUp),
        KeyCode::Down => Some(Action::NavDown),
        KeyCode::Esc => Some(Action::Escape),
        _ => None,
    }
}

/// When the terminal is focused, converts keys into raw bytes to send to the PTY.
fn resolve_terminal(key: KeyEvent, ctrl: bool, _shift: bool) -> Option<Action> {
    let bytes: Vec<u8> = match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                // Ctrl+letter -> control character (0x01..0x1a)
                let up = c.to_ascii_uppercase();
                if up.is_ascii_alphabetic() {
                    vec![(up as u8) - 0x40]
                } else {
                    let mut b = [0u8; 4];
                    c.encode_utf8(&mut b).as_bytes().to_vec()
                }
            } else {
                let mut b = [0u8; 4];
                c.encode_utf8(&mut b).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        _ => return None,
    };
    Some(Action::PtyInput(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn altgr_symbols_are_text_but_ctrl_alt_letters_are_not() {
        let ca = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert_eq!(
            altgr_char(&KeyEvent::new(KeyCode::Char('@'), ca)),
            Some('@')
        );
        assert_eq!(
            altgr_char(&KeyEvent::new(KeyCode::Char('€'), ca)),
            Some('€')
        );
        assert_eq!(altgr_char(&KeyEvent::new(KeyCode::Char('f'), ca)), None);
        assert_eq!(
            altgr_char(&KeyEvent::new(KeyCode::Char('@'), KeyModifiers::CONTROL)),
            None
        );
    }
}
