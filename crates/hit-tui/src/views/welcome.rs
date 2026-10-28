//! Temporary start screen. Replaced by the Action Required view in F5.

use std::{sync::Arc, time::Duration};

use crossterm::event::KeyCode;
use hit_core::{
    ForgeError,
    focus::{FocusItem, FocusReason},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use crate::{
    Action, Ctx, DataEvent, Event, View,
    sync::{self, Loaded},
};

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

const COMING: [(&str, &str); 4] = [
    (
        "Action Required",
        "what needs you, first thing in the morning",
    ),
    ("Inbox", "every notification, filtered your way"),
    ("Pull requests", "details, checks and diffs"),
    ("Issues", "find, read and triage"),
];

/// The user's day: last known data, how old it is, and whether a refresh
/// is running or failed.
#[derive(Default)]
struct Day {
    focus: Option<Loaded<Arc<Vec<FocusItem>>>>,
    unread: Option<Loaded<usize>>,
    error: Option<ForgeError>,
    refreshing: bool,
}

enum Summary {
    NoAccount,
    /// Signed in, but the forge cannot report Action Required.
    SignedIn(String),
    Day(Day),
}

pub struct Welcome {
    warnings: Vec<String>,
    summary: Summary,
    tasks: Vec<JoinHandle<()>>,
}

impl Welcome {
    pub fn new(warnings: Vec<String>) -> Self {
        Self {
            warnings,
            summary: Summary::NoAccount,
            tasks: Vec::new(),
        }
    }

    fn stop_tasks(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    fn on_data(&mut self, event: &DataEvent) {
        let Summary::Day(day) = &mut self.summary else {
            return;
        };
        match event {
            DataEvent::Refreshing => day.refreshing = true,
            DataEvent::Focus(Ok(loaded)) => {
                if loaded.fresh {
                    day.error = None;
                    day.refreshing = false;
                }
                day.focus = Some(loaded.clone());
            }
            DataEvent::UnreadCount(Ok(loaded)) => day.unread = Some(loaded.clone()),
            DataEvent::Focus(Err(e)) | DataEvent::UnreadCount(Err(e)) => {
                day.error = Some(e.clone());
                day.refreshing = false;
            }
            DataEvent::Login(_) => {}
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

    fn spinner(ctx: &Ctx) -> &'static str {
        let frame = if ctx.animations {
            (ctx.time() * 12.0) as usize % SPINNER.len()
        } else {
            0
        };
        SPINNER.get(frame).copied().unwrap_or("·")
    }

    fn summary_line(&self, ctx: &Ctx) -> Line<'static> {
        let t = &ctx.theme;
        let bold = |color: Color| Style::new().fg(color).add_modifier(Modifier::BOLD);
        match &self.summary {
            Summary::NoAccount if ctx.login.is_some() => Line::from(vec![
                Span::styled("not signed in yet: press ", Style::new().fg(t.muted)),
                Span::styled("l", bold(t.accent)),
                Span::styled(" to sign in, or try ", Style::new().fg(t.muted)),
                Span::styled("hit --demo", bold(t.accent_alt)),
            ]),
            Summary::NoAccount => Line::from(vec![
                Span::styled("not signed in yet: run ", Style::new().fg(t.muted)),
                Span::styled("hit auth login", bold(t.accent)),
                Span::styled(", or try ", Style::new().fg(t.muted)),
                Span::styled("hit --demo", bold(t.accent_alt)),
            ]),
            Summary::SignedIn(account) => Line::from(vec![
                Span::styled("✓ ", bold(t.success)),
                Span::styled("signed in as ", Style::new().fg(t.fg)),
                Span::styled(account.clone(), bold(t.accent_alt)),
            ]),
            Summary::Day(day) => match (&day.focus, &day.error) {
                (Some(focus), _) => {
                    let count = |reason| focus.value.iter().filter(|i| i.has(reason)).count();
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
                        (
                            day.unread.as_ref().map_or(0, |u| u.value),
                            "unread",
                            t.accent_alt,
                        ),
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
                (None, Some(e)) => Line::from(vec![
                    Span::styled("! ", bold(t.danger)),
                    Span::styled(
                        format!("couldn't load your day: {e}"),
                        Style::new().fg(t.danger),
                    ),
                ]),
                (None, None) => Line::from(vec![
                    Span::styled(Self::spinner(ctx), bold(t.accent)),
                    Span::styled(" loading your day…", Style::new().fg(t.muted)),
                ]),
            },
        }
    }

    /// How fresh the summary is: refreshing, offline with old data, or when
    /// it was last updated. Never animates the text itself.
    fn status_line(&self, ctx: &Ctx) -> Line<'static> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let Summary::Day(Day {
            focus: Some(focus),
            error,
            refreshing,
            ..
        }) = &self.summary
        else {
            return Line::raw("");
        };
        let age = sync::ago(focus.fetched_at);
        if *refreshing {
            Line::from(vec![
                Span::styled(Self::spinner(ctx), Style::new().fg(t.accent)),
                Span::styled(format!(" refreshing · updated {age}"), muted),
            ])
        } else if let Some(e) = error {
            Line::from(vec![
                Span::styled(
                    "! ",
                    Style::new().fg(t.warning).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("couldn't refresh ({e}) · showing data from {age}"),
                    Style::new().fg(t.warning),
                ),
            ])
        } else if focus.fresh {
            Line::styled(format!("updated {age}"), muted)
        } else {
            Line::styled(format!("cached · {age}"), muted)
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
                " warnings ",
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

    fn can_sign_in(&self, ctx: &Ctx) -> bool {
        matches!(self.summary, Summary::NoAccount) && ctx.login.is_some()
    }

    fn hints(&self, ctx: &Ctx) -> Line<'static> {
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
        let mut spans = Vec::new();
        if self.can_sign_in(ctx) {
            spans.extend([key("l"), label("sign in")]);
        }
        if matches!(self.summary, Summary::Day(_)) {
            spans.extend([key("r"), label("refresh")]);
        }
        spans.extend([key("q"), label("quit"), key("ctrl+c"), label("quit")]);
        Line::from(spans)
    }
}

impl Drop for Welcome {
    fn drop(&mut self) {
        self.stop_tasks();
    }
}

impl View for Welcome {
    fn mount(&mut self, ctx: &Ctx) {
        self.stop_tasks();
        // Summarizing several accounts together comes with the Action
        // Required screen (F5); for now the first account is shown.
        let Some(forge) = ctx.forges.iter().next() else {
            self.summary = Summary::NoAccount;
            return;
        };
        if forge.focus().is_none() {
            self.summary = Summary::SignedIn(format!("@{}", forge.account().login));
            return;
        }
        // Keep what is on screen while refreshing again.
        if !matches!(self.summary, Summary::Day(_)) {
            self.summary = Summary::Day(Day::default());
        }
        let every = match ctx.refresh.focus_minutes {
            0 => None,
            minutes => Some(Duration::from_secs(u64::from(minutes) * 60)),
        };
        self.tasks = vec![
            sync::spawn_focus(Arc::clone(forge), ctx.cache.clone(), every, ctx.sender()),
            sync::spawn_unread(
                Arc::clone(forge),
                ctx.cache.clone(),
                ctx.refresh.notifications,
                ctx.sender(),
            ),
        ];
    }

    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        #[cfg(debug_assertions)]
        debug_panic(event);
        let Event::Key(key) = event else {
            return None;
        };
        match key.code {
            KeyCode::Char('l') if self.can_sign_in(ctx) => Some(Action::OpenLogin),
            KeyCode::Char('r') if matches!(self.summary, Summary::Day(_)) => {
                self.mount(ctx);
                Some(Action::Redraw)
            }
            _ => None,
        }
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
        let [
            logo,
            tagline,
            version,
            _,
            summary,
            status,
            _,
            coming,
            _,
            warnings,
        ] = Layout::vertical([
            Constraint::Length(LOGO.len() as u16),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            // The summary wraps onto a second line in narrow terminals.
            Constraint::Length(2),
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
        frame.render_widget(
            Paragraph::new(self.summary_line(ctx))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            summary,
        );
        frame.render_widget(
            Paragraph::new(self.status_line(ctx)).alignment(Alignment::Center),
            status,
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
            Paragraph::new(self.hints(ctx)).alignment(Alignment::Center),
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
    use hit_core::{Forges, Timestamp};
    use hit_forge_mock::{MockForge, MockOptions};
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::Services;

    fn demo_services() -> Services {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        Services {
            forges,
            ..Services::default()
        }
    }

    fn render_to_string(view: &mut Welcome, ctx: &Ctx) -> String {
        let mut terminal = Terminal::new(TestBackend::new(110, 34)).unwrap();
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

    fn day_view(focus: Option<Loaded<Arc<Vec<FocusItem>>>>) -> Welcome {
        let mut view = Welcome::new(Vec::new());
        view.summary = Summary::Day(Day {
            focus,
            ..Day::default()
        });
        view
    }

    fn loaded<T>(value: T, minutes_ago: i64, fresh: bool) -> Loaded<T> {
        Loaded {
            value,
            fetched_at: Timestamp::now()
                .checked_sub(jiff::SignedDuration::from_mins(minutes_ago))
                .unwrap(),
            fresh,
        }
    }

    #[test]
    fn shows_logo_tagline_hints_and_sign_in_hint() {
        let (ctx, _rx) = Ctx::for_tests(Services::default());
        let mut view = Welcome::new(Vec::new());
        view.mount(&ctx);
        let screen = render_to_string(&mut view, &ctx);
        assert!(screen.contains("██╗  ██╗"));
        assert!(screen.contains("focus first"));
        assert!(screen.contains("quit"));
        assert!(screen.contains("hit --demo"));
        assert!(!screen.contains("DEMO"));
        assert!(!screen.contains("refresh"));
        assert!(!screen.contains("warnings"));
    }

    #[test]
    fn shows_config_warnings() {
        let (ctx, _rx) = Ctx::for_tests(Services::default());
        let mut view = Welcome::new(vec!["unknown key ui.colour; ignored".into()]);
        let screen = render_to_string(&mut view, &ctx);
        assert!(screen.contains("warnings"));
        assert!(screen.contains("ui.colour"));
    }

    #[tokio::test]
    async fn demo_loads_summary_in_background() {
        let (ctx, mut rx) = Ctx::for_tests(demo_services());
        let mut view = Welcome::new(Vec::new());
        view.mount(&ctx);

        let screen = render_to_string(&mut view, &ctx);
        assert!(screen.contains("DEMO"));
        assert!(screen.contains("loading your day"));
        assert!(screen.contains("refresh"));

        let (mut focus, mut unread) = (false, false);
        while !(focus && unread) {
            let action = rx.recv().await.unwrap();
            focus |= matches!(action, Action::Data(DataEvent::Focus(Ok(_))));
            unread |= matches!(action, Action::Data(DataEvent::UnreadCount(Ok(_))));
            view.update(&action, &ctx);
        }
        let screen = render_to_string(&mut view, &ctx);
        assert!(screen.contains("to review"), "{screen}");
        assert!(screen.contains("CI failing"));
        assert!(screen.contains("unread"));
        assert!(screen.contains("updated just now"));
        assert!(!screen.contains("loading your day"));
    }

    #[test]
    fn freshness_is_always_visible() {
        let (ctx, _rx) = Ctx::for_tests(Services::default());
        let items = Arc::new(Vec::new());

        let mut cached = day_view(Some(loaded(Arc::clone(&items), 42, false)));
        let screen = render_to_string(&mut cached, &ctx);
        assert!(screen.contains("all clear"));
        assert!(screen.contains("cached · 42m ago"), "{screen}");

        let mut offline = day_view(Some(loaded(Arc::clone(&items), 180, true)));
        offline.on_data(&DataEvent::Focus(Err(ForgeError::Network(
            "offline".into(),
        ))));
        let screen = render_to_string(&mut offline, &ctx);
        assert!(screen.contains("all clear"), "old data stays visible");
        assert!(screen.contains("showing data from 3h ago"), "{screen}");

        let mut refreshing = day_view(Some(loaded(items, 5, true)));
        refreshing.on_data(&DataEvent::Refreshing);
        let screen = render_to_string(&mut refreshing, &ctx);
        assert!(screen.contains("refreshing · updated 5m ago"), "{screen}");
    }

    #[test]
    fn errors_without_data_are_shown() {
        let (ctx, _rx) = Ctx::for_tests(Services::default());
        let mut view = day_view(None);
        view.on_data(&DataEvent::Focus(Err(ForgeError::Network(
            "offline".into(),
        ))));
        let screen = render_to_string(&mut view, &ctx);
        assert!(screen.contains("couldn't load your day"));
        assert!(screen.contains("offline"));
    }
}
