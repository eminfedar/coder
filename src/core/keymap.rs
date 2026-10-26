//! VSCode-style keyboard mapping: (KeyEvent, Focus) -> Action.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::model::{Focus, Panel};
use crate::services::keybindings::Keybindings;

/// High-level action to be applied by `update`.
pub enum Action {
    Quit,
    ToggleSidebar,
    ToggleTerminal,
    /// Open another terminal tab in the bottom panel (Ctrl+Shift+J).
    NewTerminal,
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
    /// Enter in the find widget: next match from the query field, replace +
    /// next from the replace field.
    FindSubmit,
    FindNext,
    FindPrev,
    FindToggleField,

    /// A key for the focused terminal, encoded for the PTY in `update`.
    PtyKey(KeyEvent),

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
    // The locked flag is resolved here: a locked command only fires while the
    // leader (unlock) key is held, otherwise it returns `None` so the keystroke
    // falls through to typing/motion below.
    // A focused terminal gets every other key (htop's F10, a shell's Ctrl+W,
    // ...): only the terminal panel's own shortcuts still fire there.
    if let Some(full) = keys.resolve_full(key, focus)
        && (focus != Focus::Terminal
            || matches!(full.action, Action::ToggleTerminal | Action::NewTerminal))
    {
        if full.locked && !unlock {
            return None;
        }
        return Some(full.action);
    }

    match focus {
        Focus::Terminal => resolve_terminal(key),
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
            Action::FindSubmit
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

/// When the terminal is focused every key goes to the PTY; the bytes are
/// encoded in `update` (see `terminal_key_bytes`), which knows the screen modes.
fn resolve_terminal(key: KeyEvent) -> Option<Action> {
    Some(Action::PtyKey(key))
}

/// xterm encoding of a key press for the PTY, or `None` for keys with none.
/// `app_cursor` is DECCKM (application cursor keys, set by htop, vim, less):
/// unmodified arrows/Home/End then send `ESC O x` instead of `ESC [ x`.
pub fn terminal_key_bytes(key: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let m = key.modifiers;
    let ctrl = m.contains(KeyModifiers::CONTROL);
    let alt = m.contains(KeyModifiers::ALT);
    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    let param = 1 + m.contains(KeyModifiers::SHIFT) as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    // `ESC [ 1 ; m x` with modifiers, else `ESC [ x` / `ESC O x`.
    let letter = |x: char, ss3: bool| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[1;{param}{x}").into_bytes()
        } else if ss3 {
            format!("\x1bO{x}").into_bytes()
        } else {
            format!("\x1b[{x}").into_bytes()
        }
    };
    // `ESC [ n ~` / `ESC [ n ; m ~`.
    let tilde = |n: u8| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[{n};{param}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    // Alt sends the plain key's bytes prefixed with ESC.
    let with_alt = |bytes: Vec<u8>| -> Vec<u8> {
        if alt {
            let mut out = vec![0x1b];
            out.extend(bytes);
            out
        } else {
            bytes
        }
    };
    let bytes = match key.code {
        // AltGr arrives as Ctrl+Alt on some platforms: it is plain text.
        KeyCode::Char(_) if altgr_char(key).is_some() => {
            let c = altgr_char(key)?;
            c.to_string().into_bytes()
        }
        KeyCode::Char(c) if ctrl => with_alt(vec![match c.to_ascii_lowercase() {
            l @ 'a'..='z' => l as u8 - b'a' + 1,
            ' ' | '@' | '2' => 0x00,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '7' => 0x1f,
            '?' | '8' => 0x7f,
            _ => return None,
        }]),
        KeyCode::Char(c) => with_alt(c.to_string().into_bytes()),
        KeyCode::Enter => with_alt(vec![b'\r']),
        KeyCode::Backspace => with_alt(vec![if ctrl { 0x08 } else { 0x7f }]),
        KeyCode::Tab => with_alt(vec![b'\t']),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => with_alt(vec![0x1b]),
        KeyCode::Up => letter('A', app_cursor),
        KeyCode::Down => letter('B', app_cursor),
        KeyCode::Right => letter('C', app_cursor),
        KeyCode::Left => letter('D', app_cursor),
        KeyCode::Home => letter('H', app_cursor),
        KeyCode::End => letter('F', app_cursor),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => letter((b'P' + n - 1) as char, true),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5]),
        _ => return None,
    };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_keys_use_xterm_encoding() {
        let k = |code, m| KeyEvent::new(code, m);
        let none = KeyModifiers::NONE;
        let bytes = |key: KeyEvent, app| terminal_key_bytes(&key, app).unwrap();
        assert_eq!(bytes(k(KeyCode::F(10), none), false), b"\x1b[21~");
        assert_eq!(bytes(k(KeyCode::F(5), none), false), b"\x1b[15~");
        assert_eq!(bytes(k(KeyCode::F(1), none), false), b"\x1bOP");
        assert_eq!(bytes(k(KeyCode::Up, none), false), b"\x1b[A");
        assert_eq!(bytes(k(KeyCode::Up, none), true), b"\x1bOA");
        assert_eq!(
            bytes(k(KeyCode::Right, KeyModifiers::CONTROL), true),
            b"\x1b[1;5C"
        );
        assert_eq!(
            bytes(k(KeyCode::Char('x'), KeyModifiers::ALT), false),
            b"\x1bx"
        );
        assert_eq!(
            bytes(k(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            [3]
        );
        assert_eq!(
            bytes(k(KeyCode::BackTab, KeyModifiers::SHIFT), false),
            b"\x1b[Z"
        );
    }

    #[test]
    fn focused_terminal_gets_every_key_but_its_own_shortcuts() {
        let kb = Keybindings::default();
        let ctrl = KeyModifiers::CONTROL;
        let at = |code, m| resolve(&kb, KeyEvent::new(code, m), Focus::Terminal, false);
        assert!(matches!(
            at(KeyCode::Char('w'), ctrl),
            Some(Action::PtyKey(_))
        ));
        assert!(matches!(
            at(KeyCode::Char('q'), ctrl),
            Some(Action::PtyKey(_))
        ));
        assert!(matches!(
            at(KeyCode::F(10), KeyModifiers::NONE),
            Some(Action::PtyKey(_))
        ));
        assert!(matches!(
            at(KeyCode::Char('j'), ctrl),
            Some(Action::ToggleTerminal)
        ));
        assert!(matches!(
            at(KeyCode::Char('j'), ctrl | KeyModifiers::SHIFT),
            Some(Action::NewTerminal)
        ));
    }

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
