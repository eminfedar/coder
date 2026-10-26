//! Application messages (the `Msg` of the Elm Architecture).

use std::path::PathBuf;

use crossterm::event::{KeyEvent, MouseEvent};

use crate::core::highlight::HlLine;
use crate::services::git::{CommitDiff, GitStatus};
use crate::services::lsp::{CompletionItem, LspHandle, RawDiagnostic, RawTextEdit, Token};
use crate::services::pty::PtySession;
use crate::services::search::SearchMatch;

pub enum Msg {
    // Input events
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),
    /// A whole block of text pasted into the terminal (bracketed paste),
    /// delivered as one atomic event instead of a flood of key events — see
    /// `main::map_event`.
    Paste(String),
    /// The terminal event stream ended or errored — exit the main loop.
    Quit,

    // Async results
    DirScanned {
        path: PathBuf,
        entries: Vec<(PathBuf, bool)>,
    },
    FileLoaded {
        path: PathBuf,
        text: String,
    },
    /// A file that could not be opened (binary / unreadable): opens a read-only
    /// tab showing the error message instead of surfacing it only on the statusbar.
    FileLoadFailed {
        path: PathBuf,
        error: String,
    },
    FileSaved {
        path: PathBuf,
        /// The text that was written: a tab is marked clean only if it still
        /// holds exactly this (typing during the async write keeps it dirty).
        contents: String,
    },
    /// A file/directory was renamed on disk (open tabs under it move too).
    PathRenamed {
        from: PathBuf,
        to: PathBuf,
    },
    /// A file/directory was deleted on disk (its open tabs close).
    PathDeleted(PathBuf),
    GitStatusLoaded(GitStatus),
    SearchResults {
        query: String,
        matches: Vec<SearchMatch>,
    },

    /// The workspace file listing for the quickbar (from `Cmd::ListFiles`).
    FilesListed {
        paths: Vec<PathBuf>,
    },
    ReplaceDone {
        changed: Vec<PathBuf>,
        count: usize,
    },
    /// The last commit was undone (soft reset); carries its message to refill the box.
    /// A commit succeeded: only now is the message box cleared, so a failed
    /// commit keeps the message for another try.
    /// The editor paste's clipboard text (see `Cmd::ReadClipboard`).
    ClipboardRead(String),
    GitCommitted,
    GitCommitUndone {
        message: String,
    },
    /// The HEAD content of a file (for the change gutter).
    HeadTextLoaded {
        path: PathBuf,
        text: Option<String>,
    },
    /// The changes of a history commit, for its read-only "<hash> diff" tab.
    CommitDiffLoaded {
        hash: String,
        diff: CommitDiff,
    },
    /// A file changed on disk (from the filesystem watcher).
    DiskChanged(PathBuf),
    /// Fresh on-disk content for an externally-changed, unmodified open file.
    FileReloaded {
        path: PathBuf,
        text: String,
    },

    // LSP
    /// A language server was spawned; carries its intent-sender handle.
    LspSessionReady {
        language: String,
        handle: LspHandle,
    },
    /// The server finished its `initialize` handshake.
    LspInitialized {
        language: String,
    },
    /// Diagnostics for a file (raw LSP UTF-16 positions; converted in update).
    LspDiagnostics {
        path: PathBuf,
        diagnostics: Vec<RawDiagnostic>,
    },
    /// Completion results for an earlier request (guarded by `token`).
    LspCompletions {
        token: Token,
        items: Vec<CompletionItem>,
    },
    /// Formatting edits for an earlier request (guarded by `token`).
    LspFormatEdits {
        token: Token,
        edits: Vec<RawTextEdit>,
    },
    /// The server process exited.
    LspExited {
        language: String,
    },
    /// The server could not be started or errored fatally.
    LspError {
        language: String,
        message: String,
    },
    /// A standalone formatter produced new text for a file.
    FormatterOutput {
        path: PathBuf,
        text: String,
        token: Token,
        save_after: bool,
    },
    /// A standalone linter produced diagnostics as `(line0, col0, message)`.
    LinterDiagnostics {
        path: PathBuf,
        items: Vec<(usize, usize, String)>,
    },
    /// PATH-availability of tool binaries: `(command, is_installed)` pairs, for
    /// the Extensions panel's per-language status.
    ToolsChecked(Vec<(String, bool)>),

    // Terminal (each carries the `TerminalState::id` of its terminal tab)
    PtyReady(u64, PtySession),
    /// Wake-up: new output is buffered in the PTY session (`take_output`).
    PtyOutput(u64),
    PtyExited(u64),

    Error(String),
    /// Show a transient toast notification.
    Toast(String),
    /// The toast duration elapsed: clear the toast if it is actually expired.
    ToastExpired,
    /// The highlight worker finished a job: colored lines for `tab`/`version`,
    /// starting at buffer line `base`. Applied only if the buffer is still there.
    Highlighted {
        tab: usize,
        version: u64,
        base: usize,
        lines: Vec<HlLine>,
    },
    /// A session checkpoint write finished (see `Cmd::SaveSession`).
    SessionSaved(crate::services::session::SaveOutcome),
}
