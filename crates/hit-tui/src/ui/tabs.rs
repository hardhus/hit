//! The tab bar: logo, numbered tabs with counts, and an underline that
//! glides to the active tab.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::Theme;

pub struct TabSpec {
    pub title: &'static str,
    pub count: Option<usize>,
}

/// Props for the tab bar.
pub struct TabBar<'a> {
    pub tabs: &'a [TabSpec],
    pub active: usize,
    pub theme: &'a Theme,
    /// Current underline position (start column, width), possibly mid-glide.
    pub underline: (f32, f32),
    /// Show the gradient ` hit ` logo before the tabs.
    pub logo: bool,
    /// Phase for the logo and underline gradients.
    pub phase: f32,
    /// Shown at the right end (accounts, mode badges).
    pub right: Line<'static>,
}

impl TabBar<'_> {
    /// Draws the bar and returns each tab's (start column, width) so the
    /// caller can aim the underline.
    pub fn render(&self, frame: &mut Frame, area: Rect) -> Vec<(u16, u16)> {
        let t = self.theme;
        let mut spans = vec![Span::raw(" ")];
        if self.logo {
            spans.extend(t.gradient_line(" hit ", 12, self.phase).spans);
            spans.push(Span::raw("  "));
        }
        let mut column = spans.iter().map(Span::width).sum::<usize>() as u16;

        let mut positions = Vec::with_capacity(self.tabs.len());
        for (index, tab) in self.tabs.iter().enumerate() {
            let active = index == self.active;
            let mut tab_spans = vec![
                Span::styled(format!(" {} ", index + 1), Style::new().fg(t.muted)),
                Span::styled(
                    tab.title,
                    if active {
                        Style::new().fg(t.fg).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(t.muted)
                    },
                ),
            ];
            if let Some(count) = tab.count.filter(|c| *c > 0) {
                tab_spans.push(Span::styled(
                    format!(" {count}"),
                    Style::new()
                        .fg(if active { t.accent_alt } else { t.muted })
                        .add_modifier(Modifier::BOLD),
                ));
            }
            tab_spans.push(Span::raw(" "));
            let width = tab_spans.iter().map(Span::width).sum::<usize>() as u16;
            positions.push((column, width));
            column += width + 1;
            spans.extend(tab_spans);
            spans.push(Span::raw(" "));
        }

        let [top, bottom] = [
            Rect { height: 1, ..area },
            Rect {
                y: area.y + 1,
                height: area.height.saturating_sub(1).min(1),
                ..area
            },
        ];
        frame.render_widget(Paragraph::new(Line::from(spans)), top);
        frame.render_widget(
            Paragraph::new(self.right.clone()).alignment(ratatui::layout::Alignment::Right),
            top,
        );

        // Baseline with the gliding underline on top.
        let buf = frame.buffer_mut();
        let (start, width) = self.underline;
        for x in bottom.x..bottom.right() {
            let column = f32::from(x - bottom.x);
            if let Some(cell) = buf.cell_mut((x, bottom.y)) {
                if column >= start.round() && column < (start + width).round() {
                    let along = (column - start) / width.max(1.0);
                    cell.set_symbol("━")
                        .set_fg(t.gradient_at(along * 0.5 + self.phase));
                } else {
                    cell.set_symbol("─").set_fg(t.border);
                }
            }
        }
        positions
    }
}
