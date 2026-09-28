//! Horizontal editor scrollbar, drawn with the editor's source-column units.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::app::model::Model;

const MIN_THUMB_WIDTH: usize = 5;

fn scrollbar_symbols(ascii: bool) -> (&'static str, &'static str) {
    if ascii { ("-", "=") } else { ("─", "▬") }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HorizontalScrollMetrics {
    pub thumb_start: usize,
    pub thumb_len: usize,
    pub max_offset: usize,
}

/// Computes the same proportional thumb used by the vertical scrollbar, with
/// a minimum width so the horizontal thumb remains practical to drag.
pub(crate) fn horizontal_scroll_metrics(
    content_len: usize,
    viewport_len: usize,
    offset: usize,
    track_len: usize,
) -> HorizontalScrollMetrics {
    if track_len == 0 {
        return HorizontalScrollMetrics {
            thumb_start: 0,
            thumb_len: 0,
            max_offset: 0,
        };
    }
    let max_offset = content_len.saturating_sub(viewport_len);
    if max_offset == 0 {
        return HorizontalScrollMetrics {
            thumb_start: 0,
            thumb_len: track_len,
            max_offset,
        };
    }

    let proportional = viewport_len
        .saturating_mul(track_len)
        .checked_div(content_len.max(1))
        .unwrap_or(0);
    let thumb_len = proportional.max(1).max(MIN_THUMB_WIDTH.min(track_len));
    let travel = track_len.saturating_sub(thumb_len);
    let thumb_start = offset
        .min(max_offset)
        .saturating_mul(travel)
        .checked_div(max_offset)
        .unwrap_or(0)
        .min(travel);
    HorizontalScrollMetrics {
        thumb_start,
        thumb_len,
        max_offset,
    }
}

pub(crate) fn render_hscrollbar(frame: &mut Frame, area: Rect, model: &Model) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let th = &model.theme;
    let Some(buf) = model.active_buffer() else {
        frame.render_widget(
            ratatui::widgets::Paragraph::new("").style(Style::new().bg(th.bg_alt)),
            area,
        );
        return;
    };

    let track_len = area.width as usize;
    let metrics = horizontal_scroll_metrics(
        super::horizontal_content_len(model),
        track_len,
        buf.scroll_x,
        track_len,
    );
    let (track, thumb) = scrollbar_symbols(model.ascii_icons);
    let cells = frame.buffer_mut();
    for col in 0..track_len {
        let in_thumb = col >= metrics.thumb_start
            && col < metrics.thumb_start.saturating_add(metrics.thumb_len);
        if let Some(cell) = cells.cell_mut((area.x + col as u16, area.y)) {
            cell.set_symbol(if in_thumb { thumb } else { track });
            let style = if in_thumb {
                Style::new()
                    .fg(th.accent)
                    .bg(th.bg_alt)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().fg(th.border).bg(th.bg_alt)
            };
            cell.set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scrollbar_symbols;

    #[test]
    fn ascii_scrollbar_has_distinct_non_hash_track_and_thumb() {
        assert_eq!(scrollbar_symbols(true), ("-", "="));
    }

    #[test]
    fn unicode_scrollbar_keeps_existing_symbols() {
        assert_eq!(scrollbar_symbols(false), ("─", "▬"));
    }
}
