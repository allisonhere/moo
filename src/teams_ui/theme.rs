//! Tide-family design tokens for the Teams screen, derived from the app's existing `Palette`.
//!
//! Quiet chrome, a full-border focus indication held to a 7:1 contrast floor, soft panels, and
//! statuses that always carry a word or glyph in addition to a color.

use ratatui::style::Color;

use crate::app::state::Palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Compact,
    Comfortable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub density: Density,
    /// ASCII-only glyphs for terminals and fonts without box or symbol coverage.
    pub ascii: bool,
    /// Reverse-video focus on top of the colored border.
    pub high_contrast: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            density: Density::Compact,
            ascii: false,
            high_contrast: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    pub working: &'static str,
    pub waiting: &'static str,
    pub approval: &'static str,
    pub blocked: &'static str,
    pub idle: &'static str,
    pub paused: &'static str,
    pub rail: &'static str,
    pub ellipsis: &'static str,
    pub open: &'static str,
    pub closed: &'static str,
    pub bar_full: &'static str,
    pub bar_empty: &'static str,
}

impl Glyphs {
    fn unicode() -> Self {
        Self {
            working: "●",
            waiting: "◐",
            approval: "?",
            blocked: "✖",
            idle: "○",
            paused: "‖",
            rail: "▍",
            ellipsis: "…",
            open: "▾",
            closed: "▸",
            bar_full: "█",
            bar_empty: "░",
        }
    }

    fn ascii() -> Self {
        Self {
            working: "*",
            waiting: "~",
            approval: "?",
            blocked: "x",
            idle: "o",
            paused: "=",
            rail: ">",
            ellipsis: "...",
            open: "v",
            closed: ">",
            bar_full: "#",
            bar_empty: "-",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Tide {
    pub bg: Color,
    pub panel: Color,
    pub text: Color,
    pub muted: Color,
    pub dim: Color,
    pub accent: Color,
    pub good: Color,
    pub warn: Color,
    pub danger: Color,
    pub border: Color,
    pub border_focus: Color,
    pub selected_bg: Color,
    pub glyphs: Glyphs,
    pub options: Options,
}

impl Tide {
    pub fn from_palette(p: &Palette, options: Options) -> Self {
        let bg = p.panel_bg;
        let legible = |color: Color, min: f64| ensure_contrast(color, bg, min);
        Self {
            bg,
            panel: p.panel_bg,
            text: legible(p.text, 7.0),
            muted: legible(p.subtext0, 4.5),
            dim: legible(p.overlay1, 3.0),
            accent: legible(p.accent, 4.5),
            good: legible(p.green, 4.5),
            warn: legible(p.yellow, 4.5),
            danger: legible(p.red, 4.5),
            border: legible(p.surface1, 1.5),
            // Never hard to spot: the focused border clears 7:1 against the background.
            border_focus: legible(p.accent, 7.0),
            selected_bg: ensure_contrast(p.surface0, bg, 1.2),
            glyphs: if options.ascii {
                Glyphs::ascii()
            } else {
                Glyphs::unicode()
            },
            options,
        }
    }

    pub fn status_glyph(&self, status: super::data::AgentStatus) -> &'static str {
        use super::data::AgentStatus as S;
        match status {
            S::Working => self.glyphs.working,
            S::Waiting => self.glyphs.waiting,
            S::Approval => self.glyphs.approval,
            S::Blocked => self.glyphs.blocked,
            S::Idle => self.glyphs.idle,
            S::Suspended => self.glyphs.paused,
        }
    }

    pub fn status_color(&self, status: super::data::AgentStatus) -> Color {
        use super::data::AgentStatus as S;
        match status {
            S::Working => self.good,
            S::Waiting => self.accent,
            S::Approval => self.warn,
            S::Blocked => self.danger,
            S::Idle | S::Suspended => self.muted,
        }
    }
}

fn rgb(color: Color) -> (f64, f64, f64) {
    match color {
        Color::Rgb(r, g, b) => (f64::from(r), f64::from(g), f64::from(b)),
        Color::White => (255.0, 255.0, 255.0),
        // Reset and indexed colors have no known value; assume a dark terminal.
        _ => (0.0, 0.0, 0.0),
    }
}

fn luminance(color: Color) -> f64 {
    let channel = |value: f64| {
        let value = value / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = rgb(color);
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

pub fn contrast(a: Color, b: Color) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// Moves `fg` toward white (on a dark background) or black (on a light one) until it clears `min`.
pub fn ensure_contrast(fg: Color, bg: Color, min: f64) -> Color {
    if contrast(fg, bg) >= min {
        return fg;
    }
    let towards = if luminance(bg) < 0.5 { 255.0 } else { 0.0 };
    let (r, g, b) = rgb(fg);
    for step in 1..=20 {
        let mix = f64::from(step) * 0.05;
        let shift = |value: f64| (value + (towards - value) * mix).round().clamp(0.0, 255.0) as u8;
        let candidate = Color::Rgb(shift(r), shift(g), shift(b));
        if contrast(candidate, bg) >= min {
            return candidate;
        }
    }
    if towards > 0.0 {
        Color::Rgb(255, 255, 255)
    } else {
        Color::Rgb(0, 0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_border_clears_seven_to_one_on_every_builtin_palette() {
        for palette in [Palette::catppuccin(), Palette::catppuccin_latte()] {
            let tide = Tide::from_palette(&palette, Options::default());
            assert!(
                contrast(tide.border_focus, tide.bg) >= 7.0,
                "{:?}",
                palette.accent
            );
            assert!(contrast(tide.text, tide.bg) >= 7.0);
            for color in [tide.good, tide.warn, tide.danger, tide.accent, tide.muted] {
                assert!(contrast(color, tide.bg) >= 4.5);
            }
        }
    }

    #[test]
    fn ascii_mode_has_no_non_ascii_glyphs() {
        let tide = Tide::from_palette(
            &Palette::catppuccin(),
            Options {
                ascii: true,
                ..Options::default()
            },
        );
        let glyphs = tide.glyphs;
        for glyph in [
            glyphs.working,
            glyphs.waiting,
            glyphs.approval,
            glyphs.blocked,
            glyphs.idle,
            glyphs.paused,
            glyphs.rail,
            glyphs.ellipsis,
            glyphs.open,
            glyphs.closed,
            glyphs.bar_full,
            glyphs.bar_empty,
        ] {
            assert!(glyph.is_ascii(), "{glyph}");
        }
    }
}
