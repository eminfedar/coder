//! User-editable keyboard shortcuts.
//!
//! The discrete *command* shortcuts (quit, save, copy, new file, …) are declared
//! in a hand-editable TOML file and loaded into a lookup that `keymap::resolve`
//! consults before its hardcoded fallback. Text typing and cursor motion are
//! **not** remappable — they stay in code — so this file only ever holds the
//! named commands a user might realistically want to rebind.
//!
//! ```toml
//! [global]
//! quit = "ctrl+q"
//! save = "ctrl+s"
//!
//! [editor]
//! copy = "ctrl+c"
//!
//! [sidebar]
//! new_file = "ctrl+n"
//! ```

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use std::path::PathBuf;

use crate::app::model::{Focus, Panel};
use crate::core::keymap::Action;

/// A remappable command. Each maps to exactly one `Action` and lives in one
/// scope (which focuses it applies to).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bindable {
    // Global (any focus)
    Quit,
    ToggleSidebar,
    ToggleTerminal,
    CloseTab,
    Save,
    NextTab,
    PrevTab,
    Find,
    Replace,
    Shortcuts,
    Quickbar,
    Explorer,
    Search,
    Git,
    Extensions,
    Themes,
    Settings,
    Rename,
    /// The unlock (leader) key: press it, then a locked command chord.
    Leader,
    // Editor focus
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    SelectAll,
    Completion,
    Format,
    MoveLineUp,
    MoveLineDown,
    // Sidebar focus
    NewFile,
    NewFolder,
    DeleteEntry,
    /// A fresh "Untitled-N" scratch buffer, typed into first and named on
    /// save — distinct from `NewFile`, which creates a real file on disk.
    NewUntitledFile,
}

/// Which focus modes a binding fires in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Fires in every focus.
    Global,
    /// Fires only while the editor is focused.
    Editor,
    /// Fires only while the sidebar is focused.
    Sidebar,
}

impl Scope {
    fn matches(self, focus: Focus) -> bool {
        match self {
            Scope::Global => true,
            Scope::Editor => focus == Focus::Editor,
            Scope::Sidebar => focus == Focus::Sidebar,
        }
    }

    /// The `[section]` name this scope is written under in the TOML file.
    fn section(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Editor => "editor",
            Scope::Sidebar => "sidebar",
        }
    }
}

impl Bindable {
    /// Every command, in file order (also the TOML write order).
    const ALL: [Bindable; 33] = [
        Bindable::Quit,
        Bindable::ToggleSidebar,
        Bindable::ToggleTerminal,
        Bindable::CloseTab,
        Bindable::Save,
        Bindable::NextTab,
        Bindable::PrevTab,
        Bindable::Find,
        Bindable::Replace,
        Bindable::Shortcuts,
        Bindable::Quickbar,
        Bindable::Explorer,
        Bindable::Search,
        Bindable::Git,
        Bindable::Extensions,
        Bindable::Themes,
        Bindable::Settings,
        Bindable::Rename,
        Bindable::Leader,
        Bindable::Copy,
        Bindable::Cut,
        Bindable::Paste,
        Bindable::Undo,
        Bindable::Redo,
        Bindable::SelectAll,
        Bindable::Completion,
        Bindable::Format,
        Bindable::MoveLineUp,
        Bindable::MoveLineDown,
        Bindable::NewFile,
        Bindable::NewFolder,
        Bindable::DeleteEntry,
        Bindable::NewUntitledFile,
    ];

    /// Stable TOML key name.
    fn name(self) -> &'static str {
        match self {
            Bindable::Quit => "quit",
            Bindable::ToggleSidebar => "toggle_sidebar",
            Bindable::ToggleTerminal => "toggle_terminal",
            Bindable::CloseTab => "close_tab",
            Bindable::Save => "save",
            Bindable::NextTab => "next_tab",
            Bindable::PrevTab => "prev_tab",
            Bindable::Find => "find",
            Bindable::Replace => "replace",
            Bindable::Shortcuts => "shortcuts",
            Bindable::Quickbar => "quickbar",
            Bindable::Explorer => "explorer",
            Bindable::Search => "search",
            Bindable::Git => "git",
            Bindable::Extensions => "extensions",
            Bindable::Themes => "themes",
            Bindable::Settings => "settings",
            Bindable::Rename => "rename",
            Bindable::Leader => "leader",
            Bindable::Copy => "copy",
            Bindable::Cut => "cut",
            Bindable::Paste => "paste",
            Bindable::Undo => "undo",
            Bindable::Redo => "redo",
            Bindable::SelectAll => "select_all",
            Bindable::Completion => "completion",
            Bindable::Format => "format",
            Bindable::MoveLineUp => "move_line_up",
            Bindable::MoveLineDown => "move_line_down",
            Bindable::NewFile => "new_file",
            Bindable::NewFolder => "new_folder",
            Bindable::DeleteEntry => "delete_entry",
            Bindable::NewUntitledFile => "new_untitled_file",
        }
    }

    fn scope(self) -> Scope {
        match self {
            Bindable::Copy
            | Bindable::Cut
            | Bindable::Paste
            | Bindable::Undo
            | Bindable::Redo
            | Bindable::SelectAll
            | Bindable::Completion
            | Bindable::Format
            | Bindable::MoveLineUp
            | Bindable::MoveLineDown
            | Bindable::NewUntitledFile => Scope::Editor,
            Bindable::NewFile | Bindable::NewFolder | Bindable::DeleteEntry => Scope::Sidebar,
            _ => Scope::Global,
        }
    }

    /// The out-of-the-box chord (also what seeds a fresh file).
    fn default_chord(self) -> &'static str {
        match self {
            Bindable::Quit => "ctrl+q",
            Bindable::ToggleSidebar => "ctrl+b",
            Bindable::ToggleTerminal => "ctrl+j",
            Bindable::CloseTab => "ctrl+w",
            Bindable::Save => "ctrl+s",
            Bindable::NextTab => "ctrl+tab",
            // Shift+Tab (BackTab) works in legacy terminals; ctrl+shift+tab does not.
            Bindable::PrevTab => "shift+tab",
            Bindable::Find => "ctrl+f",
            Bindable::Replace => "ctrl+h",
            Bindable::Shortcuts => "alt+7",
            Bindable::Quickbar => "ctrl+p",
            // Panels use Alt+digit: legacy terminals (GNOME Terminal / VTE) can't
            // send Ctrl+Shift+<letter> distinctly — the Shift bit collapses so
            // e.g. Ctrl+Shift+S is byte-identical to Ctrl+S (Save). Alt+digit
            // sends a distinct ESC-prefixed sequence that every terminal reports.
            Bindable::Explorer => "alt+1",
            Bindable::Search => "alt+2",
            Bindable::Git => "alt+3",
            Bindable::Extensions => "alt+4",
            Bindable::Themes => "alt+5",
            Bindable::Settings => "alt+6",
            Bindable::Rename => "f2",
            // Alt+letter, not Ctrl+Shift+<punctuation>: like the Alt+digit panel
            // shortcuts above, this is a distinct ESC-prefixed sequence every
            // terminal reports. Ctrl+Shift+; (the old default, "ctrl+:") has no
            // standard ASCII control code, so most terminals without the kitty
            // keyboard protocol send it as a bare ';'/':' with no Ctrl bit at all —
            // the leader key would silently never fire.
            Bindable::Leader => "alt+l",
            Bindable::Copy => "ctrl+c",
            Bindable::Cut => "ctrl+x",
            Bindable::Paste => "ctrl+v",
            Bindable::Undo => "ctrl+z",
            Bindable::Redo => "ctrl+y",
            Bindable::SelectAll => "ctrl+a",
            Bindable::Completion => "ctrl+space",
            Bindable::Format => "ctrl+alt+f",
            Bindable::MoveLineUp => "alt+up",
            Bindable::MoveLineDown => "alt+down",
            Bindable::NewFile => "ctrl+n",
            Bindable::NewFolder => "alt+shift+n",
            Bindable::DeleteEntry => "delete",
            // Same chord as the sidebar's `NewFile`, but a different scope
            // (Editor vs Sidebar), so the two never collide — whichever is
            // focused decides which one fires.
            Bindable::NewUntitledFile => "ctrl+n",
        }
    }

    /// The concrete `Action` this command triggers.
    fn to_action(self) -> Action {
        match self {
            Bindable::Quit => Action::Quit,
            Bindable::ToggleSidebar => Action::ToggleSidebar,
            Bindable::ToggleTerminal => Action::ToggleTerminal,
            Bindable::CloseTab => Action::CloseTab,
            Bindable::Save => Action::Save,
            Bindable::NextTab => Action::NextTab,
            Bindable::PrevTab => Action::PrevTab,
            Bindable::Find => Action::OpenFind,
            Bindable::Replace => Action::OpenFindReplace,
            Bindable::Shortcuts => Action::ShowShortcuts,
            Bindable::Quickbar => Action::OpenQuickbar,
            Bindable::Explorer => Action::SelectPanel(Panel::Files),
            Bindable::Search => Action::SelectPanel(Panel::Search),
            Bindable::Git => Action::SelectPanel(Panel::Git),
            Bindable::Extensions => Action::SelectPanel(Panel::Extensions),
            Bindable::Themes => Action::SelectPanel(Panel::Themes),
            Bindable::Settings => Action::SelectPanel(Panel::Settings),
            Bindable::Rename => Action::RenameEntry,
            Bindable::Leader => Action::Leader,
            Bindable::Copy => Action::Copy,
            Bindable::Cut => Action::Cut,
            Bindable::Paste => Action::Paste,
            Bindable::Undo => Action::Undo,
            Bindable::Redo => Action::Redo,
            Bindable::SelectAll => Action::SelectAll,
            Bindable::Completion => Action::TriggerCompletion,
            Bindable::Format => Action::Format,
            Bindable::MoveLineUp => Action::MoveLineUp,
            Bindable::MoveLineDown => Action::MoveLineDown,
            Bindable::NewFile => Action::NewFile,
            Bindable::NewFolder => Action::NewFolder,
            Bindable::DeleteEntry => Action::DeleteEntry,
            Bindable::NewUntitledFile => Action::NewUntitledFile,
        }
    }

    /// Whether the command is *locked* behind the leader key by default.
    /// Locking is **opt-in**: by default every command fires directly (quick).
    /// A command is only locked (needs the leader key first) when its TOML
    /// entry carries a `, locked` suffix, e.g. `quit = "ctrl+q, locked"`.
    fn default_locked(self) -> bool {
        false
    }

    fn by_name(section: &str, name: &str) -> Option<Bindable> {
        Bindable::ALL
            .into_iter()
            .find(|b| b.scope().section() == section && b.name() == name)
    }
}

/// A key press as a comparable chord (letters normalized to lowercase).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chord {
    code: KeyCode,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

impl Chord {
    fn from_event(key: KeyEvent) -> Chord {
        // Legacy terminals encode Shift in the character itself (an uppercase
        // letter, or BackTab for Shift+Tab) rather than a modifier bit. Fold that
        // back into `shift` so chords like "shift+tab" / "alt+shift+n" still match.
        let mut shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let code = match key.code {
            KeyCode::BackTab => {
                shift = true;
                KeyCode::Tab
            }
            KeyCode::Char(c) => {
                if c.is_ascii_uppercase() {
                    shift = true;
                }
                // ':' is Shift+';'; fold the physical `;` back so a "ctrl+:"
                // binding matches the Ctrl+Shift+; that legacy terminals send.
                let lower = c.to_ascii_lowercase();
                if lower == ';' {
                    shift = true;
                    KeyCode::Char(':')
                } else {
                    KeyCode::Char(lower)
                }
            }
            other => other,
        };
        Chord {
            code,
            ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
            shift,
            alt: key.modifiers.contains(KeyModifiers::ALT),
        }
    }

    /// Parses `"ctrl+shift+e"`, `"alt+up"`, `"f2"`, `"delete"`, … (case-insensitive).
    fn parse(s: &str) -> Option<Chord> {
        let (mut ctrl, mut shift, mut alt) = (false, false, false);
        let mut code = None;
        for part in s.split('+') {
            match part.trim().to_ascii_lowercase().as_str() {
                "ctrl" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                "" => {}
                key => code = Some(parse_key(key)?),
            }
        }
        Some(Chord {
            code: code?,
            ctrl,
            shift,
            alt,
        })
    }

    fn to_chord_string(self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("ctrl".into());
        }
        if self.shift {
            parts.push("shift".into());
        }
        if self.alt {
            parts.push("alt".into());
        }
        parts.push(key_string(self.code));
        parts.join("+")
    }
}

fn parse_key(s: &str) -> Option<KeyCode> {
    Some(match s {
        "space" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "delete" | "del" => KeyCode::Delete,
        "backspace" => KeyCode::Backspace,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        // `+` separates chord parts, so the plus key needs a name of its own.
        "plus" => KeyCode::Char('+'),
        other => {
            if let Some(n) = other
                .strip_prefix('f')
                .filter(|d| !d.is_empty())
                .and_then(|d| d.parse::<u8>().ok())
            {
                if (1..=12).contains(&n) {
                    KeyCode::F(n)
                } else {
                    return None;
                }
            } else if other.chars().count() == 1 {
                KeyCode::Char(other.chars().next().unwrap().to_ascii_lowercase())
            } else {
                return None;
            }
        }
    })
}

fn key_string(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "space".into(),
        KeyCode::Char('+') => "plus".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "tab".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::F(n) => format!("f{n}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// The loaded shortcuts: an ordered `(command, chord, locked)` table.
/// A resolved command binding: the `Action` plus whether it is locked
/// (needs the leader key) per the active keymap.
pub struct Resolved {
    pub action: Action,
    pub locked: bool,
}

pub struct Keybindings {
    binds: Vec<(Bindable, Chord, bool)>,
}

impl Default for Keybindings {
    fn default() -> Self {
        let binds = Bindable::ALL
            .into_iter()
            // Defaults are known-valid, so `parse` never fails here.
            .filter_map(|b| Chord::parse(b.default_chord()).map(|c| (b, c, b.default_locked())))
            .collect();
        Keybindings { binds }
    }
}

impl Keybindings {
    /// Resolves a key press to its bound `Action` for the given focus, or `None`
    /// (letting the hardcoded typing/motion fallback take over).
    pub fn resolve(&self, key: KeyEvent, focus: Focus) -> Option<Action> {
        self.resolve_full(key, focus).map(|r| r.action)
    }

    /// Like `resolve`, but also reports the `locked` flag so the caller can gate
    /// the action behind the leader key.
    pub fn resolve_full(&self, key: KeyEvent, focus: Focus) -> Option<Resolved> {
        let chord = Chord::from_event(key);
        self.binds
            .iter()
            .find(|(b, c, _)| *c == chord && b.scope().matches(focus))
            .map(|(b, _, locked)| Resolved {
                action: b.to_action(),
                locked: *locked,
            })
    }

    /// Returns the active, human-readable shortcut for a command.
    pub fn shortcut(&self, binding: Bindable) -> String {
        let Some((_, chord, locked)) = self.binds.iter().find(|(b, _, _)| *b == binding) else {
            return String::new();
        };
        let label = chord
            .to_chord_string()
            .split('+')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join("+");
        if *locked {
            format!("Leader + {label}")
        } else {
            label
        }
    }

    fn set(&mut self, b: Bindable, chord: Chord, locked: Option<bool>) {
        let default_locked = b.default_locked();
        if let Some(entry) = self.binds.iter_mut().find(|(x, _, _)| *x == b) {
            entry.1 = chord;
            entry.2 = locked.unwrap_or(default_locked);
        }
    }
}

/// Path of the keybindings file: alongside `config.toml` (`keybindings.toml`).
pub fn keybindings_path() -> Option<PathBuf> {
    Some(crate::services::config::config_path()?.with_file_name("keybindings.toml"))
}

/// Parses the file, starting from the defaults and overriding any command whose
/// chord is declared. Unknown keys / unparseable chords are ignored, so a
/// partial or slightly malformed file still yields a working keymap.
pub fn parse(text: &str) -> Keybindings {
    let mut kb = Keybindings::default();
    let Ok(table) = text.parse::<toml::Table>() else {
        return kb;
    };
    for (section, value) in &table {
        let Some(entries) = value.as_table() else {
            continue;
        };
        for (name, chord_val) in entries {
            let Some(chord_str) = chord_val.as_str() else {
                continue;
            };
            // Optional "<chord>, quick|locked" suffix overrides the default, so a
            // command may be locked behind the leader (or freed) on a per-command
            // basis: e.g. `format = "ctrl+alt+f, locked"`.
            // Only a recognized flag after the *last* comma counts, so the
            // comma key itself stays bindable (`"ctrl+,"`, `"ctrl+,, locked"`).
            let (chord_part, locked_override) = match chord_str.rsplit_once(',') {
                Some((c, flag)) if matches!(flag.trim(), "locked" | "quick") => {
                    (c.trim(), Some(flag.trim() == "locked"))
                }
                _ => (chord_str, None),
            };
            if let Some(b) = Bindable::by_name(section, name)
                && let Some(chord) = Chord::parse(chord_part)
            {
                kb.set(b, chord, locked_override);
            }
        }
    }
    kb
}

/// Serializes the shortcuts to a commented, `[section]`-grouped TOML document.
pub fn to_toml(kb: &Keybindings) -> String {
    let chord_of = |b: Bindable| {
        kb.binds
            .iter()
            .find(|(x, _, _)| *x == b)
            .map(|(_, c, _)| c.to_chord_string())
            .unwrap_or_default()
    };
    let mut out = String::new();
    out.push_str("# Coder keyboard shortcuts. Edit a value and save (Ctrl+S) to apply live.\n");
    out.push_str(
        "# Chord format: \"ctrl+shift+e\", \"alt+up\", \"f2\", \"delete\", \"ctrl+space\".\n",
    );
    out.push_str("# Modifiers: ctrl, shift, alt. Keys: a-z, 0-9, f1-f12, up/down/left/right,\n");
    out.push_str("# home/end, pageup/pagedown, tab, enter, esc, space, delete.\n");
    out.push_str("# Only these commands are remappable; typing and cursor motion are fixed.\n");
    out.push_str("# Append \", locked\" to a chord to require the leader key (alt+l) first.\n");
    for scope in [Scope::Global, Scope::Editor, Scope::Sidebar] {
        out.push_str(&format!("\n[{}]\n", scope.section()));
        for b in Bindable::ALL.into_iter().filter(|b| b.scope() == scope) {
            let locked = kb
                .binds
                .iter()
                .find(|(x, _, _)| *x == b)
                .map(|(_, _, l)| *l)
                .unwrap_or_else(|| b.default_locked());
            // Write an explicit ", quick"/", locked" suffix only when it differs from the
            // command's default, so the generated file stays minimal and readable.
            let suffix = if locked != b.default_locked() {
                if locked { ", locked" } else { ", quick" }
            } else {
                ""
            };
            // A TOML string literal, so a `"` or `\\` key is escaped properly.
            let value = toml::Value::String(format!("{}{suffix}", chord_of(b)));
            out.push_str(&format!("{name} = {value}\n", name = b.name()));
        }
    }
    out
}

/// Writes the shortcuts to disk (creating the parent directory).
pub fn save(kb: &Keybindings) -> std::io::Result<()> {
    let path = keybindings_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no config path available")
    })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::services::fs::write_atomic(&path, to_toml(kb).as_bytes())
}

/// Loads the shortcuts, seeding the file with the defaults when it is missing so
/// there is always something to open and edit.
pub fn load() -> Keybindings {
    let Some(path) = keybindings_path() else {
        return Keybindings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        // Seed only a genuinely missing file; an unreadable one is left alone.
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Keybindings::default(),
        Err(_) => {
            let kb = Keybindings::default();
            let _ = save(&kb);
            kb
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn defaults_resolve_global_and_scoped() {
        let kb = Keybindings::default();
        // Ctrl+Q quits from any focus.
        assert!(matches!(
            kb.resolve(ev(KeyCode::Char('q'), KeyModifiers::CONTROL), Focus::Editor),
            Some(Action::Quit)
        ));
        // Ctrl+C copies only in the editor.
        assert!(matches!(
            kb.resolve(ev(KeyCode::Char('c'), KeyModifiers::CONTROL), Focus::Editor),
            Some(Action::Copy)
        ));
        assert!(
            kb.resolve(
                ev(KeyCode::Char('c'), KeyModifiers::CONTROL),
                Focus::Sidebar
            )
            .is_none()
        );
    }

    #[test]
    fn alt7_opens_shortcuts() {
        let kb = Keybindings::default();
        assert!(matches!(
            kb.resolve(ev(KeyCode::Char('7'), KeyModifiers::ALT), Focus::Editor),
            Some(Action::ShowShortcuts)
        ));
    }

    #[test]
    fn plain_typing_is_not_a_command() {
        let kb = Keybindings::default();
        assert!(
            kb.resolve(ev(KeyCode::Char('a'), KeyModifiers::NONE), Focus::Editor)
                .is_none()
        );
    }

    #[test]
    fn overrides_apply_and_round_trip() {
        let kb = parse("[global]\nquit = \"ctrl+shift+q\"\n");
        assert!(
            kb.resolve(ev(KeyCode::Char('q'), KeyModifiers::CONTROL), Focus::Editor)
                .is_none()
        );
        assert!(matches!(
            kb.resolve(
                ev(
                    KeyCode::Char('q'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                ),
                Focus::Editor
            ),
            Some(Action::Quit)
        ));
        // A full round-trip preserves the override.
        let restored = parse(&to_toml(&kb));
        assert!(matches!(
            restored.resolve(
                ev(
                    KeyCode::Char('q'),
                    KeyModifiers::CONTROL | KeyModifiers::SHIFT
                ),
                Focus::Editor
            ),
            Some(Action::Quit)
        ));
    }

    #[test]
    fn quickbar_binding_is_remappable() {
        // The out-of-the-box chord opens the palette.
        let default = Keybindings::default();
        assert!(matches!(
            default.resolve(ev(KeyCode::Char('p'), KeyModifiers::CONTROL), Focus::Editor),
            Some(Action::OpenQuickbar)
        ));
        // Declaring a different chord in the file overrides it, and the old
        // chord stops working, so editing keybindings.toml re-binds the palette.
        let remapped = parse("[global]\nquickbar = \"ctrl+k\"\n");
        assert!(matches!(
            remapped.resolve(ev(KeyCode::Char('k'), KeyModifiers::CONTROL), Focus::Editor),
            Some(Action::OpenQuickbar)
        ));
        assert!(
            remapped
                .resolve(ev(KeyCode::Char('p'), KeyModifiers::CONTROL), Focus::Editor)
                .is_none()
        );
        // Global scope resolves from the sidebar focus too (palette is open).
        assert!(matches!(
            remapped.resolve(
                ev(KeyCode::Char('k'), KeyModifiers::CONTROL),
                Focus::Sidebar
            ),
            Some(Action::OpenQuickbar)
        ));
    }
    #[test]
    fn leader_chord_parses_and_resolves() {
        // Alt+letter, not Ctrl+Shift+<punctuation>: every terminal reports it
        // distinctly (see the comment on `Bindable::default_chord`'s Leader arm),
        // unlike the old "ctrl+:" default which most terminals can't send at all.
        let kb = Keybindings::default();
        let r = kb
            .resolve_full(ev(KeyCode::Char('l'), KeyModifiers::ALT), Focus::Editor)
            .expect("alt+l should resolve to Leader");
        assert!(!r.locked);
        assert!(matches!(r.action, Action::Leader));
    }

    #[test]
    fn defaults_are_quick_without_leader() {
        // Every default binding fires directly; quit/save do NOT need the leader.
        let kb = Keybindings::default();
        let quit = kb
            .resolve_full(ev(KeyCode::Char('q'), KeyModifiers::CONTROL), Focus::Editor)
            .expect("ctrl+q resolves");
        assert!(!quit.locked, "default quit must be quick");
        let save = kb
            .resolve_full(ev(KeyCode::Char('s'), KeyModifiers::CONTROL), Focus::Editor)
            .expect("ctrl+s resolves");
        assert!(!save.locked, "default save must be quick");
    }

    #[test]
    fn locked_requires_unlock_through_keymap_resolve() {
        // A command marked `, locked` only fires through keymap::resolve when the
        // unlock (leader) flag is set; otherwise it returns None so the keystroke
        // falls through to typing/motion.
        let kb = parse("[global]\nquit = \"ctrl+q, locked\"\n");
        let quick = kb
            .resolve_full(ev(KeyCode::Char('q'), KeyModifiers::CONTROL), Focus::Editor)
            .expect("ctrl+q resolves");
        assert!(quick.locked, "explicitly locked command reports locked");

        let locked_no_unlock = crate::core::keymap::resolve(
            &kb,
            ev(KeyCode::Char('q'), KeyModifiers::CONTROL),
            Focus::Editor,
            false,
        );
        assert!(
            locked_no_unlock.is_none(),
            "locked + no unlock falls through"
        );

        let locked_unlock = crate::core::keymap::resolve(
            &kb,
            ev(KeyCode::Char('q'), KeyModifiers::CONTROL),
            Focus::Editor,
            true,
        );
        assert!(
            matches!(locked_unlock, Some(Action::Quit)),
            "locked + unlock fires"
        );
    }

    #[test]
    fn comma_key_is_bindable_with_and_without_a_flag() {
        let kb = parse("[global]\nquit = \"ctrl+,\"\n");
        assert!(matches!(
            kb.resolve(ev(KeyCode::Char(','), KeyModifiers::CONTROL), Focus::Editor),
            Some(Action::Quit)
        ));
        let kb = parse("[global]\nquit = \"ctrl+,, quick\"\n");
        assert!(matches!(
            kb.resolve(ev(KeyCode::Char(','), KeyModifiers::CONTROL), Focus::Editor),
            Some(Action::Quit)
        ));
    }

    #[test]
    fn quote_and_backslash_keys_round_trip_through_toml() {
        for key in ['"', '\\', '+'] {
            let mut kb = Keybindings::default();
            let chord = Chord {
                code: KeyCode::Char(key),
                ctrl: true,
                shift: false,
                alt: false,
            };
            kb.set(Bindable::Quit, chord, None);
            let text = to_toml(&kb);
            assert!(
                text.parse::<toml::Table>().is_ok(),
                "invalid TOML for {key:?}"
            );
            let back = parse(&text);
            assert!(matches!(
                back.resolve(ev(KeyCode::Char(key), KeyModifiers::CONTROL), Focus::Editor),
                Some(Action::Quit)
            ));
        }
    }
}
