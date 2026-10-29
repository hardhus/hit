//! Reusable, props-driven building blocks. Nothing here knows which screen
//! it is on: callers pass data, theme and timing in.

pub mod chips;
pub mod hints;
pub mod list;
pub mod popup;
pub mod tabs;
pub mod toast;

use ratatui::text::Line;

/// Rows `lines` take when wrapped to `width`. Word wrapping can only add
/// rows, so this may undercount slightly on very long words; that only
/// stops scrolling a little early, never past the end.
pub fn wrapped_height(lines: &[Line<'_>], width: u16) -> u16 {
    let width = usize::from(width.max(1));
    let rows: usize = lines.iter().map(|l| l.width().max(1).div_ceil(width)).sum();
    u16::try_from(rows).unwrap_or(u16::MAX)
}
