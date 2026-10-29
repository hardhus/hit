//! A centered, bordered box that pops open.

use std::time::Duration;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Clear},
};

use crate::{Theme, anim, layout::centered};

const POP: Duration = Duration::from_millis(130);

/// Props for a popup.
pub struct Popup<'a> {
    pub title: &'a str,
    pub footer: Option<&'a str>,
    pub width: u16,
    pub height: u16,
    pub border: Color,
    pub theme: &'a Theme,
    /// When it opened; it grows from a thin bar to full height.
    pub opened: Option<anim::Moment>,
}

impl Popup<'_> {
    /// Draws the frame and returns the inner area.
    pub fn render(&self, frame: &mut Frame, area: Rect) -> Rect {
        let progress = self
            .opened
            .map_or(1.0, |at| anim::ease_out(at.progress(Duration::ZERO, POP)));
        let full = centered(area, self.width, self.height);
        let height =
            ((f32::from(full.height) * progress).round() as u16).clamp(3, full.height.max(3));
        let rect = Rect {
            y: full.y + (full.height.saturating_sub(height)) / 2,
            height: height.min(full.height),
            ..full
        };
        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(self.border))
            .style(self.theme.base())
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::new().fg(self.border).add_modifier(Modifier::BOLD),
            ));
        if let Some(footer) = self.footer {
            block = block.title_bottom(Span::styled(
                format!(" {footer} "),
                Style::new().fg(self.theme.muted),
            ));
        }
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        inner
    }

    pub fn is_animating(opened: Option<anim::Moment>) -> bool {
        opened.is_some_and(|at| at.within(POP))
    }
}
