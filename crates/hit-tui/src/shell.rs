//! The main screen: tab bar, the active tab, status and hint lines, plus
//! the palette, help and toasts on top.

mod palette;

use std::time::Duration;

use hit_core::{ForgeKind, RepoRef, query::Scope as RepoScope};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use crate::{
    Action, Ctx, Event, View,
    action::Toast,
    anim::{Moment, Tween},
    browser,
    keymap::{Command, Scope},
    layout::{LayoutStrategy, TabsLayout},
    screens::{FocusTab, InboxTab, IssuesTab, PullsTab, ReposTab, Tab, TabArea},
    ui::{
        chips::{self, bold},
        hints,
        popup::Popup,
        tabs::{TabBar, TabSpec},
        toast::Toasts,
    },
};
use palette::{Entry, Outcome, Palette};

const GLIDE: Duration = Duration::from_millis(220);
/// A repository opens on its pull requests (audit D-045).
const PULLS_TAB: usize = 2;
const REPOS_TAB: usize = 4;

pub struct Shell {
    tabs: Vec<Box<dyn Tab>>,
    active: usize,
    underline: (Tween, Tween),
    positions: Vec<(u16, u16)>,
    layout: Box<dyn LayoutStrategy>,
    palette: Option<Palette>,
    help: Option<Moment>,
    toasts: Toasts,
    warnings: Vec<String>,
    /// Where `.` goes back to after showing all repositories.
    last_repo: Option<RepoRef>,
    /// A setting asks for fresh refresh loops. They start when the settings
    /// screen closes, so holding a key down does not hit the forge each step.
    reload_pending: bool,
}

impl Shell {
    pub fn new(warnings: Vec<String>) -> Self {
        Self {
            tabs: vec![
                Box::new(FocusTab::new()),
                Box::new(InboxTab::new()),
                Box::new(PullsTab::new()),
                Box::new(IssuesTab::new()),
                Box::new(ReposTab::new()),
            ],
            active: 0,
            underline: (Tween::settled(f32::NAN), Tween::settled(0.0)),
            positions: Vec::new(),
            layout: Box::new(TabsLayout::default()),
            palette: None,
            help: None,
            toasts: Toasts::default(),
            warnings,
            last_repo: None,
            reload_pending: false,
        }
    }

    fn switch(&mut self, index: usize, ctx: &Ctx) {
        if index >= self.tabs.len() || index == self.active {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.hidden(ctx);
        }
        self.active = index;
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.shown();
        }
        if let Some(&(x, w)) = self.positions.get(index) {
            let duration = if ctx.animations {
                GLIDE
            } else {
                Duration::ZERO
            };
            self.underline.0.retarget(f32::from(x), duration);
            self.underline.1.retarget(f32::from(w), duration);
        }
    }

    fn active_tab(&mut self) -> Option<&mut Box<dyn Tab>> {
        self.tabs.get_mut(self.active)
    }

    /// Runs a command from a key or the palette.
    fn run(&mut self, command: Command, ctx: &Ctx) -> Option<Action> {
        let count = self.tabs.len();
        match command {
            Command::Quit => return Some(Action::Quit),
            Command::Help => {
                self.help = Some(Moment::now());
            }
            Command::Palette => {
                self.palette = Some(Palette::new());
            }
            Command::NextTab => self.switch((self.active + 1) % count, ctx),
            Command::PrevTab => self.switch((self.active + count - 1) % count, ctx),
            Command::Tab1 => self.switch(0, ctx),
            Command::Tab2 => self.switch(1, ctx),
            Command::Tab3 => self.switch(2, ctx),
            Command::Tab4 => self.switch(3, ctx),
            Command::Tab5 => self.switch(4, ctx),
            Command::ToggleScope => {
                if ctx.scope.repo().is_some() {
                    return Some(Action::SetScope(RepoScope::All));
                }
                if let Some(repo) = self.last_repo.clone().or_else(|| ctx.home_repo.clone()) {
                    return Some(Action::SetScope(RepoScope::Repo(repo)));
                }
                self.switch(REPOS_TAB, ctx);
                self.toasts.push(Toast::info("pick a repository"));
            }
            Command::Settings => return Some(Action::OpenSettings),
            Command::CycleAccount if ctx.multi_account() => return Some(Action::CycleAccount),
            Command::CycleAccount => {
                self.toasts
                    .push(Toast::info("only one account is signed in"));
            }
            other => {
                let handled = self.active_tab().and_then(|tab| tab.command(other, ctx));
                // Esc with nothing to close must not leave the main screen.
                return handled.or(Some(Action::Redraw));
            }
        }
        Some(Action::Redraw)
    }

    fn run_entry(&mut self, entry: Entry, ctx: &Ctx) -> Option<Action> {
        match entry {
            Entry::Command(command) => self.run(command, ctx),
            Entry::Jump(target) => {
                if let Some(index) = self
                    .tabs
                    .iter_mut()
                    .position(|tab| tab.reveal(&target.account, &target.item))
                {
                    self.switch(index, ctx);
                }
                Some(Action::Redraw)
            }
            Entry::Url { url, .. } => Some(Action::Toast(match browser::open(&url) {
                Ok(()) => Toast::success("opened in the browser"),
                Err(toast) => toast,
            })),
        }
    }

    fn jump_targets(&self) -> Vec<crate::screens::JumpTarget> {
        self.tabs.iter().flat_map(|t| t.jump_targets()).collect()
    }

    fn render_status(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let mut left = vec![Span::raw(" ")];
        if let Some(tab) = self.tabs.get(self.active) {
            left.extend(tab.status(ctx));
        }
        frame.render_widget(Paragraph::new(Line::from(left)), area);

        let mut right = Vec::new();
        if ctx.forges.is_demo() {
            right.push(Span::styled(
                " DEMO ",
                Style::new()
                    .fg(t.selection)
                    .bg(t.accent_alt)
                    .add_modifier(Modifier::BOLD),
            ));
            right.push(Span::raw(" "));
        }
        if let Some(account) = &ctx.account_filter {
            right.push(Span::styled(
                format!("only @{} ", account.login),
                bold(ctx.account_color(account)),
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(right)).alignment(ratatui::layout::Alignment::Right),
            area,
        );
    }

    fn accounts_line(ctx: &Ctx) -> Line<'static> {
        let t = &ctx.theme;
        let mut spans = Vec::new();
        if let Some(repo) = ctx.scope.repo() {
            spans.push(Span::styled(
                format!("⌂ {repo}"),
                bold(chips::repo_color(repo, t)),
            ));
            spans.push(Span::raw("   "));
        }
        for forge in ctx.forges.iter() {
            let account = forge.account();
            let shown = ctx.shows(account);
            let style = if shown {
                bold(ctx.account_color(account))
            } else {
                Style::new().fg(t.muted).add_modifier(Modifier::DIM)
            };
            let host = if forge.kind() == ForgeKind::GitHub && account.host.as_str() == "github.com"
            {
                String::new()
            } else {
                format!(" {}", account.host)
            };
            spans.push(Span::styled(format!("@{}{host}", account.login), style));
            spans.push(Span::raw("  "));
        }
        Line::from(spans)
    }

    fn help_section(scope: Scope, ctx: &Ctx) -> Vec<Line<'static>> {
        let t = &ctx.theme;
        let mut lines = vec![Line::from(Span::styled(scope.title(), bold(t.accent_alt)))];
        for &command in Command::ALL.iter().filter(|c| c.scope() == scope) {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<14}", ctx.keymap.label(command)),
                    bold(t.accent),
                ),
                Span::styled(command.description(), Style::new().fg(t.fg)),
                Span::styled(format!("  {}", command.name()), Style::new().fg(t.muted)),
            ]));
        }
        lines
    }

    fn render_help(&self, frame: &mut Frame, area: Rect, opened: Moment, ctx: &Ctx) {
        let t = &ctx.theme;
        let mut left = Self::help_section(Scope::Global, ctx);
        left.push(Line::raw(""));
        left.push(Line::styled(
            "Rebind with the dim names under [keys] in config.toml,",
            Style::new().fg(t.muted),
        ));
        left.push(Line::styled(
            "e.g. down = [\"j\", \"n\"]",
            Style::new().fg(t.muted),
        ));
        if !self.warnings.is_empty() {
            left.push(Line::raw(""));
            left.push(Line::from(Span::styled("Warnings", bold(t.warning))));
            for warning in &self.warnings {
                left.push(Line::from(vec![
                    Span::styled("  ! ", bold(t.warning)),
                    Span::styled(warning.clone(), Style::new().fg(t.fg)),
                ]));
            }
        }
        let right = Self::help_section(Scope::List, ctx);

        // Two columns when there is room, so it fits small terminals too.
        let two_columns = area.width >= 100;
        let (width, height) = if two_columns {
            (108, left.len().max(right.len()) as u16 + 2)
        } else {
            (60, (left.len() + right.len() + 1) as u16 + 2)
        };
        let inner = Popup {
            title: "keys",
            footer: Some("esc close"),
            width,
            height,
            border: t.accent,
            theme: t,
            opened: ctx.animations.then_some(opened),
        }
        .render(frame, area);
        let inner = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };
        if two_columns {
            let [l, r] = ratatui::layout::Layout::horizontal([
                ratatui::layout::Constraint::Percentage(50),
                ratatui::layout::Constraint::Percentage(50),
            ])
            .areas(inner);
            frame.render_widget(Paragraph::new(left).wrap(Wrap { trim: true }), l);
            frame.render_widget(Paragraph::new(right), r);
        } else {
            let mut lines = left;
            lines.push(Line::raw(""));
            lines.extend(right);
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        }
    }
}

impl View for Shell {
    fn mount(&mut self, ctx: &Ctx) {
        if ctx.scope.repo().is_some() {
            self.active = PULLS_TAB;
        }
        for tab in &mut self.tabs {
            tab.mount(ctx);
        }
        if !self.warnings.is_empty() {
            let count = self.warnings.len();
            self.toasts.push(Toast::warning(format!(
                "{count} warning{} · press ? to see {}",
                if count == 1 { "" } else { "s" },
                if count == 1 { "it" } else { "them" }
            )));
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let Event::Key(key) = event else {
            return None;
        };

        if self.palette.is_some() {
            let targets = self.jump_targets();
            let palette = self.palette.as_mut()?;
            let entries = palette.entries(&targets, ctx);
            return match palette.handle(key, &entries) {
                Some(Outcome::Close) => {
                    self.palette = None;
                    Some(Action::Redraw)
                }
                Some(Outcome::Run(entry)) => {
                    self.palette = None;
                    self.run_entry(entry, ctx)
                }
                None => Some(Action::Redraw),
            };
        }

        if self.help.is_some() {
            let close = matches!(
                ctx.keymap.lookup(key, &[Scope::Global, Scope::List]),
                Some(Command::Help | Command::Back | Command::Quit)
            );
            if close {
                self.help = None;
            }
            return Some(Action::Redraw);
        }

        if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.captures_input()
        {
            return tab.input(event, ctx).or(Some(Action::Redraw));
        }

        let command = ctx.keymap.lookup(key, &[Scope::Global, Scope::List])?;
        self.run(command, ctx)
    }

    fn update(&mut self, action: &Action, ctx: &Ctx) -> Option<Action> {
        match action {
            Action::Toast(toast) => self.toasts.push(toast.clone()),
            Action::Run(command) => return self.run(*command, ctx),
            Action::CycleAccount => self.toasts.push(Toast::info(match &ctx.account_filter {
                Some(account) => format!("showing only @{}", account.login),
                None => "showing all accounts".to_owned(),
            })),
            Action::ConfigChanged(change) if change.reload => self.reload_pending = true,
            Action::Back if self.reload_pending => {
                self.reload_pending = false;
                for tab in &mut self.tabs {
                    tab.mount(ctx);
                }
            }
            Action::SetScope(scope) => {
                for tab in &mut self.tabs {
                    tab.scope_changed(ctx);
                }
                match scope.repo() {
                    Some(repo) => {
                        self.last_repo = Some(repo.clone());
                        // Picking a repository is a way in, not a place to stay.
                        if self.active == REPOS_TAB {
                            self.switch(PULLS_TAB, ctx);
                        }
                        self.toasts
                            .push(Toast::info(format!("only {repo} · . shows everything")));
                    }
                    None => self.toasts.push(Toast::info("showing all repositories")),
                }
            }
            _ => {}
        }
        for tab in &mut self.tabs {
            tab.update(action, ctx);
        }
        None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        self.toasts.prune();
        let regions = self.layout.regions(area);

        let specs: Vec<TabSpec> = self
            .tabs
            .iter()
            .map(|tab| TabSpec {
                title: tab.title(),
                count: tab.count(ctx),
            })
            .collect();
        let underline = (self.underline.0.value(), self.underline.1.value());
        let positions = TabBar {
            tabs: &specs,
            active: self.active,
            theme: t,
            underline: if underline.0.is_nan() {
                (0.0, 0.0)
            } else {
                underline
            },
            logo: true,
            phase: if ctx.animations {
                ctx.time() * 0.1
            } else {
                0.0
            },
            right: Self::accounts_line(ctx),
        }
        .render(frame, regions.tabs);
        // First frame (or a layout change): put the underline in place.
        if let Some(&(x, w)) = positions.get(self.active)
            && (self.underline.0.target().is_nan()
                || (!self.underline.0.is_running()
                    && (self.underline.0.target() != f32::from(x)
                        || self.underline.1.target() != f32::from(w))))
        {
            self.underline = (Tween::settled(f32::from(x)), Tween::settled(f32::from(w)));
        }
        self.positions = positions;

        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.render(
                frame,
                TabArea {
                    list: regions.list,
                    preview: regions.preview,
                },
                ctx,
            );
        }

        self.render_status(frame, regions.status, ctx);
        let mut items: Vec<(&[Command], &'static str)> = self
            .tabs
            .get(self.active)
            .map(|tab| tab.hints().to_vec())
            .unwrap_or_default();
        items.push((
            &[Command::ToggleScope],
            if ctx.scope.repo().is_some() {
                "all repos"
            } else {
                "repo"
            },
        ));
        items.push((&[Command::NextTab], "tabs"));
        items.push((&[Command::Palette], "palette"));
        items.push((&[Command::Help], "help"));
        items.push((&[Command::Settings], "settings"));
        frame.render_widget(
            Paragraph::new(hints::line(&items, &ctx.keymap, t)),
            Rect {
                x: regions.hints.x + 1,
                width: regions.hints.width.saturating_sub(1),
                ..regions.hints
            },
        );

        self.toasts.render(
            frame,
            Rect {
                y: regions.list.y,
                height: regions.list.height,
                ..area
            },
            t,
        );
        if let Some(opened) = self.help {
            self.render_help(frame, area, opened, ctx);
        }
        if let Some(palette) = &self.palette {
            let entries = palette.entries(&self.jump_targets(), ctx);
            palette.render(frame, area, &entries, ctx);
        }
    }

    fn is_animating(&self) -> bool {
        self.underline.0.is_running()
            || self.toasts.is_active()
            || Popup::is_animating(self.help)
            || Popup::is_animating(self.palette.as_ref().map(|p| p.opened))
            || self
                .tabs
                .get(self.active)
                .is_some_and(|tab| tab.is_animating())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use hit_core::Forges;
    use hit_forge_mock::{MockForge, MockOptions};
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::Services;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[tokio::test]
    async fn comma_opens_settings_and_refresh_changes_wait_for_it_to_close() {
        let (ctx, _rx) = Ctx::for_tests(Services::default());
        let mut shell = Shell::new(Vec::new());
        assert!(matches!(
            shell.handle_event(&key(KeyCode::Char(',')), &ctx),
            Some(Action::OpenSettings)
        ));

        let change = crate::action::ConfigChange {
            config: Box::default(),
            reload: true,
        };
        shell.update(&Action::ConfigChanged(change), &ctx);
        assert!(shell.reload_pending, "not while the screen is open");
        shell.update(&Action::Back, &ctx);
        assert!(!shell.reload_pending);
    }

    fn render(shell: &mut Shell, ctx: &Ctx) -> String {
        let mut terminal = Terminal::new(TestBackend::new(150, 36)).unwrap();
        terminal
            .draw(|frame| shell.render(frame, frame.area(), ctx))
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn demo_ctx() -> (Ctx, tokio::sync::mpsc::UnboundedReceiver<Action>) {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        Ctx::for_tests(Services {
            forges,
            ..Services::default()
        })
    }

    #[tokio::test]
    async fn tabs_switch_and_render() {
        let (ctx, _rx) = demo_ctx();
        let mut shell = Shell::new(Vec::new());
        shell.mount(&ctx);
        let screen = render(&mut shell, &ctx);
        assert!(screen.contains("Focus"));
        assert!(screen.contains("Inbox"));
        assert!(screen.contains("DEMO"));
        assert!(screen.contains("help"));

        shell.handle_event(&key(KeyCode::Char('3')), &ctx);
        assert_eq!(shell.active, 2);
        let screen = render(&mut shell, &ctx);
        assert!(screen.contains("review requested"), "{screen}");

        shell.handle_event(&key(KeyCode::Tab), &ctx);
        assert_eq!(shell.active, 3);
        shell.handle_event(
            &Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
            &ctx,
        );
        assert_eq!(shell.active, 2);
    }

    fn acme_rocket() -> RepoRef {
        RepoRef::new(
            hit_core::Host::new(hit_forge_mock::DEMO_HOST),
            "acme",
            "rocket",
        )
    }

    /// Does what the app does with a scope change.
    fn apply(shell: &mut Shell, ctx: &mut Ctx, action: Option<Action>) {
        let Some(Action::SetScope(scope)) = action else {
            panic!("expected a scope change, got {action:?}");
        };
        ctx.scope = scope.clone();
        shell.update(&Action::SetScope(scope), ctx);
    }

    #[tokio::test]
    async fn a_repository_opens_on_its_pull_requests_and_dot_leaves_it() {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        let (mut ctx, _rx) = Ctx::for_tests(Services {
            forges,
            repo: Some(acme_rocket()),
            ..Services::default()
        });
        let mut shell = Shell::new(Vec::new());
        shell.mount(&ctx);
        assert_eq!(shell.active, PULLS_TAB);
        let screen = render(&mut shell, &ctx);
        assert!(screen.contains("⌂ acme/rocket"), "{screen}");
        assert!(screen.contains("all open"), "{screen}");

        let action = shell.handle_event(&key(KeyCode::Char('.')), &ctx);
        apply(&mut shell, &mut ctx, action);
        assert_eq!(ctx.scope, RepoScope::All);
        let screen = render(&mut shell, &ctx);
        assert!(!screen.contains("⌂ acme/rocket"), "{screen}");
        assert!(screen.contains("involved"), "{screen}");

        // And back to the same repository.
        let action = shell.handle_event(&key(KeyCode::Char('.')), &ctx);
        apply(&mut shell, &mut ctx, action);
        assert_eq!(ctx.scope, RepoScope::Repo(acme_rocket()));
    }

    #[tokio::test]
    async fn outside_a_repository_it_opens_on_focus_and_dot_offers_repos() {
        let (ctx, _rx) = demo_ctx();
        let mut shell = Shell::new(Vec::new());
        shell.mount(&ctx);
        assert_eq!(shell.active, 0);
        shell.handle_event(&key(KeyCode::Char('.')), &ctx);
        assert_eq!(shell.active, REPOS_TAB);
        assert_eq!(ctx.scope, RepoScope::All);
    }

    #[tokio::test]
    async fn esc_never_leaves_the_main_screen() {
        let (ctx, _rx) = demo_ctx();
        let mut shell = Shell::new(Vec::new());
        assert!(matches!(
            shell.handle_event(&key(KeyCode::Esc), &ctx),
            Some(Action::Redraw)
        ));
    }

    #[tokio::test]
    async fn help_and_palette_open_and_close() {
        let (ctx, _rx) = demo_ctx();
        let mut shell = Shell::new(vec!["keys: unknown command \"x\"".into()]);
        shell.handle_event(&key(KeyCode::Char('?')), &ctx);
        let screen = render(&mut shell, &ctx);
        assert!(screen.contains("Move down"), "{screen}");
        assert!(screen.contains("Warnings"));
        shell.handle_event(&key(KeyCode::Esc), &ctx);
        assert!(shell.help.is_none());

        shell.handle_event(&key(KeyCode::Char(':')), &ctx);
        for c in "refr".chars() {
            shell.handle_event(&key(KeyCode::Char(c)), &ctx);
        }
        let screen = render(&mut shell, &ctx);
        assert!(screen.contains("Refresh now"), "{screen}");
        shell.handle_event(&key(KeyCode::Esc), &ctx);
        assert!(shell.palette.is_none());
    }
}
