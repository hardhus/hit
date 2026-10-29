//! Tabs that arrive in later phases: a friendly empty state.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use super::{Tab, TabArea};
use crate::{Ctx, ui::chips::bold};

pub struct Placeholder {
    title: &'static str,
    icon: &'static str,
    pitch: &'static str,
    phase: &'static str,
}

impl Placeholder {
    pub fn new(
        title: &'static str,
        icon: &'static str,
        pitch: &'static str,
        phase: &'static str,
    ) -> Self {
        Self {
            title,
            icon,
            pitch,
            phase,
        }
    }
}

impl Tab for Placeholder {
    fn title(&self) -> &'static str {
        self.title
    }

    fn render(&mut self, frame: &mut Frame, area: TabArea, ctx: &Ctx) {
        let t = &ctx.theme;
        let full = area.preview.map_or(area.list, |p| area.list.union(p));
        let [middle] = Layout::vertical([Constraint::Length(5)])
            .flex(Flex::Center)
            .areas(full);
        let phase = ctx.time() * 0.15;
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(self.icon, bold(t.gradient_at(phase)))),
                Line::raw(""),
                Line::from(Span::styled(self.title, bold(t.fg))),
                Line::from(Span::styled(self.pitch, Style::new().fg(t.muted))),
                Line::from(Span::styled(
                    format!("coming in {}", self.phase),
                    Style::new().fg(t.accent_alt),
                )),
            ])
            .alignment(Alignment::Center),
            middle,
        );
    }
}
