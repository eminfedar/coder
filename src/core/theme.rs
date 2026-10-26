//! VSCode Dark+ style color palette.

use ratatui::style::Color;

pub struct Theme {
    pub bg: Color,
    pub bg_alt: Color,
    pub fg: Color,
    pub fg_dim: Color,
    pub accent: Color,
    pub selection: Color,
    pub activity_bg: Color,
    pub statusbar_bg: Color,
    pub statusbar_fg: Color,
    pub tab_active_bg: Color,
    pub tab_inactive_bg: Color,
    pub border: Color,
    pub line_number: Color,
    pub git_added: Color,
    pub git_modified: Color,
    pub git_deleted: Color,
    pub git_untracked: Color,
    /// Subtle line background for added/modified lines (diff view in the editor).
    pub diff_add_bg: Color,
    /// Subtle line background for removed lines (diff view in the editor).
    pub diff_del_bg: Color,
    /// Background of every in-editor find match (yellow).
    pub find_match: Color,
    /// Background of the active find match (blue).
    pub find_current: Color,
}

/// Scales each RGB channel by `f`, clamped to 0..=255 (non-RGB colors pass through).
fn scale(c: Color, f: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * f).min(255.0) as u8,
            (g as f32 * f).min(255.0) as u8,
            (b as f32 * f).min(255.0) as u8,
        ),
        other => other,
    }
}

impl Theme {
    /// Background for text-input fields: 20% darker than the panel background so
    /// inputs read as sunken even when not focused.
    pub fn input_bg(&self) -> Color {
        scale(self.bg_alt, 0.7)
    }

    /// Background of an active status-bar button: the bar color pushed away from
    /// `statusbar_fg` (darker under light text, lighter under dark text), so the
    /// label keeps its color and stays readable.
    pub fn statusbar_active_bg(&self) -> Color {
        let light_fg = match self.statusbar_fg {
            Color::Rgb(r, g, b) => r as u32 + g as u32 + b as u32 > 3 * 128,
            _ => true,
        };
        scale(self.statusbar_bg, if light_fg { 0.7 } else { 1.3 })
    }

    /// Background of the selected search result: panel background darkened 30%.
    pub fn selected_bg(&self) -> Color {
        scale(self.bg_alt, 0.7)
    }

    /// Blends `c` over the editor background at `alpha` (0 = bg, 1 = `c`),
    /// yielding a translucent tint like the git diff-row backgrounds.
    pub fn tint(&self, c: Color, alpha: f32) -> Color {
        match (self.bg, c) {
            (Color::Rgb(br, bg, bb), Color::Rgb(r, g, b)) => {
                let mix =
                    |base: u8, top: u8| (base as f32 + (top as f32 - base as f32) * alpha) as u8;
                Color::Rgb(mix(br, r), mix(bg, g), mix(bb, b))
            }
            _ => c,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            bg: Color::Rgb(30, 30, 30),
            bg_alt: Color::Rgb(37, 37, 38),
            fg: Color::Rgb(212, 212, 212),
            fg_dim: Color::Rgb(133, 133, 133),
            accent: Color::Rgb(0, 122, 204),
            selection: Color::Rgb(38, 79, 120),
            activity_bg: Color::Rgb(51, 51, 51),
            statusbar_bg: Color::Rgb(0, 122, 204),
            statusbar_fg: Color::Rgb(255, 255, 255),
            tab_active_bg: Color::Rgb(30, 30, 30),
            tab_inactive_bg: Color::Rgb(45, 45, 45),
            border: Color::Rgb(64, 64, 64),
            line_number: Color::Rgb(133, 133, 133),
            git_added: Color::Rgb(129, 184, 139),
            git_modified: Color::Rgb(226, 192, 141),
            git_deleted: Color::Rgb(199, 118, 117),
            git_untracked: Color::Rgb(115, 201, 145),
            diff_add_bg: Color::Rgb(32, 51, 37),
            diff_del_bg: Color::Rgb(60, 34, 34),
            find_match: Color::Rgb(138, 114, 0),
            find_current: Color::Rgb(21, 94, 170),
        }
    }
}
