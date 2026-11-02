//! Short messages that slide in at the top right and fade away.

use std::time::Duration;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Wrap},
};

use crate::{
    Theme,
    action::{Toast, ToastKind},
    anim,
};

const SLIDE: Duration = Duration::from_millis(170);
const WIDTH: u16 = 46;

#[derive(Debug, Default)]
pub struct Toasts {
    items: Vec<(Toast, anim::Moment)>,
}

impl Toasts {
    pub fn push(&mut self, toast: Toast) {
        // The same message again just restarts its timer.
        self.items.retain(|(t, _)| t != &toast);
        self.items.push((toast, anim::Moment::now()));
        if self.items.len() > 4 {
            self.items.remove(0);
        }
    }

    fn lifetime(kind: ToastKind) -> Duration {
        match kind {
            ToastKind::Info | ToastKind::Success => Duration::from_secs(4),
            ToastKind::Warning | ToastKind::Error => Duration::from_secs(8),
        }
    }

    pub fn prune(&mut self) {
        self.items
            .retain(|(toast, at)| at.within(Self::lifetime(toast.kind)));
    }

    pub fn is_active(&self) -> bool {
        !self.items.is_empty()
    }

    /// Draws the toasts stacked below `area.y`, right-aligned.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut y = area.y;
        for (toast, at) in &self.items {
            let (icon, color) = match toast.kind {
                ToastKind::Info => ("●", theme.info),
                ToastKind::Success => ("✓", theme.success),
                ToastKind::Warning => ("!", theme.warning),
                ToastKind::Error => ("✗", theme.danger),
            };
            let width = WIDTH.min(area.width);
            let inner_width = usize::from(width.saturating_sub(4)).max(1);
            let lines = (toast.message.chars().count() + 2).div_ceil(inner_width) as u16;
            let height = (lines + 2).min(6);
            if y + height > area.bottom() {
                break;
            }
            let progress = anim::ease_out(at.progress(Duration::ZERO, SLIDE));
            let offset = ((1.0 - progress) * f32::from(width)).round() as u16;
            let rect = Rect {
                x: area.right().saturating_sub(width + 1) + offset,
                y,
                width: width.saturating_sub(offset),
                height,
            };
            frame.render_widget(Clear, rect);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!("{icon} "),
                        Style::new().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(toast.message.clone(), Style::new().fg(theme.fg)),
                ]))
                .wrap(Wrap { trim: true })
                .block(
                    Block::bordered()
                        .border_type(BorderType::Rounded)
                        .border_style(Style::new().fg(color))
                        .style(theme.base()),
                ),
                rect,
            );
            y += height;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_restart_and_the_stack_is_bounded() {
        let mut toasts = Toasts::default();
        toasts.push(Toast::info("a"));
        toasts.push(Toast::info("a"));
        assert_eq!(toasts.items.len(), 1);
        for n in 0..10 {
            toasts.push(Toast::info(format!("{n}")));
        }
        assert_eq!(toasts.items.len(), 4);
    }
}
