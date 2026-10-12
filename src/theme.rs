//! ArtCraft brand palette and terminal colour-depth detection.
//!
//! Token names and values follow getartcraft.com — the same tokens as the
//! official launcher's `ui-egui/src/theme.rs`. Terminals get the richest tier
//! they can show: truecolor RGB, then xterm-256, then ANSI 16, then monochrome
//! (`NO_COLOR` / `TERM=dumb`), where bold and reverse still carry the meaning.

use ratatui::style::{Color, Style};

/// How much colour the terminal can actually show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// `NO_COLOR` / `TERM=dumb`: emphasis only.
    Mono,
    /// Named ANSI colours.
    Ansi16,
    /// xterm-256 palette.
    Ansi256,
    /// 24-bit RGB.
    TrueColor,
}

/// The brand roles, by meaning rather than by hue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    /// Selection, running tasks, primary accents.
    Accent,
    /// Accent for text and icons.
    AccentInk,
    /// Up to date, success.
    Good,
    /// Updates available, caution.
    Warn,
    /// Failures.
    Bad,
    /// Secondary text.
    Muted,
    /// Tertiary text, hints.
    Faint,
    /// Body text.
    Ink,
}

/// One appearance plus the terminal's colour depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub depth: Depth,
    /// `ARTCRAFT_THEME=light` — darker ink for light terminal backgrounds.
    light: bool,
}

/// Truecolor pairs: (dark, light).
const TRUECOLOR: [(Tint, u32, u32); 8] = [
    (Tint::Accent, 0x2d81ff, 0x2d81ff),
    (Tint::AccentInk, 0x74aaff, 0x1659c4),
    (Tint::Good, 0x3fd6b8, 0x0a7563),
    (Tint::Warn, 0xffc14d, 0x8a5800),
    (Tint::Bad, 0xff6259, 0xb3261e),
    (Tint::Muted, 0x989794, 0x66666a),
    (Tint::Faint, 0x676663, 0x939397),
    (Tint::Ink, 0xf2f1ee, 0x101014),
];

/// xterm-256 indices: (dark, light).
const ANSI256: [(Tint, u8, u8); 8] = [
    (Tint::Accent, 33, 33),
    (Tint::AccentInk, 111, 26),
    (Tint::Good, 79, 30),
    (Tint::Warn, 214, 94),
    (Tint::Bad, 203, 124),
    (Tint::Muted, 245, 244),
    (Tint::Faint, 242, 246),
    (Tint::Ink, 255, 233),
];

impl Palette {
    /// Monochrome: bold/reverse only, never a colour.
    pub const fn mono() -> Self {
        Self {
            depth: Depth::Mono,
            light: false,
        }
    }

    /// Pick the richest palette this terminal can show.
    pub fn detect() -> Self {
        if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Self::mono();
        }
        let light = std::env::var("ARTCRAFT_THEME").is_ok_and(|t| t.eq_ignore_ascii_case("light"));
        let term = std::env::var("TERM").unwrap_or_default();
        if term == "dumb" {
            return Self::mono();
        }
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        let truecolor = colorterm.contains("truecolor")
            || colorterm.contains("24bit")
            || term.contains("truecolor")
            || term.contains("direct");
        let depth = if truecolor {
            Depth::TrueColor
        } else if term.contains("256color") {
            Depth::Ansi256
        } else {
            // A named-colour terminal; 16 colours are always there.
            Depth::Ansi16
        };
        Self { depth, light }
    }

    /// An explicit tier, for tests and future `--theme` plumbing.
    #[allow(dead_code)]
    pub const fn of(depth: Depth) -> Self {
        Self {
            depth,
            light: false,
        }
    }

    /// Lighter ink for light terminal backgrounds (`ARTCRAFT_THEME=light`).
    #[allow(dead_code)]
    pub const fn light(mut self) -> Self {
        self.light = true;
        self
    }

    /// The colour for a role, or `None` in monochrome.
    pub fn color(self, tint: Tint) -> Option<Color> {
        match self.depth {
            Depth::Mono => None,
            Depth::TrueColor => TRUECOLOR
                .iter()
                .find(|(t, _, _)| *t == tint)
                .map(|(_, dark, light)| rgb(if self.light { *light } else { *dark })),
            Depth::Ansi256 => ANSI256
                .iter()
                .find(|(t, _, _)| *t == tint)
                .map(|(_, dark, light)| Color::Indexed(if self.light { *light } else { *dark })),
            Depth::Ansi16 => Some(ansi16(tint, self.light)),
        }
    }
}

fn rgb(v: u32) -> Color {
    let [_, r, g, b] = v.to_be_bytes();
    Color::Rgb(r, g, b)
}

fn ansi16(tint: Tint, light: bool) -> Color {
    match (tint, light) {
        (Tint::Accent, _) | (Tint::AccentInk, _) => Color::Blue,
        (Tint::Good, _) => Color::Green,
        (Tint::Warn, false) => Color::Yellow,
        // ANSI brown: readable on a light background where yellow is not
        (Tint::Warn, true) => Color::Indexed(3),
        (Tint::Bad, _) => Color::Red,
        (Tint::Muted, _) | (Tint::Faint, _) => Color::DarkGray,
        (Tint::Ink, false) => Color::White,
        (Tint::Ink, true) => Color::Black,
    }
}

/// Style builder for one frame: colours by role, monochrome-safe.
#[derive(Clone, Copy)]
pub struct Sty {
    pub palette: Palette,
}

impl Sty {
    pub fn new(palette: Palette) -> Self {
        Self { palette }
    }

    pub fn fg(self, tint: Tint) -> Style {
        match self.palette.color(tint) {
            Some(c) => Style::default().fg(c),
            None => Style::default(),
        }
    }

    pub fn bold(self) -> Style {
        Style::default().bold()
    }

    /// The focused row: an accent bar where there is colour, reverse video where not.
    pub fn hl(self) -> Style {
        match self.palette.depth {
            Depth::Mono => Style::default().reversed().bold(),
            _ => Style::default()
                .bg(self.palette.color(Tint::Accent).unwrap_or(Color::Blue))
                .fg(on_accent(self.palette.depth))
                .bold(),
        }
    }

    /// Text style for anything sitting on the accent bar: the dark brand ink.
    pub fn on_bar(self) -> Style {
        match self.palette.depth {
            Depth::Mono => Style::default(),
            _ => Style::default().fg(on_accent(self.palette.depth)),
        }
    }
}

/// The dark ink used on top of the accent bar (`#101014` in the brand tokens).
fn on_accent(depth: Depth) -> Color {
    match depth {
        Depth::Mono => Color::Reset,
        Depth::TrueColor => rgb(0x101014),
        Depth::Ansi256 => Color::Indexed(233),
        Depth::Ansi16 => Color::Black,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_never_yields_a_colour() {
        let p = Palette::mono();
        for t in [
            Tint::Accent,
            Tint::AccentInk,
            Tint::Good,
            Tint::Warn,
            Tint::Bad,
            Tint::Muted,
            Tint::Faint,
            Tint::Ink,
        ] {
            assert_eq!(p.color(t), None, "{t:?} must be colourless in mono");
        }
    }

    #[test]
    fn truecolor_uses_the_brand_rgb() {
        let p = Palette::of(Depth::TrueColor);
        assert_eq!(p.color(Tint::Accent), Some(Color::Rgb(0x2d, 0x81, 0xff)));
        assert_eq!(p.color(Tint::Good), Some(Color::Rgb(0x3f, 0xd6, 0xb8)));
        assert_eq!(p.color(Tint::Warn), Some(Color::Rgb(0xff, 0xc1, 0x4d)));
        assert_eq!(p.color(Tint::Bad), Some(Color::Rgb(0xff, 0x62, 0x59)));
    }

    #[test]
    fn light_theme_flips_the_ink() {
        let dark = Palette::of(Depth::TrueColor);
        let light = Palette::of(Depth::TrueColor).light();
        assert_ne!(dark.color(Tint::Ink), light.color(Tint::Ink));
        assert_eq!(light.color(Tint::Accent), dark.color(Tint::Accent));
    }

    #[test]
    fn lower_tiers_use_indexed_then_named() {
        assert_eq!(
            Palette::of(Depth::Ansi256).color(Tint::Accent),
            Some(Color::Indexed(33))
        );
        assert_eq!(
            Palette::of(Depth::Ansi16).color(Tint::Good),
            Some(Color::Green)
        );
    }

    #[test]
    fn highlight_never_depends_on_colour_alone() {
        // mono must still mark focus, via reverse video
        assert_eq!(
            Sty::new(Palette::mono()).hl(),
            Style::default().reversed().bold()
        );
    }

    #[test]
    fn highlight_puts_dark_ink_on_the_accent_bar() {
        assert_eq!(
            Sty::new(Palette::of(Depth::TrueColor)).hl(),
            Style::default()
                .bg(Color::Rgb(0x2d, 0x81, 0xff))
                .fg(Color::Rgb(0x10, 0x10, 0x14))
                .bold()
        );
    }
}
