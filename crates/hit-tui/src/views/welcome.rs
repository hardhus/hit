//! Temporary start screen. Replaced by the Action Required view in F5.

use std::sync::Arc;

use hit_core::{
    Forge, ForgeError,
    focus::{FocusItem, FocusReason},
    query::{NotificationQuery, PageRequest, Scope},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
};

use crate::{Action, Ctx, DataEvent, Event, View};

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
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// Upper bound on notification pages fetched just to count unread ones.
const MAX_UNREAD_PAGES: usize = 10;

const COMING: [(&str, &str); 4] = [
    (
        "Action Required",
        "what needs you, first thing in the morning",
    ),
    ("Inbox", "every notification, filtered your way"),
    ("Pull requests", "details, checks and diffs"),
    ("Issues", "find, read and triage"),
];

/// A taste of the user's day, fetched in the background.
enum Summary {
    NoAccount,
    Loading {
        focus: Option<Arc<Vec<FocusItem>>>,
        unread: Option<usize>,
    },
    Ready {
        focus: Arc<Vec<FocusItem>>,
        unread: usize,
    },
    Failed(ForgeError),
}

pub struct Welcome {
    warnings: Vec<String>,
    summary: Summary,
}

impl Welcome {
    pub fn new(warnings: Vec<String>) -> Self {
        Self {
            warnings,
            summary: Summary::NoAccount,
        }
    }

    fn on_data(&mut self, event: &DataEvent) {
        let Summary::Loading { focus, unread } = &mut self.summary else {
            return;
        };
        match event {
            DataEvent::Focus(Ok(items)) => *focus = Some(Arc::clone(items)),
            DataEvent::UnreadCount(Ok(count)) => *unread = Some(*count),
            DataEvent::Focus(Err(e)) | DataEvent::UnreadCount(Err(e)) => {
                self.summary = Summary::Failed(e.clone());
                return;
            }
        }
        if let (Some(focus), Some(unread)) = (focus.as_ref(), *unread) {
            self.summary = Summary::Ready {
                focus: Arc::clone(focus),
                unread,
            };
        }
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

    fn summary_line(&self, ctx: &Ctx) -> Line<'static> {
        let t = &ctx.theme;
        let bold = |color: Color| Style::new().fg(color).add_modifier(Modifier::BOLD);
        match &self.summary {
            Summary::NoAccount => Line::from(vec![
                Span::styled("not signed in yet, try ", Style::new().fg(t.muted)),
                Span::styled("hit --demo", bold(t.accent_alt)),
            ]),
            Summary::Loading { .. } => {
                let frame = if ctx.animations {
                    (ctx.time() * 12.0) as usize % SPINNER.len()
                } else {
                    0
                };
                Line::from(vec![
                    Span::styled(SPINNER.get(frame).copied().unwrap_or("·"), bold(t.accent)),
                    Span::styled(" loading your day…", Style::new().fg(t.muted)),
                ])
            }
            Summary::Failed(e) => Line::from(vec![
                Span::styled("! ", bold(t.danger)),
                Span::styled(
                    format!("couldn't load your day: {e}"),
                    Style::new().fg(t.danger),
                ),
            ]),
            Summary::Ready { focus, unread } => {
                let count = |reason| focus.iter().filter(|i| i.has(reason)).count();
                let parts = [
                    (count(FocusReason::ReviewRequested), "to review", t.accent),
                    (count(FocusReason::ChecksFailing), "CI failing", t.danger),
                    (
                        count(FocusReason::ChangesRequested),
                        "changes requested",
                        t.warning,
                    ),
                    (
                        count(FocusReason::ReadyToMerge),
                        "ready to merge",
                        t.success,
                    ),
                    (count(FocusReason::Mentioned), "mentions", t.info),
                    (count(FocusReason::Assigned), "assigned", t.highlight),
                    (*unread, "unread", t.accent_alt),
                ];
                let mut spans = Vec::new();
                for (n, label, color) in parts.into_iter().filter(|(n, ..)| *n > 0) {
                    if !spans.is_empty() {
                        spans.push(Span::styled(" · ", Style::new().fg(t.border)));
                    }
                    spans.push(Span::styled(n.to_string(), bold(color)));
                    spans.push(Span::styled(format!(" {label}"), Style::new().fg(t.fg)));
                }
                if spans.is_empty() {
                    spans.push(Span::styled(
                        "all clear, nothing needs you ✨",
                        bold(t.success),
                    ));
                }
                Line::from(spans)
            }
        }
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

    fn render_demo_badge(frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let badge = Span::styled(
            " DEMO ",
            Style::new()
                .fg(t.selection)
                .bg(t.accent_alt)
                .add_modifier(Modifier::BOLD),
        );
        let [top, _] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![badge, Span::raw(" ")])).alignment(Alignment::Right),
            top,
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

/// Fetches the Action Required items and the unread count, reporting each
/// through the action channel.
fn spawn_summary(forge: Arc<dyn Forge>, ctx: &Ctx) {
    let tx = ctx.sender();
    tokio::spawn(async move {
        let focus = match forge.focus() {
            Some(focus) => focus.items(&Scope::All).await.map(Arc::new),
            None => Err(ForgeError::Unsupported("Action Required")),
        };
        let _ = tx.send(Action::Data(DataEvent::Focus(focus)));

        let unread = match forge.notifications() {
            Some(notifications) => count_unread(notifications).await,
            None => Ok(0),
        };
        let _ = tx.send(Action::Data(DataEvent::UnreadCount(unread)));
    });
}

async fn count_unread(notifications: &dyn hit_core::Notifications) -> Result<usize, ForgeError> {
    let query = NotificationQuery {
        unread_only: true,
        ..NotificationQuery::default()
    };
    let mut page = PageRequest::first(100);
    let mut total = 0;
    for _ in 0..MAX_UNREAD_PAGES {
        let result = notifications.list(&query, &page).await?;
        total += result.items.len();
        match result.next {
            Some(cursor) => page = PageRequest::after(cursor, 100),
            None => break,
        }
    }
    Ok(total)
}

impl View for Welcome {
    fn mount(&mut self, ctx: &Ctx) {
        // Multiple accounts are summarized once there is a real accounts model (F2).
        if let Some(forge) = ctx.forges.iter().next() {
            self.summary = Summary::Loading {
                focus: None,
                unread: None,
            };
            spawn_summary(Arc::clone(forge), ctx);
        }
    }

    fn handle_event(&mut self, event: &Event, _ctx: &Ctx) -> Option<Action> {
        #[cfg(debug_assertions)]
        debug_panic(event);
        let _ = event;
        None
    }

    fn update(&mut self, action: &Action, _ctx: &Ctx) -> Option<Action> {
        if let Action::Data(event) = action {
            self.on_data(event);
        }
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

        if ctx.forges.is_demo() {
            Self::render_demo_badge(frame, area, ctx);
        }

        let [body, hints] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        let [logo, tagline, version, _, summary, _, coming, _, warnings] = Layout::vertical([
            Constraint::Length(LOGO.len() as u16),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            // The summary wraps onto a second line in narrow terminals.
            Constraint::Length(2),
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
        frame.render_widget(
            Paragraph::new(self.summary_line(ctx))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            summary,
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
    use hit_core::Forges;
    use hit_forge_mock::{MockForge, MockOptions};
    use ratatui::{Terminal, backend::TestBackend};
    use tokio::sync::mpsc;

    use super::*;
    use crate::Theme;

    fn ctx(forges: Forges) -> (Ctx, mpsc::UnboundedReceiver<Action>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Ctx::new(Theme::jinx(true, true), false, forges, tx), rx)
    }

    fn demo_forges() -> Forges {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        forges
    }

    fn render_to_string(view: &mut Welcome, ctx: &Ctx) -> String {
        let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
        terminal
            .draw(|frame| view.render(frame, frame.area(), ctx))
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
    fn shows_logo_tagline_hints_and_sign_in_hint() {
        let (ctx, _rx) = ctx(Forges::new());
        let mut view = Welcome::new(Vec::new());
        view.mount(&ctx);
        let screen = render_to_string(&mut view, &ctx);
        assert!(screen.contains("██╗  ██╗"));
        assert!(screen.contains("focus first"));
        assert!(screen.contains("quit"));
        assert!(screen.contains("hit --demo"));
        assert!(!screen.contains("DEMO"));
        assert!(!screen.contains("config warnings"));
    }

    #[test]
    fn shows_config_warnings() {
        let (ctx, _rx) = ctx(Forges::new());
        let mut view = Welcome::new(vec!["unknown key ui.colour; ignored".into()]);
        let screen = render_to_string(&mut view, &ctx);
        assert!(screen.contains("config warnings"));
        assert!(screen.contains("ui.colour"));
    }

    #[tokio::test]
    async fn demo_loads_summary_in_background() {
        let (ctx, mut rx) = ctx(demo_forges());
        let mut view = Welcome::new(Vec::new());
        view.mount(&ctx);

        let screen = render_to_string(&mut view, &ctx);
        assert!(screen.contains("DEMO"));
        assert!(screen.contains("loading your day"));

        for _ in 0..2 {
            let action = rx.recv().await.unwrap();
            view.update(&action, &ctx);
        }
        let screen = render_to_string(&mut view, &ctx);
        assert!(screen.contains("to review"), "{screen}");
        assert!(screen.contains("CI failing"));
        assert!(screen.contains("unread"));
        assert!(!screen.contains("loading your day"));
    }

    #[test]
    fn errors_are_shown() {
        let (ctx, _rx) = ctx(Forges::new());
        let mut view = Welcome::new(Vec::new());
        view.summary = Summary::Loading {
            focus: None,
            unread: None,
        };
        view.on_data(&DataEvent::Focus(Err(ForgeError::Network(
            "offline".into(),
        ))));
        let screen = render_to_string(&mut view, &ctx);
        assert!(screen.contains("couldn't load your day"));
        assert!(screen.contains("offline"));
    }
}
