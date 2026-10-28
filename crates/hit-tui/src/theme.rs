//! Colors. Views use semantic tokens (`accent`, `danger`, …) and never raw
//! colors, so themes can change without touching views.

use hit_config::{ThemeName, UiConfig};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

type Rgb = (u8, u8, u8);

/// Jinx: electric blue, neon pink and purple with bright cyan, toxic green and
/// sunny yellow.
mod jinx {
    use super::Rgb;

    pub const BLUE: Rgb = (0x00, 0xB4, 0xFF);
    pub const PINK: Rgb = (0xFF, 0x2E, 0x97);
    pub const PURPLE: Rgb = (0x9D, 0x4E, 0xDD);
    pub const CYAN: Rgb = (0x2D, 0xE2, 0xE6);
    pub const GREEN: Rgb = (0x7C, 0xFF, 0x4F);
    pub const YELLOW: Rgb = (0xFF, 0xE1, 0x4D);
    pub const RED: Rgb = (0xFF, 0x4D, 0x6D);
    pub const FG: Rgb = (0xE6, 0xE1, 0xFF);
    pub const MUTED: Rgb = (0x8A, 0x7F, 0xB0);
    pub const BORDER: Rgb = (0x5A, 0x3F, 0x8F);
    pub const SELECTION: Rgb = (0x3A, 0x1F, 0x5C);
    pub const BG: Rgb = (0x14, 0x0F, 0x24);
    pub const GRADIENT: [Rgb; 5] = [BLUE, CYAN, PURPLE, PINK, BLUE];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_alt: Color,
    pub highlight: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub info: Color,
    pub border: Color,
    pub selection: Color,
    /// Stops for gradients; the last stop should equal the first so animated
    /// gradients loop seamlessly.
    gradient: Vec<Rgb>,
    truecolor: bool,
}

impl Theme {
    pub fn from_config(ui: &UiConfig) -> Self {
        match ui.theme {
            ThemeName::Jinx => Self::jinx(ui.transparent, supports_truecolor()),
        }
    }

    pub fn jinx(transparent: bool, truecolor: bool) -> Self {
        use jinx::*;
        if truecolor {
            Self {
                bg: if transparent { Color::Reset } else { rgb(BG) },
                fg: rgb(FG),
                muted: rgb(MUTED),
                accent: rgb(BLUE),
                accent_alt: rgb(PINK),
                highlight: rgb(PURPLE),
                success: rgb(GREEN),
                warning: rgb(YELLOW),
                danger: rgb(RED),
                info: rgb(CYAN),
                border: rgb(BORDER),
                selection: rgb(SELECTION),
                gradient: GRADIENT.to_vec(),
                truecolor,
            }
        } else {
            Self {
                bg: if transparent {
                    Color::Reset
                } else {
                    Color::Black
                },
                fg: Color::White,
                muted: Color::DarkGray,
                accent: Color::LightBlue,
                accent_alt: Color::LightMagenta,
                highlight: Color::Magenta,
                success: Color::LightGreen,
                warning: Color::LightYellow,
                danger: Color::LightRed,
                info: Color::LightCyan,
                border: Color::Magenta,
                selection: Color::DarkGray,
                gradient: GRADIENT.to_vec(),
                truecolor,
            }
        }
    }

    pub fn base(&self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }

    /// Color at position `t` (wraps around 0..1) along the gradient.
    pub fn gradient_at(&self, t: f32) -> Color {
        let stops = &self.gradient;
        let segments = stops.len().saturating_sub(1);
        if segments == 0 {
            return self.accent;
        }
        let t = t.rem_euclid(1.0) * segments as f32;
        let index = (t.floor() as usize).min(segments - 1);
        let (Some(&from), Some(&to)) = (stops.get(index), stops.get(index + 1)) else {
            return self.accent;
        };
        if self.truecolor {
            rgb(lerp(from, to, t - index as f32))
        } else {
            // Without truecolor, step through the nearest named colors.
            [
                Color::LightBlue,
                Color::LightCyan,
                Color::Magenta,
                Color::LightMagenta,
            ]
            .get(index % 4)
            .copied()
            .unwrap_or(self.accent)
        }
    }

    /// Paints `text` with the gradient spread across `width` columns, shifted by
    /// `phase` (0..1) so it can be animated.
    pub fn gradient_line<'a>(&self, text: &'a str, width: usize, phase: f32) -> Line<'a> {
        let width = width.max(1) as f32;
        let spans: Vec<Span<'a>> = text
            .char_indices()
            .enumerate()
            .map(|(column, (byte, ch))| {
                let color = self.gradient_at(column as f32 / width + phase);
                let slice = text.get(byte..byte + ch.len_utf8()).unwrap_or_default();
                Span::styled(slice, Style::new().fg(color).add_modifier(Modifier::BOLD))
            })
            .collect();
        Line::from(spans)
    }
}

fn rgb((r, g, b): Rgb) -> Color {
    Color::Rgb(r, g, b)
}

fn lerp(from: Rgb, to: Rgb, t: f32) -> Rgb {
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    (mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2))
}

/// Best-effort truecolor detection.
pub fn supports_truecolor() -> bool {
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    colorterm.eq_ignore_ascii_case("truecolor")
        || colorterm.eq_ignore_ascii_case("24bit")
        // Windows Terminal and conhost (Windows 10+) support truecolor without
        // advertising it.
        || cfg!(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_endpoints_and_wrap() {
        let theme = Theme::jinx(true, true);
        assert_eq!(theme.gradient_at(0.0), rgb(jinx::BLUE));
        assert_eq!(theme.gradient_at(1.0), theme.gradient_at(0.0));
        assert_eq!(theme.gradient_at(-0.25), theme.gradient_at(0.75));
    }

    #[test]
    fn transparent_uses_terminal_background() {
        assert_eq!(Theme::jinx(true, true).bg, Color::Reset);
        assert_ne!(Theme::jinx(false, true).bg, Color::Reset);
    }

    #[test]
    fn gradient_line_keeps_text() {
        let theme = Theme::jinx(true, true);
        let line = theme.gradient_line("héllo", 5, 0.3);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "héllo");
    }
}
