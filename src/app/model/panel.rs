//! Layout-level enums and state: activity-bar panels, keyboard focus, mouse
//! drag targets and pane sizes.

use super::Model;

/// Left activity bar panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Files,
    Search,
    Git,
    Extensions,
    /// Theme picker — right below Extensions.
    Themes,
    /// Editor preferences (format on save, etc.) — right below Themes.
    Settings,
}

impl Panel {
    pub const ALL: [Panel; 6] = [
        Panel::Files,
        Panel::Search,
        Panel::Git,
        Panel::Extensions,
        Panel::Themes,
        Panel::Settings,
    ];

    pub fn icon(&self) -> &'static str {
        match self {
            Panel::Files => "\u{f4a5}", // file
            Panel::Search => "\u{f002}",
            Panel::Git => "\u{f419}",
            Panel::Extensions => "\u{f12e}",
            Panel::Themes => "\u{f1fc}",   // palette
            Panel::Settings => "\u{f013}", // gear
        }
    }

    /// Fallback icons when Nerd Font glyphs are off (`ascii_icons`): plain
    /// emoji, which every modern terminal font renders without a patched font.
    pub fn ascii_icon(&self) -> &'static str {
        match self {
            Panel::Files => "📁",
            Panel::Search => "🔍",
            Panel::Git => "🔀",
            Panel::Extensions => "📖",
            Panel::Themes => "🔦",
            Panel::Settings => "⚙️",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Panel::Files => "EXPLORER",
            Panel::Search => "SEARCH",
            Panel::Git => "SOURCE CONTROL",
            Panel::Extensions => "EXTENSIONS",
            Panel::Themes => "THEMES",
            Panel::Settings => "SETTINGS",
        }
    }
}

/// Focus — the target that keyboard events are routed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Terminal,
    Sidebar,
    SearchInput,
    /// The commit message input in the Git panel.
    GitCommit,
    /// The in-editor find/replace widget.
    Find,
}

/// Mouse drag target (panel resizing / text selection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragTarget {
    SidebarBorder,
    TerminalBorder,
    EditorSelect,
    /// Dragging the editor scrollbar thumb.
    Scrollbar,
    /// Dragging the editor's horizontal scrollbar thumb.
    ScrollbarX,
    /// Dragging inside the terminal area to select text.
    TerminalSelect,
    /// Dragging the terminal scrollback scrollbar thumb.
    TerminalScrollbar,
}

pub struct LayoutState {
    pub sidebar_width: u16,
    pub terminal_height: u16,
    pub sidebar_open: bool,
    pub terminal_open: bool,
}

impl Default for LayoutState {
    fn default() -> Self {
        LayoutState {
            sidebar_width: 30,
            terminal_height: 12,
            sidebar_open: true,
            terminal_open: false,
        }
    }
}

impl Model {
    pub fn panel_icon(&self, panel: Panel) -> &'static str {
        if self.ascii_icons {
            panel.ascii_icon()
        } else {
            panel.icon()
        }
    }
}
