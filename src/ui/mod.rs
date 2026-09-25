//! view(): draws the Model with ratatui + layout computation (shared with mouse hit-testing).

pub mod activity_bar;
pub mod completion;
pub mod context_menu;
pub mod dialog;
pub mod editor;
pub mod find;
pub mod quickbar;
pub mod sidebar;
pub mod statusbar;
pub mod tabs;
pub mod terminal;
pub mod text_input;
pub mod toast;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::model::Model;

/// Region rectangles computed during render; reused for mouse events.
pub struct Areas {
    pub activity: Rect,
    pub sidebar: Rect,
    pub sidebar_open: bool,
    /// x coordinate of the sidebar/editor border (drag-resize handle).
    pub sidebar_border_x: u16,
    pub tabs: Rect,
    pub editor: Rect,
    /// Editor scrollbar column (rightmost); zero width when there's no room.
    pub scrollbar: Rect,
    /// Horizontal track, excluding the gutter and vertical-scrollbar corner.
    pub scrollbar_x_track: Rect,
    pub terminal: Rect,
    pub terminal_open: bool,
    /// y coordinate of the terminal top edge (drag-resize handle).
    pub terminal_border_y: u16,
    pub statusbar: Rect,
    /// x where the editor text begins (after the gutter).
    pub editor_text_x: u16,
    pub gutter_w: u16,
}

pub const ACTIVITY_WIDTH: u16 = 4;
pub const SCROLLBAR_WIDTH: u16 = 1;

/// Gutter width based on the active buffer's line count.
pub fn gutter_width(model: &Model) -> u16 {
    let digits = model.max_gutter_number().to_string().len() as u16;
    // One extra column for the git change marker when the file is tracked.
    let git = if model.git_gutter() { 1 } else { 0 };
    (digits + 2).max(4) + git
}

fn needs_hscrollbar(model: &Model, full_width: u16, gutter_w: u16) -> bool {
    let vertical_w = if full_width > gutter_w + SCROLLBAR_WIDTH {
        SCROLLBAR_WIDTH
    } else {
        0
    };
    let text_w = full_width
        .saturating_sub(gutter_w)
        .saturating_sub(vertical_w) as usize;
    editor::horizontal_content_len(model) > text_w
}

/// Computes the layout. view() and mouse routing use the same result.
pub fn compute_areas(model: &Model, area: Rect) -> Areas {
    let [main, statusbar] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);

    let [activity, rest] =
        Layout::horizontal([Constraint::Length(ACTIVITY_WIDTH), Constraint::Min(0)]).areas(main);

    let (sidebar, editor_col) = if model.layout.sidebar_open {
        let [sb, ec] = Layout::horizontal([
            Constraint::Length(model.layout.sidebar_width),
            Constraint::Min(0),
        ])
        .areas(rest);
        (sb, ec)
    } else {
        (Rect { width: 0, ..rest }, rest)
    };

    let (editor_body, terminal) = if model.layout.terminal_open {
        let th = model
            .layout
            .terminal_height
            .min(editor_col.height.saturating_sub(3));
        let [tabs_and_body, term] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(th.max(1))]).areas(editor_col);
        (tabs_and_body, term)
    } else {
        (
            editor_col,
            Rect {
                height: 0,
                ..editor_col
            },
        )
    };

    let [tabs, editor_full] =
        Layout::vertical([Constraint::Length(tabs::TAB_BAR_HEIGHT), Constraint::Min(0)])
            .areas(editor_body);

    let gutter_w = gutter_width(model);

    // Add a bottom horizontal-scroll row only when content extends beyond the
    // editor's text width and there's enough height to keep one row visible.
    let (editor_body, scrollbar_x) =
        if editor_full.height > 1 && needs_hscrollbar(model, editor_full.width, gutter_w) {
            let [body, horizontal] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(editor_full);
            (body, horizontal)
        } else {
            (
                editor_full,
                Rect {
                    height: 0,
                    ..editor_full
                },
            )
        };

    // Preserve the upstream vertical scrollbar geometry inside the editor body.
    let (editor, scrollbar) = if editor_body.width > gutter_w + SCROLLBAR_WIDTH {
        let [e, sb] = Layout::horizontal([Constraint::Min(1), Constraint::Length(SCROLLBAR_WIDTH)])
            .areas(editor_body);
        (e, sb)
    } else {
        (
            editor_body,
            Rect {
                width: 0,
                ..editor_body
            },
        )
    };

    let scrollbar_x_track =
        if scrollbar_x.height > 0 && scrollbar_x.width > gutter_w + scrollbar.width {
            Rect {
                x: scrollbar_x.x + gutter_w,
                width: scrollbar_x.width - gutter_w - scrollbar.width,
                ..scrollbar_x
            }
        } else {
            Rect {
                width: 0,
                height: 0,
                ..scrollbar_x
            }
        };

    let editor_text_x = editor.x + gutter_w;

    Areas {
        activity,
        sidebar,
        sidebar_open: model.layout.sidebar_open,
        sidebar_border_x: editor_col.x,
        tabs,
        editor,
        scrollbar,
        scrollbar_x_track,
        terminal,
        terminal_open: model.layout.terminal_open,
        terminal_border_y: terminal.y,
        statusbar,
        editor_text_x,
        gutter_w,
    }
}

pub fn view(frame: &mut Frame, model: &Model) {
    let area = frame.area();
    let a = compute_areas(model, area);

    // Clear the background.
    let bg = ratatui::widgets::Block::new().style(ratatui::style::Style::new().bg(model.theme.bg));
    frame.render_widget(bg, area);

    activity_bar::render(frame, a.activity, model);
    if a.sidebar_open && a.sidebar.width > 0 {
        sidebar::render(frame, a.sidebar, model);
    }
    tabs::render(frame, a.tabs, model);
    editor::render(frame, a.editor, model, a.gutter_w);
    if a.scrollbar.width > 0 {
        editor::render_scrollbar(frame, a.scrollbar, model);
    }
    if a.scrollbar_x_track.height > 0 {
        editor::render_hscrollbar(frame, a.scrollbar_x_track, model);
    }
    // The find widget floats over the top-right of the editor.
    find::render(frame, a.editor, model);
    // The completion popup floats at the cursor.
    completion::render(frame, a.editor, a.gutter_w, model);
    if a.terminal_open && a.terminal.height > 0 {
        terminal::render(frame, a.terminal, model);
    }
    statusbar::render(frame, a.statusbar, model);

    // The file-tree context menu floats over everything but the dialog.
    if model.context_menu.is_some() {
        context_menu::render(frame, model);
    }

    // Modal dialog on top.
    if model.dialog.is_some() {
        dialog::render(frame, model);
    }

    // Quickbar is the topmost overlay (above even the dialog).
    if model.quickbar.is_some() {
        quickbar::render(frame, model);
    }

    // Toast floats bottom-center over everything.
    toast::render(
        frame,
        area,
        model,
        (a.scrollbar_x_track.height > 0).then_some(a.scrollbar_x_track.y),
    );
}
