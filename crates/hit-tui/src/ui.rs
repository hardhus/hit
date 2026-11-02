//! Reusable, props-driven building blocks. Nothing here knows which screen
//! it is on: callers pass data, theme and timing in.

pub mod chips;
pub mod hints;
pub mod list;
pub mod picker;
pub mod popup;
pub mod tabs;
pub mod toast;

use ratatui::buffer::Buffer;
use ratatui::text::Line;

/// Rows `lines` take when wrapped to `width`. Word wrapping can only add
/// rows, so this may undercount slightly on very long words; that only
/// stops scrolling a little early, never past the end.
/// Blanks whatever could change how the terminal shows the rest: control
/// characters and bidirectional overrides, which can make a title read as
/// something it is not. Runs once per frame over everything drawn.
pub fn scrub(buffer: &mut Buffer) {
    let harmful =
        |c: char| c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    for cell in &mut buffer.content {
        if cell.symbol().chars().any(harmful) {
            cell.set_symbol(" ");
        }
    }
}

pub fn wrapped_height(lines: &[Line<'_>], width: u16) -> u16 {
    let width = usize::from(width.max(1));
    let rows: usize = lines.iter().map(|l| l.width().max(1).div_ceil(width)).sum();
    u16::try_from(rows).unwrap_or(u16::MAX)
}
