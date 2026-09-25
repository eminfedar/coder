//! Mouse event routing: resize handles, editor selection, tabs, sidebar, scrollbar.

use super::*;

pub(super) fn handle_mouse(model: &mut Model, m: MouseEvent) -> Vec<Cmd> {
    let area = full_rect(model);
    let a = ui::compute_areas(model, area);
    let (x, y) = (m.column, m.row);

    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Resize handles first.
            if a.sidebar_open && x == a.sidebar_border_x {
                model.drag = Some(DragTarget::SidebarBorder);
                return Vec::new();
            }
            if a.terminal_open && y == a.terminal_border_y && x >= a.tabs.x {
                model.drag = Some(DragTarget::TerminalBorder);
                return Vec::new();
            }
            // The find widget floats over the editor; intercept its clicks.
            if model.find.open
                && let Some(hit) = ui::find::hit(model, a.editor, x, y)
            {
                return handle_find_hit(model, hit);
            }
            // Scrollbar thumb drag.
            if a.scrollbar.width > 0 && rect_contains(a.scrollbar, x, y) {
                model.drag = Some(DragTarget::Scrollbar);
                scrollbar_jump(model, &a, y);
                return Vec::new();
            }
            if a.scrollbar_x_track.height > 0 && rect_contains(a.scrollbar_x_track, x, y) {
                model.drag = Some(DragTarget::ScrollbarX);
                hscrollbar_jump(model, &a, x);
                return Vec::new();
            }
            // Terminal scrollbar (rightmost inner column of the terminal).
            if a.terminal_open
                && model.terminal.session.is_some()
                && rect_contains(a.terminal, x, y)
                && y > a.terminal.y
                && x == a.terminal.x + a.terminal.width - 1
            {
                model.focus = Focus::Terminal;
                model.drag = Some(DragTarget::TerminalScrollbar);
                terminal_scrollbar_jump(model, &a, y);
                return Vec::new();
            }
            mouse_click(model, &a, x, y)
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            match model.drag {
                Some(DragTarget::SidebarBorder) => {
                    let min_x = ui::ACTIVITY_WIDTH + 10;
                    let new_w = x.saturating_sub(ui::ACTIVITY_WIDTH).max(10);
                    if x > min_x {
                        model.layout.sidebar_width = new_w.min(model.term_size.0 / 2);
                    }
                }
                Some(DragTarget::TerminalBorder) => {
                    // Drag down -> terminal shrinks.
                    let bottom = model.term_size.1.saturating_sub(1); // statusbar
                    let new_h = bottom.saturating_sub(y).max(2);
                    model.layout.terminal_height = new_h.min(model.term_size.1.saturating_sub(4));
                    sync_terminal_size(model);
                }
                Some(DragTarget::EditorSelect) => {
                    let c = editor_cursor_at(model, &a, x, y);
                    if let Some(buf) = model.active_buffer_mut() {
                        buf.set_cursor(c, true); // anchor is kept -> the selection grows
                    }
                    ensure_cursor_visible(model);
                }
                Some(DragTarget::Scrollbar) => scrollbar_jump(model, &a, y),
                Some(DragTarget::ScrollbarX) => hscrollbar_jump(model, &a, x),
                Some(DragTarget::TerminalScrollbar) => terminal_scrollbar_jump(model, &a, y),
                Some(DragTarget::TerminalSelect) => {
                    if let Some((sr, sc, _, _)) = model.terminal.selection {
                        let (r, c) = terminal_cell(model, &a, x, y);
                        model.terminal.selection = Some((sr, sc, r, c));
                    }
                }
                None => {}
            }
            Vec::new()
        }
        MouseEventKind::Up(MouseButton::Left) => {
            let was_terminal_select = model.drag == Some(DragTarget::TerminalSelect);
            model.drag = None;
            // Selecting terminal text copies it to the clipboard immediately.
            // A plain click leaves a zero-span selection (start == end); only a
            // real drag across cells should copy.
            let dragged =
                matches!(model.terminal.selection, Some((r1, c1, r2, c2)) if (r1, c1) != (r2, c2));
            if was_terminal_select && dragged {
                let text = model.terminal.selected_text();
                if !text.is_empty() {
                    model.internal_clipboard = text.clone();
                    let toast = model.show_toast("Copied to clipboard");
                    return vec![Cmd::SetClipboard(text), toast];
                }
            }
            Vec::new()
        }
        // Middle-click anywhere on a tab closes it (like clicking its ✕).
        MouseEventKind::Down(MouseButton::Middle) => {
            if rect_contains(a.tabs, x, y)
                && let Some(hit) = ui::tabs::tab_at(model, a.tabs, x, y)
            {
                let i = match hit {
                    ui::tabs::TabHit::Select(i) | ui::tabs::TabHit::Close(i) => i,
                };
                return close_tab_with_dirty_check(model, i);
            }
            Vec::new()
        }
        // Right-click on a tab opens the tab menu; on a file-tree row, the
        // file menu.
        MouseEventKind::Down(MouseButton::Right) => {
            if rect_contains(a.tabs, x, y)
                && let Some(hit) = ui::tabs::tab_at(model, a.tabs, x, y)
            {
                let i = match hit {
                    ui::tabs::TabHit::Select(i) | ui::tabs::TabHit::Close(i) => i,
                };
                return open_tab_menu(model, i, x, y);
            }
            if a.sidebar_open
                && model.sidebar.active == Panel::Files
                && rect_contains(a.sidebar, x, y)
                && let Some(idx) = ui::sidebar::file_row_at(model, a.sidebar, y)
            {
                return open_file_menu(model, idx, x, y);
            }
            Vec::new()
        }
        MouseEventKind::ScrollDown => mouse_scroll(model, &a, x, y, 3),
        MouseEventKind::ScrollUp => mouse_scroll(model, &a, x, y, -3),
        _ => Vec::new(),
    }
}

fn rect_contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

/// Handles a click on the find widget.
fn handle_find_hit(model: &mut Model, hit: ui::find::FindHit) -> Vec<Cmd> {
    use ui::find::FindHit;
    match hit {
        FindHit::QueryField => {
            model.focus = Focus::Find;
            model.find.field = FindField::Query;
            model.find.query.cursor_to_end();
            Vec::new()
        }
        FindHit::ReplaceField => {
            model.focus = Focus::Find;
            model.find.field = FindField::Replace;
            model.find.replace.cursor_to_end();
            Vec::new()
        }
        FindHit::Prev => {
            find_step(model, -1);
            Vec::new()
        }
        FindHit::Next => {
            find_step(model, 1);
            Vec::new()
        }
        FindHit::Close => {
            close_find(model);
            Vec::new()
        }
        FindHit::ReplaceOne => find_replace_one(model),
        FindHit::ReplaceAll => find_replace_all(model),
    }
}

fn mouse_click(model: &mut Model, a: &ui::Areas, x: u16, y: u16) -> Vec<Cmd> {
    if rect_contains(a.activity, x, y) {
        if let Some(p) = ui::activity_bar::panel_at(a.activity, y) {
            return toggle_panel(model, p);
        }
        return Vec::new();
    }
    if a.sidebar_open && rect_contains(a.sidebar, x, y) {
        return sidebar_click(model, a, x, y);
    }
    if rect_contains(a.tabs, x, y) {
        match ui::tabs::tab_at(model, a.tabs, x, y) {
            Some(ui::tabs::TabHit::Close(i)) => return close_tab_with_dirty_check(model, i),
            Some(ui::tabs::TabHit::Select(i)) => {
                model.active_tab = Some(i);
                model.focus = Focus::Editor;
            }
            None => {}
        }
        return Vec::new();
    }
    if a.terminal_open && rect_contains(a.terminal, x, y) {
        model.focus = Focus::Terminal;
        if model.terminal.session.is_some() && y > a.terminal.y {
            model.drag = Some(DragTarget::TerminalSelect);
            let (r, c) = terminal_cell(model, a, x, y);
            model.terminal.selection = Some((r, c, r, c));
        }
        return Vec::new();
    }
    if rect_contains(a.editor, x, y) {
        model.focus = Focus::Editor;
        // Double-click (same cell within 400ms) selects the word under the cursor.
        let now = std::time::Instant::now();
        let double = model
            .last_click
            .map(|(t, cx, cy)| cx == x && cy == y && now.duration_since(t).as_millis() < 400)
            .unwrap_or(false);
        model.last_click = Some((now, x, y));
        let cur = editor_cursor_at(model, a, x, y);
        if let Some(buf) = model.active_buffer_mut() {
            if double {
                buf.select_word_at(cur);
            } else {
                buf.set_cursor(cur, false);
            }
        }
        // A single click starts a drag selection; a double-click keeps the word.
        if !double {
            model.drag = Some(DragTarget::EditorSelect);
        }
        ensure_cursor_visible(model);
        return Vec::new();
    }
    Vec::new()
}

/// Converts a mouse position to an editor (line, column) cursor. Maps the screen
/// row through the diff view so clicks land on the right buffer line even when
/// removed lines are woven in.
fn editor_cursor_at(model: &Model, a: &ui::Areas, x: u16, y: u16) -> Cursor {
    // Clamp y to the editor area (drift when dragging past the top/bottom edge).
    let ey = y.clamp(a.editor.y, a.editor.y + a.editor.height.saturating_sub(1));
    let line = model.screen_row_to_line((ey - a.editor.y) as usize);
    let scroll_x = model.active_buffer().map(|b| b.scroll_x).unwrap_or(0);
    let col = scroll_x + x.saturating_sub(a.editor_text_x) as usize;
    Cursor { line, col }
}

fn sidebar_click(model: &mut Model, a: &ui::Areas, x: u16, y: u16) -> Vec<Cmd> {
    match model.sidebar.active {
        Panel::Files => {
            use ui::sidebar::FileHit;
            // Header buttons (create in the workspace root) take priority.
            if let Some(hit) = ui::sidebar::files_header_hit(a.sidebar, x, y) {
                return match hit {
                    FileHit::NewFileRoot => new_root_entry_dialog(model, false),
                    FileHit::NewFolderRoot => new_root_entry_dialog(model, true),
                    _ => Vec::new(),
                };
            }
            match ui::sidebar::file_hit(model, a.sidebar, x, y) {
                // A folder toggles; a file is previewed and the keyboard stays
                // in the tree (Enter/→ moves focus to the editor).
                Some(FileHit::Row(idx)) => {
                    model.sidebar.files.selected = idx;
                    model.focus = Focus::Sidebar;
                    let is_dir = model
                        .sidebar
                        .files
                        .visible_rows()
                        .get(idx)
                        .is_some_and(|r| r.is_dir);
                    if is_dir {
                        activate_selection(model)
                    } else {
                        preview_selection(model)
                    }
                }
                Some(FileHit::NewFile(idx)) => new_entry_dialog(model, idx, false),
                Some(FileHit::NewFolder(idx)) => new_entry_dialog(model, idx, true),
                Some(FileHit::NewFileRoot) => new_root_entry_dialog(model, false),
                Some(FileHit::NewFolderRoot) => new_root_entry_dialog(model, true),
                None => Vec::new(),
            }
        }
        Panel::Git => {
            use ui::sidebar::GitHit;
            // A click also moves the keyboard zone, so Tab continues from where
            // the mouse left off instead of from a stale zone.
            match ui::sidebar::git_hit(model, a.sidebar, x, y) {
                // A click previews (like the arrow keys) and keeps the keyboard
                // in the panel; Enter/→ is what moves focus to the editor.
                Some(GitHit::Entry(idx)) => {
                    model.sidebar.git.selected = idx;
                    set_git_zone(model, GitZone::Files);
                    preview_selection(model)
                }
                Some(GitHit::Stage(rel)) => {
                    set_git_zone(model, GitZone::Files);
                    vec![Cmd::GitStage(rel)]
                }
                Some(GitHit::Unstage(rel)) => {
                    set_git_zone(model, GitZone::Files);
                    vec![Cmd::GitUnstage(rel)]
                }
                Some(GitHit::StageAll) => {
                    set_git_zone(model, GitZone::Files);
                    vec![Cmd::GitStageAll]
                }
                Some(GitHit::UnstageAll) => {
                    set_git_zone(model, GitZone::Files);
                    vec![Cmd::GitUnstageAll]
                }
                Some(GitHit::Revert(rel)) => {
                    set_git_zone(model, GitZone::Files);
                    model.dialog = Some(Dialog::ask(
                        "Revert changes".to_string(),
                        format!(
                            "Changes in '{rel}' will be reverted. This cannot be undone. Are you sure?"
                        ),
                        DialogAction::GitRevert(rel),
                    ));
                    Vec::new()
                }
                Some(GitHit::CommitInput) => {
                    set_git_zone(model, GitZone::Message);
                    Vec::new()
                }
                Some(GitHit::Refresh) => {
                    set_git_zone(model, GitZone::Files);
                    model.notify("Refreshing…".to_string());
                    vec![Cmd::LoadGitStatus]
                }
                // The file icon opens the plain file, not the diff view (also
                // as a preview, keyboard stays in the panel).
                Some(GitHit::OpenFile(rel)) => {
                    set_git_zone(model, GitZone::Files);
                    preview(model, PreviewKey::File(model.root.join(rel)))
                }
                // The buttons share their enabled/disabled rules with the
                // keyboard, so both routes go through `git_button`.
                Some(GitHit::CommitButton) => {
                    set_git_zone(model, GitZone::Commit);
                    git_button(model, GitZone::Commit)
                }
                Some(GitHit::UndoLastCommit) => {
                    set_git_zone(model, GitZone::Uncommit);
                    git_button(model, GitZone::Uncommit)
                }
                Some(GitHit::Fetch) => {
                    set_git_zone(model, GitZone::Fetch);
                    git_button(model, GitZone::Fetch)
                }
                Some(GitHit::Pull) => {
                    set_git_zone(model, GitZone::Pull);
                    git_button(model, GitZone::Pull)
                }
                Some(GitHit::Push) => {
                    set_git_zone(model, GitZone::Push);
                    git_button(model, GitZone::Push)
                }
                None => Vec::new(),
            }
        }
        Panel::Search => {
            use ui::sidebar::SearchHit;
            match ui::sidebar::search_hit(model, a.sidebar, x, y) {
                Some(SearchHit::QueryField) => {
                    model.sidebar.search.field = SearchField::Query;
                    model.focus = Focus::SearchInput;
                }
                Some(SearchHit::ReplaceField) => {
                    model.sidebar.search.field = SearchField::Replace;
                    model.focus = Focus::SearchInput;
                }
                Some(SearchHit::ReplaceModeToggle) => model.sidebar.search.toggle_replace_mode(),
                Some(SearchHit::RegexToggle) => {
                    model.sidebar.search.use_regex = !model.sidebar.search.use_regex;
                    return rerun_search(model);
                }
                Some(SearchHit::MatchCaseToggle) => {
                    model.sidebar.search.match_case = !model.sidebar.search.match_case;
                    return rerun_search(model);
                }
                Some(SearchHit::SearchHiddenToggle) => {
                    model.sidebar.search.search_hidden = !model.sidebar.search.search_hidden;
                    return rerun_search(model);
                }
                Some(SearchHit::ReplaceOne) => return search_replace_one(model),
                Some(SearchHit::ReplaceAll) => return search_replace_all(model),
                Some(SearchHit::Result(idx)) => {
                    model.sidebar.search.selected = idx;
                    model.focus = Focus::Sidebar;
                    return preview_selection(model);
                }
                None => model.focus = Focus::Sidebar,
            }
            Vec::new()
        }
        Panel::Themes => {
            if let Some(i) = ui::sidebar::theme_row_at(model, a.sidebar, y) {
                model.focus = Focus::Sidebar;
                model.apply_theme(i);
                return persist_config(model);
            }
            Vec::new()
        }
        Panel::Settings => {
            if let Some(i) = ui::sidebar::settings_row_at(a.sidebar, y) {
                model.focus = Focus::Sidebar;
                model.sidebar.settings_selected = i;
                // An "Edit ..." row previews its file; the click keeps the
                // keyboard in the panel (Enter opens it and focuses the editor).
                use ui::sidebar::SettingsItem;
                let file = match ui::sidebar::SETTINGS_ITEMS.get(i) {
                    Some(SettingsItem::EditConfig) => config_file(model),
                    Some(SettingsItem::EditKeybindings) => keybindings_file(model),
                    _ => return activate_settings(model),
                };
                return file.map_or_else(Vec::new, |p| preview(model, PreviewKey::File(p)));
            }
            Vec::new()
        }
        // Extensions has no clickable rows.
        Panel::Extensions => Vec::new(),
    }
}

/// Jumps the editor scroll so the clicked scrollbar row is centered in the viewport.
fn scrollbar_jump(model: &mut Model, a: &ui::Areas, y: u16) {
    let h = a.scrollbar.height as usize;
    if h == 0 {
        return;
    }
    let row = y.saturating_sub(a.scrollbar.y) as usize;
    // A diff tab with woven deletions draws its track over *display* rows
    // (see `ui::editor::render_scrollbar`): map through them so the click lands
    // where the thumb is drawn, then snap to the first real line at/after it.
    if model.has_inline_deletions() {
        let rows = model.diff_rows();
        let n = rows.len().max(1);
        let target = (row * n / h).saturating_sub(h / 2).min(n.saturating_sub(h));
        let line = rows[target.min(rows.len().saturating_sub(1))..]
            .iter()
            .find_map(|r| match r {
                crate::app::model::DiffRow::Real(l) => Some(*l),
                crate::app::model::DiffRow::Deleted(_) => None,
            });
        if let (Some(line), Some(buf)) = (line, model.active_buffer_mut()) {
            buf.scroll_y = line;
        }
        return;
    }
    if let Some(buf) = model.active_buffer_mut() {
        let n = buf.line_count().max(1);
        let target = row * n / h;
        let max = n.saturating_sub(h);
        buf.scroll_y = target.saturating_sub(h / 2).min(max);
    }
}

/// Jumps the horizontal viewport so the clicked scrollbar position is centered.
fn hscrollbar_jump(model: &mut Model, a: &ui::Areas, x: u16) {
    let track = a.scrollbar_x_track;
    let track_len = track.width as usize;
    if track_len == 0 {
        return;
    }
    let total = ui::editor::horizontal_content_len(model);
    if let Some(buf) = model.active_buffer_mut() {
        let viewport = track_len;
        let metrics =
            ui::editor::horizontal_scroll_metrics(total, viewport, buf.scroll_x, track_len);
        let travel = track_len.saturating_sub(metrics.thumb_len);
        let col = x.saturating_sub(track.x) as usize;
        buf.scroll_x = col
            .saturating_sub(metrics.thumb_len / 2)
            .saturating_mul(metrics.max_offset)
            .checked_div(travel)
            .unwrap_or(0)
            .min(metrics.max_offset);
    }
}

fn mouse_scroll(model: &mut Model, a: &ui::Areas, x: u16, y: u16, delta: isize) -> Vec<Cmd> {
    if a.scrollbar_x_track.height > 0 && rect_contains(a.scrollbar_x_track, x, y) {
        let total = ui::editor::horizontal_content_len(model);
        if let Some(buf) = model.active_buffer_mut() {
            let viewport = a.scrollbar_x_track.width as usize;
            let max = total.saturating_sub(viewport);
            let step = (viewport / 3).max(1) as isize;
            buf.scroll_x = (buf.scroll_x as isize + delta * step).clamp(0, max as isize) as usize;
        }
        return Vec::new();
    }
    let over_scrollbar = a.scrollbar.width > 0 && rect_contains(a.scrollbar, x, y);
    if rect_contains(a.editor, x, y) || over_scrollbar {
        if let Some(buf) = model.active_buffer_mut() {
            let max = buf.line_count().saturating_sub(1);
            let new = (buf.scroll_y as isize + delta).clamp(0, max as isize) as usize;
            buf.scroll_y = new;
        }
    } else if a.terminal_open && rect_contains(a.terminal, x, y) {
        // Wheel up (delta < 0) scrolls back into history (offset grows).
        model.terminal.scroll_by(-delta);
    } else if a.sidebar_open && rect_contains(a.sidebar, x, y) {
        nav(model, delta.signum());
        // Scrolling through the Themes panel live-previews each theme the same
        // way arrow-key nav does (see `nav`'s `Panel::Themes` arm) — it must be
        // persisted the same way too, or the choice is only visual and reverts
        // to the last actually-saved theme on the next launch.
        return post_nav_persist(model);
    }
    Vec::new()
}

/// Maps a screen (x, y) to a clamped visible-grid (row, col) inside the
/// terminal. The terminal has a 1-row top border and a 1-column scrollbar on
/// the right, so the text area is inset accordingly.
fn terminal_cell(model: &Model, a: &ui::Areas, x: u16, y: u16) -> (u16, u16) {
    let (rows, cols) = model.terminal.parser.screen().size();
    let row = y
        .saturating_sub(a.terminal.y + 1)
        .min(rows.saturating_sub(1));
    let col = x.saturating_sub(a.terminal.x).min(cols.saturating_sub(1));
    (row, col)
}

/// Jumps the terminal scrollback so the clicked row of the scrollbar track maps
/// to that position in the history.
fn terminal_scrollbar_jump(model: &mut Model, a: &ui::Areas, y: u16) {
    let h = a.terminal.height.saturating_sub(1) as usize; // inner height
    if h == 0 {
        return;
    }
    let rows = model.terminal.rows as usize;
    let total = model.terminal.scrollback_lines + rows;
    let track_row = y.saturating_sub(a.terminal.y + 1) as usize;
    // Row in [0, total): top of the viewport we want.
    let above = (track_row * total / h).min(model.terminal.scrollback_lines);
    // offset = history rows below the viewport top.
    let offset = model.terminal.scrollback_lines.saturating_sub(above);
    model.terminal.scroll_to(offset);
}

#[cfg(test)]
mod mouse_scroll_tests {
    use super::*;
    use crate::app::model::Panel;

    #[test]
    fn scrolling_the_themes_panel_persists_like_arrow_nav_does() {
        // Regression test: scrolling the mouse wheel over the Themes panel must
        // save the newly-previewed theme (`Cmd::SaveConfig`), the same way
        // arrow-key navigation already does via `post_nav_persist` — otherwise
        // the choice is only a live preview that reverts on the next launch.
        let mut model = Model::new(std::env::temp_dir());
        model.sidebar.active = Panel::Themes;
        assert!(
            model.sidebar.themes.names.len() > 1,
            "need at least 2 themes for scrolling to change the selection"
        );
        let area = full_rect(&model);
        let a = ui::compute_areas(&model, area);

        let cmds = mouse_scroll(&mut model, &a, a.sidebar.x + 1, a.sidebar.y + 1, 1);

        assert!(
            cmds.iter().any(|c| matches!(c, Cmd::SaveConfig(_))),
            "scrolling the Themes panel must persist the selection"
        );
    }
}
