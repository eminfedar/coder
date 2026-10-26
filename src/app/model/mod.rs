//! Application state (the `Model` of the Elm Architecture).

mod config;
mod find;
mod gitmarks;
mod highlighting;
mod hscroll;
mod lsp;
mod overlay;
mod panel;
mod search_marks;
mod session;
mod sidebar;
mod tab;
mod terminal;
mod workspace;

pub use find::{FindField, FindState};
pub use lsp::{CompletionState, Diagnostic, LspState, PendingFormat};
pub use overlay::{
    ContextMenu, Dialog, DialogAction, DialogKind, MenuItem, QuickbarFile, QuickbarItem,
    QuickbarState, TOAST_DURATION, Toast,
};
pub use panel::{DragTarget, Focus, LayoutState, Panel};
pub use session::SessionRestore;
pub use sidebar::{
    GitStatus, GitZone, SearchField, SearchState, SettingsState, Sidebar, ThemesState,
};
pub use tab::{DiffRow, PreviewKey, Tab};
pub use terminal::{TerminalState, Terminals};

use std::path::PathBuf;

use crate::app::cmd::Cmd;
use crate::core::filetree::FileTree;
use crate::core::highlight::{self, HlLine};
use crate::core::theme::Theme;
use crate::services::git::GutterKind;

pub struct Model {
    pub root: PathBuf,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<usize>,
    pub sidebar: Sidebar,
    pub terminals: Terminals,
    pub layout: LayoutState,
    pub focus: Focus,
    pub should_quit: bool,
    /// User-editable keyboard shortcuts (loaded from `keybindings.toml`).
    pub keybindings: crate::services::keybindings::Keybindings,
    /// Leader/unlock mode: while true, locked commands fire directly instead of
    /// falling through to typing/motion (see `Action::Leader`).
    pub leader: bool,
    pub internal_clipboard: String,
    pub theme: Theme,
    /// Last known terminal size — for mouse hit-testing and layout.
    pub term_size: (u16, u16),
    pub drag: Option<DragTarget>,
    /// Use ASCII instead of Nerd Font icons (for compatibility).
    pub ascii_icons: bool,
    /// Colored lines for the active buffer, produced off-thread by the highlight
    /// worker (see `app::hlworker`). Indexed from `display_base`; rows outside the
    /// range render as plain text until the worker fills them in.
    display_hl: Vec<HlLine>,
    /// Buffer line index of `display_hl[0]` (the first visible line last shipped).
    display_base: usize,
    /// The (tab, version) `display_hl` was produced for. Colors may lag the current
    /// version by a frame or two while typing; that is the point — text is never
    /// held back waiting for color.
    display_key: Option<(usize, u64)>,
    /// Channel to the highlight worker; `None` until wired up in `run`.
    hl_tx: Option<std::sync::mpsc::Sender<crate::app::hlworker::HlJob>>,
    /// The (tab, version, scroll_y, needed) of the last job sent, to avoid
    /// resubmitting an identical request every frame while still catching a scroll
    /// that reveals lines above or below the shipped slice.
    hl_sent: Option<(usize, u64, usize, usize)>,
    /// Set when the worker must drop its cache before the next job (content replaced
    /// by reload / format, or the theme changed).
    hl_reset: bool,
    /// Line to jump to after the file is loaded (opening from a search result).
    pub pending_goto: Option<(PathBuf, usize)>,
    /// A file whose next load should become a diff-mode tab (opened from the Git panel).
    pub pending_diff: Option<PathBuf>,
    /// The entry the Files/Git panel's arrow keys last asked to preview; its
    /// load becomes (or replaces) the preview tab.
    pub pending_preview: Option<PreviewKey>,
    /// Preview loads still in flight, counted per key. A load that arrives for
    /// an entry the user has already arrowed past is dropped, not opened.
    pub preview_loads: std::collections::HashMap<PreviewKey, usize>,
    /// A just-opened diff tab that should scroll to its first change once HEAD loads.
    pub pending_diff_scroll: Option<PathBuf>,
    /// The open modal dialog (captures all input when present).
    pub dialog: Option<Dialog>,
    /// The open file-tree context menu (captures all input when present).
    pub context_menu: Option<ContextMenu>,
    /// The open quickbar (command palette) overlay, if any. It captures all input
    /// while present, like `Dialog`/`ContextMenu`.
    pub quickbar: Option<QuickbarState>,
    /// Change-gutter markers for the active buffer, keyed by line index.
    pub active_git_marks: std::collections::HashMap<usize, GutterKind>,
    /// Which tab the current git-diff markers were computed for. The diff is
    /// recomputed only on a tab switch or when `git_marks_dirty` is set (save,
    /// reload, disk change, HEAD load) — never on a plain edit, so the gutter does
    /// not churn a whole-file diff on every keystroke while typing.
    active_git_marks_tab: Option<usize>,
    /// Set when the git diff needs recomputing for a non-edit reason (file saved,
    /// reloaded, changed on disk, or its HEAD text (re)loaded). Consumed by the
    /// next `refresh_git_marks`.
    git_marks_dirty: bool,
    /// Search-panel query matches in the active buffer ([start, end) char
    /// indices), painted in the editor like find matches. See
    /// `refresh_search_marks`.
    pub search_marks: Vec<(usize, usize)>,
    /// What `search_marks` was computed for: (tab, buffer version, query,
    /// use_regex, match_case). Recomputed only when one of them changes.
    search_marks_key: Option<(usize, u64, String, bool, bool)>,
    /// Character width of the active tab's longest line (horizontal
    /// scrollbar). See `refresh_max_line_width`.
    pub active_max_cols: usize,
    /// What `active_max_cols` was computed for: (tab id, buffer version,
    /// woven deletion blocks).
    active_max_cols_key: Option<(usize, u64, usize)>,
    /// Removed line blocks for the active diff tab's inline view: `(anchor, lines)`
    /// renders `lines` right after buffer line `anchor` (`None` = before line 0).
    /// Empty unless the active tab is a diff tab. Recomputed with `active_git_marks`.
    pub active_deleted: Vec<(Option<usize>, Vec<String>)>,
    /// Cached visual rows for the active tab (see `diff_rows`). Rebuilt only when
    /// the buffer version changes — the render path borrows it instead of
    /// re-materializing a whole-file `Vec` two or three times per frame.
    active_display: Vec<DiffRow>,
    /// The (tab index, buffer version) `active_display` was built for.
    active_display_key: Option<(usize, u64)>,
    /// Deadline to fire a debounced autocomplete request, or `None`. Set to
    /// ~400ms ahead on each identifier keystroke and checked every main-loop
    /// iteration, so a burst of typing spawns no timer tasks and only asks the
    /// server once the user pauses.
    autocomplete_at: Option<std::time::Instant>,
    /// Deadline to flush a debounced LSP `didChange`, or `None`. Set ~1s ahead on
    /// each edit; checked every main-loop iteration like `autocomplete_at`.
    didchange_at: Option<std::time::Instant>,
    /// In-editor find / replace widget state.
    pub find: FindState,
    /// Last left-click (time, column, row) for editor double-click detection.
    pub last_click: Option<(std::time::Instant, u16, u16)>,
    /// Installed language extensions (LSP / formatter / linter manifests).
    pub extensions: crate::services::extensions::ExtensionRegistry,
    /// Running language servers.
    pub lsp: LspState,
    /// Diagnostics per file (buffer char coordinates).
    pub diagnostics: std::collections::HashMap<PathBuf, Vec<Diagnostic>>,
    /// Whether each tool binary (lsp / formatter / linter command) is installed
    /// on PATH, keyed by command name. Filled by `Cmd::CheckTools`; a missing
    /// key means "not probed yet".
    pub tool_available: std::collections::HashMap<String, bool>,
    /// The open completion popup, if any.
    pub completion: Option<CompletionState>,
    /// An in-flight format request awaiting edits.
    pub pending_format: Option<PendingFormat>,
    /// A transient toast notification shown bottom-center, or `None`.
    pub toast: Option<Toast>,
    /// This workspace's session-file generation last observed by this
    /// instance (seeds the multi-instance conflict check in
    /// `services::session::save`). `None` until the first load/save.
    pub session_seen_generation: Option<u64>,
    /// Deadline to flush a debounced session checkpoint, or `None`. Set a
    /// couple seconds ahead on each edit, like `didchange_at`.
    session_save_at: Option<std::time::Instant>,
    /// Next number to hand out for "Untitled-N" (persisted across restarts so
    /// numbering never collides with a still-open buffer).
    pub untitled_seq: u64,
    /// While a session restore is in flight: file tabs still loading async
    /// (`Cmd::ReadFile`), keyed by path, with the cursor/scroll to apply once
    /// loaded and — for a dirty file stored as a diff — the hunks to
    /// reconstruct its unsaved content.
    pub pending_session_restore: std::collections::HashMap<PathBuf, SessionRestore>,
    /// While a session restore is in flight: the path of the tab that should
    /// end up focused, so whichever async load happens to finish last doesn't
    /// win the focus race. Cleared once that load arrives.
    pub session_active_path: Option<PathBuf>,
}

impl Model {
    pub fn new(root: PathBuf) -> Self {
        // Unit tests must never read (or seed) the developer's real config in
        // `$HOME`: results would depend on the machine, and `load` writes files.
        let (config, err) = if cfg!(test) {
            (crate::services::config::seed(), None)
        } else {
            crate::services::config::load_checked()
        };
        let mut model = Model::with_defaults(root);
        model.apply_config(&config);
        // A broken config file is left untouched on disk (never saved over);
        // the defaults are used for this run and the user is told why.
        if let Some(err) = err {
            model.notify(format!("Config error, using defaults: {err}"));
        }
        model
    }

    fn with_defaults(root: PathBuf) -> Self {
        Model {
            keybindings: if cfg!(test) {
                crate::services::keybindings::Keybindings::default()
            } else {
                crate::services::keybindings::load()
            },
            sidebar: Sidebar {
                active: Panel::Files,
                files: FileTree::new(root.clone()),
                git: GitStatus::default(),
                search: SearchState::default(),
                themes: ThemesState::default(),
                settings: SettingsState::default(),
                settings_selected: 0,
            },
            tabs: Vec::new(),
            active_tab: None,
            terminals: Terminals::new(),
            layout: LayoutState::default(),
            focus: Focus::Sidebar,
            should_quit: false,
            leader: false,
            internal_clipboard: String::new(),
            // Keep the UI palette and the syntax theme consistent at startup.
            theme: highlight::theme_for(highlight::DEFAULT_THEME),
            term_size: (80, 24),
            drag: None,
            ascii_icons: std::env::var("CODER_ASCII").is_ok(),
            display_hl: Vec::new(),
            display_base: 0,
            display_key: None,
            hl_tx: None,
            hl_sent: None,
            hl_reset: false,
            pending_goto: None,
            pending_diff: None,
            pending_preview: None,
            preview_loads: std::collections::HashMap::new(),
            pending_diff_scroll: None,
            dialog: None,
            context_menu: None,
            quickbar: None,
            active_git_marks: std::collections::HashMap::new(),
            active_git_marks_tab: None,
            git_marks_dirty: false,
            search_marks: Vec::new(),
            active_max_cols: 0,
            active_max_cols_key: None,
            search_marks_key: None,
            active_deleted: Vec::new(),
            active_display: Vec::new(),
            active_display_key: None,
            autocomplete_at: None,
            didchange_at: None,
            find: FindState::default(),
            last_click: None,
            extensions: crate::services::extensions::ExtensionRegistry::default(),
            lsp: LspState::default(),
            diagnostics: std::collections::HashMap::new(),
            tool_available: std::collections::HashMap::new(),
            completion: None,
            pending_format: None,
            toast: None,
            session_seen_generation: None,
            session_save_at: None,
            untitled_seq: 0,
            pending_session_restore: std::collections::HashMap::new(),
            session_active_path: None,
            root,
        }
    }

    /// Raises a transient toast notification (bottom-center, auto-hides). Returns
    /// the command that schedules its disappearance.
    pub fn show_toast(&mut self, message: impl Into<String>) -> Cmd {
        self.toast = Some(Toast {
            message: message.into(),
            shown_at: std::time::Instant::now(),
        });
        Cmd::ScheduleToastExpiry
    }

    /// Surfaces a transient status message to the user as a toast. Fire-and-forget
    /// convenience for the many sync handlers that previously wrote the status bar;
    /// the toast's auto-hide is gated by `Toast::is_expired` at render time.
    pub fn notify(&mut self, message: impl Into<String>) {
        let _ = self.show_toast(message);
    }

    /// Schedules a debounced autocomplete request ~400ms out, resetting the timer
    /// on every keystroke so the server is only asked once typing pauses.
    pub fn schedule_autocomplete(&mut self) {
        self.autocomplete_at =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(400));
    }

    /// Schedules a debounced LSP `didChange` flush ~1s out (reset on every edit).
    pub fn schedule_didchange(&mut self) {
        self.didchange_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
    }

    /// Cancels any pending autocomplete deadline (e.g. the popup was dismissed).
    pub fn cancel_autocomplete(&mut self) {
        self.autocomplete_at = None;
    }

    /// Cancels any pending `didChange` deadline (its text was already flushed).
    pub fn cancel_didchange(&mut self) {
        self.didchange_at = None;
    }

    /// Returns `(autocomplete_due, didchange_due, session_save_due)` for
    /// deadlines that have elapsed by `now`, clearing each that fired. Called
    /// once per main-loop iteration.
    pub fn take_due_timers(&mut self, now: std::time::Instant) -> (bool, bool, bool) {
        let ac = self.autocomplete_at.is_some_and(|t| t <= now);
        if ac {
            self.autocomplete_at = None;
        }
        let dc = self.didchange_at.is_some_and(|t| t <= now);
        if dc {
            self.didchange_at = None;
        }
        let ss = self.session_save_at.is_some_and(|t| t <= now);
        if ss {
            self.session_save_at = None;
        }
        (ac, dc, ss)
    }
}
