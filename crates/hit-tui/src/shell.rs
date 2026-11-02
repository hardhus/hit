//! The main screen: tab bar, the active tab, status and hint lines, plus
//! the palette, help and toasts on top.

mod palette;

use std::time::Duration;

use hit_core::{AccountId, ForgeKind, RepoRef, query::Scope as RepoScope};
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
        wrapped_height,
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
    help_scroll: u16,
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
            help_scroll: 0,
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
            tab.activated(ctx);
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
                self.help_scroll = 0;
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
            Command::Actions => {
                match self
                    .tabs
                    .get(self.active)
                    .and_then(|tab| tab.action_target(ctx))
                {
                    Some((target, facts)) => return Some(Action::OpenActions(target, facts)),
                    None => self
                        .toasts
                        .push(Toast::info("select a pull request or an issue")),
                }
            }
            Command::SignIn => {
                if Self::can_sign_in_here(ctx) {
                    return Some(Action::OpenLogin);
                }
                self.toasts.push(Toast::info(
                    "no account to sign in again here; use `hit auth login`",
                ));
            }
            // The banner's key; this is otherwise only used inside diffs.
            Command::ScrollRight if Self::can_sign_in_here(ctx) => {
                return Some(Action::OpenLogin);
            }
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

    /// Accounts whose token GitHub rejected.
    fn signed_out(ctx: &Ctx) -> Vec<&AccountId> {
        ctx.forges
            .iter()
            .filter(|forge| forge.health().token_rejected)
            .map(|forge| forge.account())
            .collect()
    }

    /// Whether the sign-in screen can repair one of them.
    fn can_sign_in_here(ctx: &Ctx) -> bool {
        ctx.login.as_ref().is_some_and(|login| {
            Self::signed_out(ctx)
                .iter()
                .any(|account| &account.host == login.host())
        })
    }

    /// One line above the list: the data below it is getting old and only the
    /// user can fix that.
    fn signed_out_banner(ctx: &Ctx) -> Option<Line<'static>> {
        let accounts = Self::signed_out(ctx);
        if accounts.is_empty() {
            return None;
        }
        let names: Vec<String> = accounts
            .iter()
            .map(|account| format!("@{}", account.login))
            .collect();
        let fix = if Self::can_sign_in_here(ctx) {
            format!(
                "press {} to sign in again",
                ctx.keymap.short(Command::ScrollRight)
            )
        } else {
            "run `hit auth login`".to_owned()
        };
        let t = &ctx.theme;
        Some(
            Line::from(format!(
                " ✗ {} signed out: GitHub rejected the token · {fix} · below is the last known data",
                names.join(", ")
            ))
            .style(bold(t.selection).bg(t.danger)),
        )
    }

    /// The tightest request budget of any account, once it is low.
    fn rate_limit_span(ctx: &Ctx) -> Option<Span<'static>> {
        let t = &ctx.theme;
        let rate = ctx
            .forges
            .iter()
            .filter_map(|forge| forge.health().rate_limit)
            .min_by_key(|rate| rate.remaining)?;
        let until = rate
            .reset_at
            .map(|at| format!(" · back at {}", chips::clock(at)))
            .unwrap_or_default();
        Some(if rate.is_exhausted() {
            Span::styled(format!("rate limit reached{until} "), bold(t.danger))
        } else {
            Span::styled(
                format!("rate limit low: {} left{until} ", rate.remaining),
                bold(t.warning),
            )
        })
    }

    fn render_status(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let mut left = vec![Span::raw(" ")];
        if let Some(tab) = self.tabs.get(self.active) {
            left.extend(tab.status(ctx));
        }
        frame.render_widget(Paragraph::new(Line::from(left)), area);

        let mut right = Vec::new();
        right.extend(Self::rate_limit_span(ctx));
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

    fn repo_badge(ctx: &Ctx) -> Line<'static> {
        ctx.scope.repo().map_or_else(Line::default, |repo| {
            Line::from(Span::styled(
                format!("⌂ {repo} "),
                bold(chips::repo_color(repo, &ctx.theme)),
            ))
        })
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
            let signed_out = forge.health().token_rejected;
            if signed_out {
                spans.push(Span::styled("✗ ", bold(t.danger)));
            }
            let style = if signed_out {
                bold(t.danger)
            } else if shown {
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
        let bound = Command::ALL
            .iter()
            .filter(|c| c.scope() == scope && !ctx.keymap.keys(**c).is_empty());
        for &command in bound {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {:<14}", ctx.keymap.label(command)),
                    bold(t.accent),
                ),
                Span::styled(command.description(), Style::new().fg(t.fg)),
            ]));
        }
        lines
    }

    fn render_help(&mut self, frame: &mut Frame, area: Rect, opened: Moment, ctx: &Ctx) {
        let t = &ctx.theme;
        let mut left = Self::help_section(Scope::Global, ctx);
        left.push(Line::raw(""));
        left.push(Line::styled(
            "Rebind under [keys] in config.toml;",
            Style::new().fg(t.muted),
        ));
        left.push(Line::styled(
            "press , to see each command's name there.",
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
        let overflows = height > area.height.saturating_sub(2);
        let inner = Popup {
            title: "keys",
            footer: Some(if overflows {
                "j/k scroll · esc close"
            } else {
                "esc close"
            }),
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
            let content = wrapped_height(&left, l.width).max(wrapped_height(&right, r.width));
            self.help_scroll = self.help_scroll.min(content.saturating_sub(inner.height));
            let scroll = (self.help_scroll, 0);
            frame.render_widget(
                Paragraph::new(left)
                    .wrap(Wrap { trim: false })
                    .scroll(scroll),
                l,
            );
            frame.render_widget(
                Paragraph::new(right)
                    .wrap(Wrap { trim: false })
                    .scroll(scroll),
                r,
            );
        } else {
            let mut lines = left;
            lines.push(Line::raw(""));
            lines.extend(right);
            let content = wrapped_height(&lines, inner.width);
            self.help_scroll = self.help_scroll.min(content.saturating_sub(inner.height));
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((self.help_scroll, 0)),
                inner,
            );
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
            match ctx.keymap.lookup(key, &[Scope::Global, Scope::List]) {
                Some(Command::Help | Command::Back | Command::Quit) => self.help = None,
                Some(Command::Down) => self.help_scroll = self.help_scroll.saturating_add(1),
                Some(Command::Up) => self.help_scroll = self.help_scroll.saturating_sub(1),
                Some(Command::PageDown) => self.help_scroll = self.help_scroll.saturating_add(10),
                Some(Command::PageUp) => self.help_scroll = self.help_scroll.saturating_sub(10),
                Some(Command::Top) => self.help_scroll = 0,
                // Stops at the end when drawn.
                Some(Command::Bottom) => self.help_scroll = u16::MAX,
                _ => {}
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
            // What was changed shows up everywhere it is listed.
            Action::Data(
                crate::DataEvent::ActionDone { result: Ok(()), .. } | crate::DataEvent::Settled,
            ) => {
                for tab in &mut self.tabs {
                    tab.mount(ctx);
                }
            }
            // Signed in again: fresh loops with the new token, nothing else moves.
            Action::AccountAdded(_) => {
                for tab in &mut self.tabs {
                    tab.mount(ctx);
                }
            }
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

    fn shows_toasts(&self) -> bool {
        true
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        self.toasts.prune();
        let mut regions = self.layout.regions(area);
        if let Some(banner) = Self::signed_out_banner(ctx) {
            frame.render_widget(
                Paragraph::new(banner),
                Rect {
                    x: area.x,
                    y: regions.list.y,
                    width: area.width,
                    height: 1,
                },
            );
            regions.list.y += 1;
            regions.list.height = regions.list.height.saturating_sub(1);
            if let Some(preview) = &mut regions.preview {
                preview.y += 1;
                preview.height = preview.height.saturating_sub(1);
            }
        }

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
            // On a narrow screen the repository matters more than who you are.
            right: vec![Self::accounts_line(ctx), Self::repo_badge(ctx)],
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
        if self
            .tabs
            .get(self.active)
            .is_some_and(|tab| tab.action_target(ctx).is_some())
        {
            items.insert(0, (&[Command::Actions], "actions"));
        }
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
        items.push((&[Command::Settings], "settings"));
        items.push((&[Command::Help], "help"));
        frame.render_widget(
            Paragraph::new(hints::line(
                &items,
                &ctx.keymap,
                t,
                regions.hints.width.saturating_sub(1),
            )),
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
    async fn starting_asks_only_for_what_is_needed() {
        let forge = Arc::new(MockForge::demo(MockOptions::instant()));
        let mut forges = Forges::new();
        forges.add(Arc::clone(&forge) as Arc<dyn hit_core::Forge>);
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let mut shell = Shell::new(Vec::new());
        shell.mount(&ctx);
        let mut waiting = ["focus", "inbox", "pulls"].to_vec();
        while !waiting.is_empty() {
            let action = rx.recv().await.unwrap();
            let done = match &action {
                Action::Data(crate::DataEvent::Focus { .. }) => "focus",
                Action::Data(crate::DataEvent::Notifications { .. }) => "inbox",
                Action::Data(crate::DataEvent::Pulls { .. }) => "pulls",
                _ => "",
            };
            waiting.retain(|w| *w != done);
            shell.update(&action, &ctx);
        }

        let calls = forge.calls();
        assert_eq!(calls.notifications, 1, "one notifications loop");
        assert_eq!(calls.pulls, 1, "one view");
        assert_eq!(calls.repos, 0, "Repos waits until it is opened");

        // Opening it asks once; coming back does not ask again.
        shell.switch(REPOS_TAB, &ctx);
        while !matches!(
            rx.recv().await.unwrap(),
            Action::Data(crate::DataEvent::Repos { .. })
        ) {}
        shell.switch(0, &ctx);
        shell.switch(REPOS_TAB, &ctx);
        tokio::task::yield_now().await;
        assert_eq!(forge.calls().repos, 1);
    }

    struct Rejected {
        account: hit_core::AccountId,
        health: hit_core::Health,
    }

    impl hit_core::Forge for Rejected {
        fn kind(&self) -> ForgeKind {
            ForgeKind::GitHub
        }

        fn account(&self) -> &hit_core::AccountId {
            &self.account
        }

        fn health(&self) -> hit_core::Health {
            self.health
        }
    }

    fn with_health(health: hit_core::Health) -> Ctx {
        let mut forges = Forges::new();
        forges.add(Arc::new(Rejected {
            account: hit_core::AccountId::new(hit_core::Host::new("example.test"), "someone"),
            health,
        }));
        Ctx::for_tests(Services {
            forges,
            ..Services::default()
        })
        .0
    }

    fn screen(shell: &mut Shell, ctx: &Ctx) -> String {
        let mut terminal = Terminal::new(TestBackend::new(160, 24)).unwrap();
        terminal
            .draw(|frame| shell.render(frame, frame.area(), ctx))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(160)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn a_rejected_token_is_impossible_to_miss_and_says_what_to_do() {
        let ctx = with_health(hit_core::Health {
            token_rejected: true,
            rate_limit: None,
        });
        let text = screen(&mut Shell::new(Vec::new()), &ctx);
        assert!(text.contains("✗ @someone"), "{text}");
        assert!(
            text.contains("signed out: GitHub rejected the token"),
            "{text}"
        );
        assert!(text.contains("hit auth login"), "no sign-in here: {text}");
    }

    #[tokio::test]
    async fn a_healthy_account_shows_no_banner_or_limit() {
        let ctx = with_health(hit_core::Health::default());
        let text = screen(&mut Shell::new(Vec::new()), &ctx);
        assert!(!text.contains("signed out"), "{text}");
        assert!(!text.contains("rate limit"), "{text}");
    }

    #[tokio::test]
    async fn a_low_or_spent_rate_limit_shows_up() {
        let low = |remaining| {
            with_health(hit_core::Health {
                token_rejected: false,
                rate_limit: Some(hit_core::RateLimit {
                    remaining,
                    limit: 5000,
                    reset_at: None,
                }),
            })
        };
        let text = screen(&mut Shell::new(Vec::new()), &low(312));
        assert!(text.contains("rate limit low: 312 left"), "{text}");
        let text = screen(&mut Shell::new(Vec::new()), &low(0));
        assert!(text.contains("rate limit reached"), "{text}");
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

    fn screen_at(shell: &mut Shell, ctx: &Ctx, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| shell.render(frame, frame.area(), ctx))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn every_key_can_be_read_on_a_small_terminal() {
        let (ctx, _rx) = demo_ctx();
        let mut shell = Shell::new(Vec::new());
        shell.handle_event(&key(KeyCode::Char('?')), &ctx);
        let top = screen_at(&mut shell, &ctx, 80, 24);
        assert!(top.contains("Quit hit"), "{top}");
        assert!(top.contains("j/k scroll"), "{top}");
        assert!(!top.contains("Scroll a diff right"), "{top}");

        shell.handle_event(&key(KeyCode::Char('G')), &ctx);
        let bottom = screen_at(&mut shell, &ctx, 80, 24);
        assert!(bottom.contains("Scroll a diff right"), "{bottom}");

        // A key press past the end still scrolls back at once.
        let end = shell.help_scroll;
        shell.handle_event(&key(KeyCode::Char('j')), &ctx);
        let _ = screen_at(&mut shell, &ctx, 80, 24);
        assert_eq!(shell.help_scroll, end);
        shell.handle_event(&key(KeyCode::Char('k')), &ctx);
        assert_eq!(shell.help_scroll, end - 1);
    }

    #[tokio::test]
    async fn help_is_always_on_the_hint_line_and_tabs_stay_readable() {
        let (ctx, _rx) = demo_ctx();
        let mut shell = Shell::new(Vec::new());
        for (width, height) in [(80, 24), (60, 20)] {
            let text = screen_at(&mut shell, &ctx, width, height);
            let hints = text.lines().last().unwrap();
            assert!(hints.contains("?  help"), "{hints}");
        }
        for width in [20, 40, 60, 72, 80, 100] {
            let text = screen_at(&mut shell, &ctx, width, 20);
            let tabs = text.lines().next().unwrap();
            if width >= 80 {
                assert!(tabs.contains("5 Repos"), "{tabs}");
            }
            assert!(!tabs.contains("Repo@"), "{tabs}");
        }
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
