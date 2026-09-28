//! Transient toast notification, floated bottom-center over everything.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use crate::app::model::Model;

/// Draws the active toast (if any and not yet expired) centered along the bottom
/// of the whole screen. `area` is the full frame. When `avoid_row` is set, the
/// toast moves above that row if its normal position would cover it.
pub fn render(frame: &mut Frame, area: Rect, model: &Model, avoid_row: Option<u16>) {
    let Some(toast) = model.toast.as_ref() else {
        return;
    };
    if toast.is_expired() {
        return;
    }

    let th = &model.theme;
    let text = toast.message.as_str();
    // Box wide enough for the text plus horizontal padding and borders.
    // Clamped in `usize` before the cast so a huge message can't overflow `u16`.
    let inner_w = text.chars().count();
    let box_w = (inner_w + 4)
        .min(area.width.saturating_sub(2) as usize)
        .max(3) as u16;
    let box_h = 3;
    if area.height < box_h + 2 || area.width < box_w {
        return;
    }

    // Centered horizontally, a couple of rows above the very bottom edge.
    let x = area.x + (area.width.saturating_sub(box_w)) / 2;
    let mut y = area.y + area.height.saturating_sub(box_h + 1);
    if let Some(row) = avoid_row
        && row >= y
        && row < y.saturating_add(box_h)
    {
        let above = row.saturating_sub(box_h + 1);
        if above >= area.y {
            y = above;
        }
    }
    let rect = Rect {
        x,
        y,
        width: box_w,
        height: box_h,
    };

    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(th.accent))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(th.bg_alt));
    let p = Paragraph::new(Line::from(Span::styled(
        text.to_string(),
        Style::new().fg(th.fg).add_modifier(Modifier::BOLD),
    )))
    .block(block)
    .alignment(ratatui::layout::Alignment::Center);

    frame.render_widget(ratatui::widgets::Clear, rect);
    frame.render_widget(p, rect);
}
