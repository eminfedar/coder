//! Startup session restore: reopens the tabs the last run left open — or a
//! crash interrupted — including cursor/scroll and any unsaved ("dirty")
//! content. See `services::session` for the on-disk format and the
//! multi-instance/large-file notes.

use super::*;

use crate::services::session::{self, Content, TabEntry};

/// Loads the session for `model.root` (if any) and rebuilds its tabs. A tab
/// whose unsaved content is already known — an untitled buffer, or a dirty
/// file small enough to have been checkpointed as full text — is built
/// synchronously right here (this also covers a file deleted outside the
/// editor since: it still shows up, unsaved, because nothing needs to touch
/// disk). Everything else (a clean file, or a large dirty file stored as a
/// diff) needs its *current* on-disk content first, so it goes through the
/// ordinary `Cmd::ReadFile` -> `Msg::FileLoaded` path instead — see
/// `Model.pending_session_restore` for what gets applied once that arrives.
pub(super) fn restore(model: &mut Model) -> Vec<Cmd> {
    let Some(snap) = session::load(&model.root) else {
        model.session_seen_generation = Some(session::current_generation(&model.root));
        return Vec::new();
    };
    model.session_seen_generation = Some(snap.generation);
    model.untitled_seq = snap.untitled_seq;
    model.layout.sidebar_width = snap.sidebar_width;
    model.layout.terminal_open = snap.terminal_open;
    if let Some(p) = parse_panel(&snap.sidebar_panel) {
        model.sidebar.active = p;
    }

    let mut cmds = Vec::new();
    // Restoring the terminal panel open re-spawns its PTY too, the same way
    // `Action::ToggleTerminal` does when the user opens it by hand.
    if model.layout.terminal_open {
        super::terminal::sync_terminal_size(model);
        cmds.extend(super::terminal::spawn_active_terminal(model));
    }
    for entry in snap.tabs {
        match entry.kind.as_str() {
            "untitled" => restore_untitled(model, entry, snap.active.as_deref()),
            "file" => cmds.extend(restore_file(model, entry, snap.active.as_deref())),
            _ => {}
        }
    }
    cmds
}

/// Reverses the `format!("{:?}", panel)` used to write `sidebar_panel`.
fn parse_panel(name: &str) -> Option<Panel> {
    Panel::ALL.into_iter().find(|p| format!("{p:?}") == name)
}

/// Clamps a restored cursor/scroll onto a freshly built buffer's actual line
/// count, so a session written against a longer/shorter version of the text
/// never indexes past the end.
fn clamp_and_place(buf: &mut Buffer, line: usize, col: usize, scroll_y: usize, scroll_x: usize) {
    let last = buf.line_count().saturating_sub(1);
    let line = line.min(last);
    let col = col.min(buf.line_len(line));
    buf.cursor = Cursor { line, col };
    buf.scroll_y = scroll_y.min(last);
    buf.scroll_x = scroll_x;
}

fn restore_untitled(model: &mut Model, entry: TabEntry, active: Option<&str>) {
    let Some(id) = entry.untitled_id.clone() else {
        return;
    };
    let text = match entry.content {
        Some(Content::Full { text }) => text,
        _ => String::new(), // an untitled buffer always keeps full text; this is just a safe fallback
    };
    let mut tab = Tab::new(Buffer::new(None, &text));
    tab.buffer.dirty = entry.dirty;
    tab.untitled_id = Some(id.clone());
    tab.label = entry.label;
    clamp_and_place(
        &mut tab.buffer,
        entry.line,
        entry.col,
        entry.scroll_y,
        entry.scroll_x,
    );
    model.tabs.push(tab);
    if active == Some(format!("untitled:{id}").as_str()) {
        model.active_tab = Some(model.tabs.len() - 1);
        model.focus = Focus::Editor;
    }
}

/// Restores a `kind = "file"` entry: a dirty file small enough to have kept
/// its full text is built synchronously; a clean file or a large dirty file
/// (stored as hunks) waits for its current on-disk content via `Cmd::ReadFile`.
fn restore_file(model: &mut Model, entry: TabEntry, active: Option<&str>) -> Vec<Cmd> {
    let Some(path_str) = entry.path.clone() else {
        return Vec::new();
    };
    let path = PathBuf::from(&path_str);
    let is_active = active == Some(path_str.as_str());

    if let Some(Content::Full { text }) = &entry.content {
        let mut tab = Tab::new(Buffer::new(Some(path.clone()), text));
        tab.buffer.dirty = entry.dirty;
        clamp_and_place(
            &mut tab.buffer,
            entry.line,
            entry.col,
            entry.scroll_y,
            entry.scroll_x,
        );
        model.tabs.push(tab);
        let idx = model.tabs.len() - 1;
        if is_active {
            model.active_tab = Some(idx);
            model.focus = Focus::Editor;
        }
        let mut cmds = vec![Cmd::LoadHeadText(path)];
        cmds.extend(super::lsp::open_tab(model, idx));
        return cmds;
    }

    let dirty_hunks = match entry.content {
        Some(Content::Diff { hunks }) => Some(hunks),
        _ => None,
    };
    model.pending_session_restore.insert(
        path.clone(),
        crate::app::model::SessionRestore {
            line: entry.line,
            col: entry.col,
            scroll_y: entry.scroll_y,
            scroll_x: entry.scroll_x,
            dirty_hunks,
        },
    );
    if is_active {
        model.session_active_path = Some(path.clone());
    }
    vec![Cmd::ReadFile(path)]
}

/// A session checkpoint write finished (`Msg::SessionSaved`).
pub(super) fn session_saved(
    model: &mut Model,
    outcome: crate::services::session::SaveOutcome,
) -> Vec<Cmd> {
    use crate::services::session::SaveOutcome;
    match outcome {
        SaveOutcome::Saved(new_gen) => {
            model.session_seen_generation = Some(new_gen);
            Vec::new()
        }
        SaveOutcome::Conflict(disk_gen) => {
            // Another coder instance wrote a newer checkpoint since we last
            // checked: adopt its generation as our new baseline (so we don't
            // re-report the same conflict every checkpoint) and skip this
            // write rather than clobber it — see `services::session::save`'s
            // multi-instance note.
            model.session_seen_generation = Some(disk_gen);
            model.notify(
                "Session updated by another coder window; this checkpoint was skipped".to_string(),
            );
            Vec::new()
        }
        SaveOutcome::NoPath => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::msg::Msg;
    use crate::services::session::{self, Content, SessionSnapshot, TEST_ENV_LOCK, TabEntry};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("coder-restore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn snapshot(
        root: &std::path::Path,
        tabs: Vec<TabEntry>,
        active: Option<&str>,
    ) -> SessionSnapshot {
        SessionSnapshot {
            root: root.display().to_string(),
            generation: 0,
            active: active.map(str::to_string),
            sidebar_panel: "Files".into(),
            sidebar_width: 30,
            terminal_open: false,
            untitled_seq: 0,
            tabs,
        }
    }

    fn file_entry(
        path: &std::path::Path,
        dirty: bool,
        content: Option<Content>,
        line: usize,
    ) -> TabEntry {
        TabEntry {
            kind: "file".into(),
            path: Some(path.display().to_string()),
            untitled_id: None,
            label: None,
            line,
            col: 0,
            scroll_y: 0,
            scroll_x: 0,
            dirty,
            content,
        }
    }

    #[test]
    fn restore_reconstructs_a_dirty_file_even_if_it_was_deleted_since() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let root = temp_dir("deleted");
        unsafe {
            std::env::set_var("CODER_SESSION_DIR", &root);
        }
        let gone = root.join("gone.txt"); // never actually written to disk
        let key = gone.display().to_string();
        let mut snap = snapshot(
            &root,
            vec![file_entry(
                &gone,
                true,
                Some(Content::Full {
                    text: "hello\nworld\n".into(),
                }),
                1,
            )],
            Some(&key),
        );
        session::save(&root, &mut snap, 0);

        let mut model = Model::new(root.clone());
        let cmds = restore(&mut model);
        assert!(
            !cmds.iter().any(|c| matches!(c, Cmd::ReadFile(_))),
            "a full-content dirty tab restores synchronously — its content never needs a disk read"
        );
        assert_eq!(model.tabs.len(), 1);
        assert!(model.tabs[0].buffer.dirty);
        assert_eq!(model.tabs[0].buffer.full_text(), "hello\nworld\n");
        assert_eq!(model.active_tab, Some(0));

        unsafe {
            std::env::remove_var("CODER_SESSION_DIR");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_queues_a_clean_file_and_restores_cursor_once_it_loads() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let root = temp_dir("clean");
        unsafe {
            std::env::set_var("CODER_SESSION_DIR", &root);
        }
        let file = root.join("a.txt");
        std::fs::write(&file, "line1\nline2\nline3\n").unwrap();
        let key = file.display().to_string();
        let mut entry = file_entry(&file, false, None, 2);
        entry.col = 1;
        entry.scroll_y = 1;
        let mut snap = snapshot(&root, vec![entry], Some(&key));
        session::save(&root, &mut snap, 0);

        let mut model = Model::new(root.clone());
        let cmds = restore(&mut model);
        assert!(matches!(cmds.as_slice(), [Cmd::ReadFile(p)] if p == &file));
        assert!(
            model.tabs.is_empty(),
            "waits for the async read before creating the tab"
        );
        assert_eq!(model.session_active_path.as_deref(), Some(file.as_path()));

        let text = std::fs::read_to_string(&file).unwrap();
        update(
            &mut model,
            Msg::FileLoaded {
                path: file.clone(),
                text,
            },
        );
        assert_eq!(model.tabs.len(), 1);
        assert!(!model.tabs[0].buffer.dirty);
        assert_eq!(model.tabs[0].buffer.cursor.line, 2);
        assert_eq!(model.tabs[0].buffer.cursor.col, 1);
        assert_eq!(model.tabs[0].buffer.scroll_y, 1);
        assert_eq!(model.active_tab, Some(0));
        assert!(
            model.session_active_path.is_none(),
            "cleared once the target tab loaded"
        );

        unsafe {
            std::env::remove_var("CODER_SESSION_DIR");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn active_tab_targets_the_session_active_file_regardless_of_load_order() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let root = temp_dir("order");
        unsafe {
            std::env::set_var("CODER_SESSION_DIR", &root);
        }
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        std::fs::write(&a, "a\n").unwrap();
        std::fs::write(&b, "b\n").unwrap();
        let key_b = b.display().to_string();
        let mut snap = snapshot(
            &root,
            vec![
                file_entry(&a, false, None, 0),
                file_entry(&b, false, None, 0),
            ],
            Some(&key_b),
        );
        session::save(&root, &mut snap, 0);

        let mut model = Model::new(root.clone());
        restore(&mut model);
        // "a" happens to finish its async read first...
        update(
            &mut model,
            Msg::FileLoaded {
                path: a.clone(),
                text: "a\n".into(),
            },
        );
        let active_path = |m: &Model| {
            m.active_tab
                .and_then(|i| m.tabs.get(i))
                .and_then(|t| t.buffer.path.clone())
        };
        assert_ne!(
            active_path(&model).as_deref(),
            Some(a.as_path()),
            "must not claim focus for the non-active file"
        );
        // ...then "b" (the session's actual active file) arrives.
        update(
            &mut model,
            Msg::FileLoaded {
                path: b.clone(),
                text: "b\n".into(),
            },
        );
        assert_eq!(active_path(&model).as_deref(), Some(b.as_path()));

        unsafe {
            std::env::remove_var("CODER_SESSION_DIR");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_applies_stored_hunks_over_the_current_on_disk_content() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let root = temp_dir("diff");
        unsafe {
            std::env::set_var("CODER_SESSION_DIR", &root);
        }
        let file = root.join("big.rs");
        let base = "one\ntwo\nthree\n";
        let edited = "one\nTWO\nthree\nfour\n";
        std::fs::write(&file, base).unwrap();
        let hunks = crate::services::session::diff_hunks(base, edited).unwrap();
        let key = file.display().to_string();
        let mut snap = snapshot(
            &root,
            vec![file_entry(&file, true, Some(Content::Diff { hunks }), 1)],
            Some(&key),
        );
        session::save(&root, &mut snap, 0);

        let mut model = Model::new(root.clone());
        restore(&mut model);
        update(
            &mut model,
            Msg::FileLoaded {
                path: file.clone(),
                text: base.to_string(),
            },
        );
        assert_eq!(model.tabs.len(), 1);
        assert!(model.tabs[0].buffer.dirty);
        assert_eq!(model.tabs[0].buffer.full_text(), edited);

        unsafe {
            std::env::remove_var("CODER_SESSION_DIR");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_rebuilds_an_untitled_scratch_buffer() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let root = temp_dir("untitled");
        unsafe {
            std::env::set_var("CODER_SESSION_DIR", &root);
        }
        let mut snap = snapshot(
            &root,
            vec![TabEntry {
                kind: "untitled".into(),
                path: None,
                untitled_id: Some("abc".into()),
                label: Some("Untitled-3".into()),
                line: 0,
                col: 2,
                scroll_y: 0,
                scroll_x: 0,
                dirty: true,
                content: Some(Content::Full {
                    text: "sketch".into(),
                }),
            }],
            Some("untitled:abc"),
        );
        session::save(&root, &mut snap, 0);

        let mut model = Model::new(root.clone());
        let cmds = restore(&mut model);
        assert!(cmds.is_empty());
        assert_eq!(model.tabs.len(), 1);
        assert!(model.tabs[0].buffer.dirty);
        assert_eq!(model.tabs[0].buffer.full_text(), "sketch");
        assert_eq!(model.tabs[0].untitled_id.as_deref(), Some("abc"));
        assert_eq!(model.tabs[0].title(), "Untitled-3");
        assert_eq!(model.active_tab, Some(0));

        unsafe {
            std::env::remove_var("CODER_SESSION_DIR");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_reopens_the_terminal_panel_and_respawns_its_pty() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let root = temp_dir("terminal");
        unsafe {
            std::env::set_var("CODER_SESSION_DIR", &root);
        }
        let mut snap = snapshot(&root, Vec::new(), None);
        snap.terminal_open = true;
        session::save(&root, &mut snap, 0);

        let mut model = Model::new(root.clone());
        assert!(
            !model.layout.terminal_open,
            "starts closed, like any fresh workspace"
        );
        let cmds = restore(&mut model);
        assert!(model.layout.terminal_open);
        assert!(
            cmds.iter().any(|c| matches!(c, Cmd::SpawnPty { .. })),
            "reopening the panel must also respawn its PTY, like Action::ToggleTerminal does"
        );

        unsafe {
            std::env::remove_var("CODER_SESSION_DIR");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
