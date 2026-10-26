//! update(): takes a Msg, updates the Model, returns side-effect Cmds.
//!
//! Split into focused submodules by concern. This file holds the top-level
//! Msg dispatcher plus the shared imports every submodule pulls in via
//! `use super::*`.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::app::cmd::Cmd;
use crate::app::model::{
    ContextMenu, Dialog, DialogAction, DialogKind, DragTarget, FindField, Focus, GitZone, MenuItem,
    Model, Panel, PreviewKey, QuickbarItem, QuickbarState, SearchField, Tab,
};
use crate::app::msg::Msg;
use crate::core::buffer::{Buffer, Cursor};
use crate::core::keymap::{self, Action, Motion};
use crate::core::text_input::{InputOutcome, TextInputState};
use crate::ui;

mod action;
mod dialog;
mod editor;
mod find;
mod git;
mod input;
mod lsp;
mod menu;
mod mouse;
mod quickbar;
mod search;
mod session;
mod settings;
mod sidebar_nav;
mod tabs;
mod terminal;

use action::apply_action;
use dialog::{dialog_key, dialog_mouse, dialog_paste};
use editor::*;
use find::*;
use git::*;
use menu::{menu_key, menu_mouse, open_file_menu, open_tab_menu};
use mouse::handle_mouse;
use quickbar::{files_listed, open_quickbar, quickbar_key, quickbar_mouse, quickbar_paste};
use search::*;
use settings::*;
use sidebar_nav::*;
use tabs::*;
use terminal::{paste_into_terminal, sync_terminal_size};

/// Resolves `key` against the user's global shortcuts (Quit, Save, panel
/// switches, ...) and applies the action if there is one. Shared fallback for
/// every modal overlay (quickbar, dialog, context menu): a key the overlay
/// itself doesn't recognize falls through here instead of being silently
/// swallowed, so e.g. Ctrl+Q/Ctrl+S still work while one is open.
pub(super) fn overlay_fallback(model: &mut Model, key: KeyEvent) -> Vec<Cmd> {
    // Only the user-bound command table, never `keymap::resolve`'s hardcoded
    // typing/motion fallback — an overlay is open, so a plain letter must not
    // fall through into the editor buffer as text.
    match model.keybindings.resolve(key, Focus::Editor) {
        Some(action) => apply_action(model, action),
        None => Vec::new(),
    }
}

/// Loads and rebuilds the workspace session at startup (see `services::session`
/// and `update::session::restore`). Called once from `main::run`, before the
/// event loop starts.
pub fn restore_session(model: &mut Model) -> Vec<Cmd> {
    session::restore(model)
}

/// Fires debounced work whose deadline has elapsed. Called once per main-loop
/// iteration (not on a message) so autocomplete and `didChange` are throttled
/// without spawning a timer task per keystroke.
pub fn tick(model: &mut Model) -> Vec<Cmd> {
    let now = std::time::Instant::now();
    let (autocomplete, didchange, session_save) = model.take_due_timers(now);
    let mut cmds = Vec::new();
    if model
        .pending_format
        .as_ref()
        .is_some_and(|p| p.deadline <= now)
    {
        model.notify("Formatter timed out; saved without formatting".to_string());
        cmds.extend(lsp::abandon_format(model));
    }
    if autocomplete {
        // The completion request flushes the current text on its own, so a pending
        // didChange for the same edit is now redundant — drop it.
        model.cancel_didchange();
        cmds.extend(lsp::request_completion(model));
    }
    if didchange {
        cmds.extend(lsp::flush_didchange(model));
    }
    if session_save {
        cmds.push(Cmd::SaveSession {
            snapshot: model.session_snapshot(),
            seen: model.session_seen_generation.unwrap_or(0),
        });
    }
    cmds
}

pub fn update(model: &mut Model, msg: Msg) -> Vec<Cmd> {
    let cmds = dispatch(model, msg);
    // Cross-cutting invariant, enforced once here rather than in every handler
    // that can switch tabs or replace text.
    find::sync_find(model);
    cmds
}

/// Routes one `Msg` to its handler.
fn dispatch(model: &mut Model, msg: Msg) -> Vec<Cmd> {
    match msg {
        Msg::Key(key) => input::key(model, key),
        Msg::Mouse(m) => input::mouse(model, m),
        Msg::Paste(text) => input::paste(model, text),
        Msg::Resize(w, h) => {
            model.term_size = (w, h);
            sync_terminal_size(model);
            Vec::new()
        }
        Msg::Quit => {
            model.should_quit = true;
            Vec::new()
        }
        Msg::Highlighted {
            tab,
            version,
            base,
            lines,
        } => {
            // Only the active tab is highlighted; drop a result for a tab that has
            // since been switched away or closed.
            let active_id = model
                .active_tab
                .and_then(|i| model.tabs.get(i))
                .map(|t| t.id);
            if active_id == Some(tab) {
                model.set_display_hl(tab, version, base, lines);
            }
            Vec::new()
        }
        Msg::DirScanned { path, entries } => {
            model.sidebar.files.set_children(&path, entries);
            // A rescan after a delete can leave the selection past the last row.
            let len = model.sidebar.files.visible_rows().len();
            if model.sidebar.files.selected >= len {
                model.sidebar.files.selected = len.saturating_sub(1);
            }
            Vec::new()
        }
        Msg::FileLoaded { path, text } => file_loaded(model, path, text),
        Msg::FileLoadFailed { path, error } => file_load_failed(model, path, error),
        Msg::CommitDiffLoaded { hash, diff } => commit_diff_loaded(model, hash, diff),
        Msg::HeadTextLoaded { path, text } => git::head_text_loaded(model, path, text),
        Msg::DiskChanged(path) => disk_changed(model, path),
        Msg::FileReloaded { path, text } => apply_reload(model, path, text),
        Msg::PathRenamed { from, to } => path_renamed(model, from, to),
        Msg::PathDeleted(path) => path_deleted(model, path),
        Msg::FileSaved { path, contents } => file_saved(model, path, contents),
        Msg::GitStatusLoaded(status) => git::status_loaded(model, status),
        Msg::ClipboardRead(text) => paste_into_editor(model, &text),
        Msg::GitCommitted => {
            model.sidebar.git.commit.clear();
            vec![model.show_toast("Committed")]
        }
        Msg::GitCommitUndone { message } => {
            model.sidebar.git.commit.set_content(message); // moves caret to end
            // Land in the commit box so the restored message can be edited.
            set_git_zone(model, GitZone::Message);
            Vec::new()
        }
        Msg::SearchResults { query, matches } => search::search_results(model, query, matches),
        Msg::FilesListed { paths } => files_listed(model, paths),
        Msg::ReplaceDone { changed, count } => search::replace_done(model, changed, count),
        Msg::PtyReady(id, session) => terminal::pty_ready(model, id, session),
        // Output is buffered in the PTY session and drained there.
        Msg::PtyOutput(id) => terminal::pty_output(model, id),
        Msg::PtyExited(id) => terminal::pty_exited(model, id),
        Msg::LspSessionReady { language, handle } => {
            model.lsp.starting.remove(&language);
            model.lsp.sessions.insert(language, handle);
            Vec::new()
        }
        Msg::LspInitialized { language } => lsp::on_initialized(model, &language),
        Msg::LspDiagnostics { path, diagnostics } => {
            lsp::store_diagnostics(model, path, diagnostics);
            Vec::new()
        }
        Msg::LspCompletions { token, items } => lsp::completions_arrived(model, token, items),
        Msg::LspFormatEdits { token, edits } => lsp::format_edits_arrived(model, token, edits),
        Msg::LspExited { language } => {
            let cmds = lsp::remove_server(model, &language);
            model.notify(format!("Language server '{language}' stopped"));
            cmds
        }
        Msg::LspError { language, message } => {
            let cmds = lsp::remove_server(model, &language);
            model.notify(format!("LSP ({language}): {message}"));
            cmds
        }
        Msg::FormatterOutput {
            path,
            text,
            token,
            save_after,
        } => lsp::formatter_output(model, path, text, token, save_after),
        Msg::LinterDiagnostics { path, items } => lsp::linter_diagnostics(model, path, items),
        Msg::ToolsChecked(statuses) => {
            for (command, installed) in statuses {
                model.tool_available.insert(command, installed);
            }
            Vec::new()
        }
        Msg::Error(e) => {
            model.notify(format!("Error: {e}"));
            Vec::new()
        }
        Msg::Toast(s) => {
            model.show_toast(s);
            Vec::new()
        }
        Msg::ToastExpired => {
            // Only clear if actually expired: a newer toast raised in the meantime
            // has a later `shown_at` and must survive this stale timer.
            if model.toast.as_ref().is_some_and(|t| t.is_expired()) {
                model.toast = None;
            }
            Vec::new()
        }
        Msg::SessionSaved(outcome) => session::session_saved(model, outcome),
    }
}
