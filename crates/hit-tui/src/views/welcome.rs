//! Temporary start screen. Replaced by the Action Required view in F5.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
};

use crate::{Action, Ctx, Event, View};

const LOGO: [&str; 6] = [
    "██╗  ██╗ ██╗ ████████╗",
    "██║  ██║ ██║ ╚══██╔══╝",
    "███████║ ██║    ██║   ",
    "██╔══██║ ██║    ██║   ",
    "██║  ██║ ██║    ██║   ",
    "╚═╝  ╚═╝ ╚═╝    ╚═╝   ",
];
const LOGO_WIDTH: u16 = 22;
/// Gradient cycles per second.
const SHIMMER_SPEED: f32 = 0.12;

const COMING: [(&str, &str); 4] = [
    (
        "Action Required",
        "what needs you, first thing in the morning",
    ),
    ("Inbox", "every notification, filtered your way"),
    ("Pull requests", "details, checks and diffs"),
    ("Issues", "find, read and triage"),
];

pub struct Welcome {
    warnings: Vec<String>,
}

impl Welcome {
    pub fn new(warnings: Vec<String>) -> Self {
        Self { warnings }
    }

    fn logo(&self, ctx: &Ctx) -> Vec<Line<'static>> {
        let phase = if ctx.animations {
            -ctx.time() * SHIMMER_SPEED
        } else {
            0.0
        };
        LOGO.iter()
            .enumerate()
            .map(|(row, text)| {
                // A slight diagonal tilt makes the shimmer feel like light.
                let row_phase = phase + row as f32 * 0.03;
                ctx.theme
                    .gradient_line(text, usize::from(LOGO_WIDTH) * 2, row_phase)
            })
            .collect()
    }

    fn coming_soon(&self, ctx: &Ctx) -> Vec<Line<'static>> {
        let t = &ctx.theme;
        let bullets = [t.accent, t.accent_alt, t.success, t.warning];
        COMING
            .iter()
            .zip(bullets)
            .map(|((title, desc), color)| {
                Line::from(vec![
                    Span::styled("  ◆ ", Style::new().fg(color)),
                    Span::styled(*title, Style::new().fg(t.fg).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("  {desc}"), Style::new().fg(t.muted)),
                ])
            })
            .collect()
    }

    fn render_warnings(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let lines: Vec<Line> = self
            .warnings
            .iter()
            .map(|w| {
                Line::from(vec![
                    Span::styled(
                        "! ",
                        Style::new().fg(t.warning).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(w.as_str(), Style::new().fg(t.fg)),
                ])
            })
            .collect();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.warning))
            .title(Span::styled(
                " config warnings ",
                Style::new().fg(t.warning).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(" hit config check ", Style::new().fg(t.muted)));
        frame.render_widget(
            Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
            area,
        );
    }

    fn hints(ctx: &Ctx) -> Line<'static> {
        let t = &ctx.theme;
        let key = |k: &'static str| {
            Span::styled(
                format!(" {k} "),
                Style::new()
                    .fg(t.selection)
                    .bg(t.accent_alt)
                    .add_modifier(Modifier::BOLD),
            )
        };
        let label = |l: &'static str| Span::styled(format!(" {l}   "), Style::new().fg(t.muted));
        Line::from(vec![key("q"), label("quit"), key("ctrl+c"), label("quit")])
    }
}

impl View for Welcome {
    fn handle_event(&mut self, event: &Event, _ctx: &Ctx) -> Option<Action> {
        #[cfg(debug_assertions)]
        debug_panic(event);
        let _ = event;
        None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let warnings_height = if self.warnings.is_empty() {
            0
        } else {
            u16::try_from(self.warnings.len())
                .unwrap_or(u16::MAX)
                .saturating_add(2)
                .min(8)
        };

        let [body, hints] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        let [logo, tagline, version, _, coming, _, warnings] = Layout::vertical([
            Constraint::Length(LOGO.len() as u16),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(COMING.len() as u16),
            Constraint::Length(1),
            Constraint::Length(warnings_height),
        ])
        .flex(Flex::Center)
        .areas(body);

        frame.render_widget(
            Paragraph::new(self.logo(ctx)).alignment(Alignment::Center),
            logo,
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "GitHub",
                    Style::new().fg(t.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" from your terminal — ", Style::new().fg(t.fg)),
                Span::styled(
                    "focus first",
                    Style::new().fg(t.accent_alt).add_modifier(Modifier::BOLD),
                ),
            ]))
            .alignment(Alignment::Center),
            tagline,
        );
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("v{}  ·  early development", env!("CARGO_PKG_VERSION")),
                Style::new().fg(t.muted),
            ))
            .alignment(Alignment::Center),
            version,
        );

        let [coming] = Layout::horizontal([Constraint::Length(68)])
            .flex(Flex::Center)
            .areas(coming);
        frame.render_widget(Paragraph::new(self.coming_soon(ctx)), coming);

        if warnings_height > 0 {
            let [warnings] = Layout::horizontal([Constraint::Max(80)])
                .flex(Flex::Center)
                .areas(warnings);
            self.render_warnings(frame, warnings, ctx);
        }

        frame.render_widget(
            Paragraph::new(Self::hints(ctx)).alignment(Alignment::Center),
            hints,
        );
    }

    fn is_animating(&self) -> bool {
        true
    }
}

/// Debug builds only, with `HIT_DEBUG_PANIC` set: `P` panics on the UI thread,
/// `B` in a background task, so shutdown after a panic can be verified by hand.
#[cfg(debug_assertions)]
#[allow(clippy::panic)]
fn debug_panic(event: &Event) {
    use crossterm::event::KeyCode;
    let Event::Key(key) = event else { return };
    if std::env::var_os("HIT_DEBUG_PANIC").is_none() {
        return;
    }
    match key.code {
        KeyCode::Char('P') => panic!("debug panic on the UI thread (HIT_DEBUG_PANIC)"),
        KeyCode::Char('B') => {
            tokio::spawn(async { panic!("debug panic in a background task (HIT_DEBUG_PANIC)") });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};
    use tokio::sync::mpsc;

    use super::*;
    use crate::Theme;

    fn render_to_string(view: &mut Welcome) -> String {
        let (tx, _rx) = mpsc::unbounded_channel();
        let ctx = Ctx::new(Theme::jinx(true, true), false, tx);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| view.render(frame, frame.area(), &ctx))
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shows_logo_tagline_and_hints() {
        let screen = render_to_string(&mut Welcome::new(Vec::new()));
        assert!(screen.contains("██╗  ██╗"));
        assert!(screen.contains("focus first"));
        assert!(screen.contains("quit"));
        assert!(!screen.contains("config warnings"));
    }

    #[test]
    fn shows_config_warnings() {
        let mut view = Welcome::new(vec!["unknown key ui.colour; ignored".into()]);
        let screen = render_to_string(&mut view);
        assert!(screen.contains("config warnings"));
        assert!(screen.contains("ui.colour"));
    }
}
