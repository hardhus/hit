//! Pull requests: ready-made views (involved, mine, review, closed) across
//! all shown accounts. Inside a repository the first view is everything open
//! in it, most urgent first (audit D-045).

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use crossterm::event::KeyCode;
use hit_core::{
    AccountId, ForgeError, ItemRef,
    focus::{self, FocusReason},
    model::{CheckState, PrState, PullRequest, ReviewDecision},
    query::{PrQuery, Scope, StateFilter},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use super::{FocusAsks, JumpTarget, Tab, TabArea, focus::spinner, pr_fields};
use crate::{
    Action, Ctx, DataEvent, Event,
    action::{Detail, Target, Toast},
    anim::{Moment, Tween},
    browser,
    keymap::{Command, typed_char},
    sync::{self, Listing, Loaded},
    ui::{
        chips::{self, bold},
        list::{ItemList, ListState, Row},
    },
    views::Facts,
};

const ENTRANCE: Duration = Duration::from_millis(700);
/// How close to the end of a list its next page is asked for.
const LOAD_MORE_WITHIN: usize = 10;
const CURSOR_GLIDE: Duration = Duration::from_millis(90);

/// The ready-made views `f` cycles through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Involved,
    Mine,
    Review,
    Closed,
}

const VIEWS: [View; 4] = [View::Involved, View::Mine, View::Review, View::Closed];

impl View {
    fn key(self) -> &'static str {
        match self {
            Self::Involved => "involved",
            Self::Mine => "mine",
            Self::Review => "review",
            Self::Closed => "closed",
        }
    }

    fn title(self, in_repo: bool) -> &'static str {
        match self {
            Self::Involved if in_repo => "all open",
            Self::Involved => "involved",
            Self::Mine => "mine",
            Self::Review => "review requested",
            Self::Closed => "recently closed",
        }
    }

    fn query(self, me: &str) -> PrQuery {
        let mut query = PrQuery::default();
        match self {
            Self::Involved => {}
            Self::Mine => query.author = Some(me.to_owned()),
            Self::Review => query.reviewer = Some(me.to_owned()),
            Self::Closed => query.state = StateFilter::Closed,
        }
        query
    }

    /// What to fetch. A repository's first view also asks for what concerns
    /// the user directly, so it cannot fall off the end of a busy repository.
    /// Those come first: the review search's copy is the one that knows about
    /// requests to the user's teams.
    fn queries(self, me: &str, scope: &Scope) -> Vec<PrQuery> {
        let scoped = |mut query: PrQuery| {
            query.scope = scope.clone();
            query
        };
        let mut queries = Vec::new();
        if self == Self::Involved && scope.repo().is_some() {
            queries.push(scoped(Self::Review.query(me)));
            queries.push(scoped(Self::Mine.query(me)));
        }
        queries.push(scoped(self.query(me)));
        queries
    }
}

/// Groups of a repository's first view, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    NeedsYou,
    Yours,
    Others,
    Drafts,
}

impl Group {
    fn of(pr: &PullRequest, me: &str) -> Self {
        if !focus::pr_reasons(pr, me, false).is_empty() {
            Self::NeedsYou
        } else if pr.author.login == me {
            Self::Yours
        } else if pr.is_draft() {
            Self::Drafts
        } else {
            Self::Others
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::NeedsYou => "Needs you",
            Self::Yours => "Your pull requests",
            Self::Others => "Everything else",
            Self::Drafts => "Drafts",
        }
    }

    fn color(self, ctx: &Ctx) -> ratatui::style::Color {
        match self {
            Self::NeedsYou => ctx.theme.accent_alt,
            Self::Yours => ctx.theme.accent,
            Self::Others => ctx.theme.info,
            Self::Drafts => ctx.theme.muted,
        }
    }
}

/// A list position: a group header or a pull request.
enum Slot<'a> {
    Header(Group, usize),
    Item(&'a AccountId, &'a PullRequest),
}

#[derive(Default)]
struct Data {
    items: Option<Loaded<Arc<Listing<PullRequest>>>>,
    error: Option<ForgeError>,
    loading_more: bool,
}

#[derive(Default)]
pub struct PullsTab {
    /// Per account and view.
    data: HashMap<(AccountId, &'static str), Data>,
    tasks: Vec<JoinHandle<()>>,
    view: usize,
    filter: String,
    editing: bool,
    list: ListState,
    selected: Option<(AccountId, ItemRef)>,
    reveal_pending: bool,
    cursor: Option<Tween>,
    appeared: Option<Moment>,
    list_height: u16,
    /// Whether the rows are grouped, for the hint line.
    grouped: bool,
    asks: FocusAsks,
}

impl Drop for PullsTab {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A one-line summary of a PR's state for list rows.
pub fn pr_status_spans(pr: &PullRequest, ctx: &Ctx) -> Vec<Span<'static>> {
    let t = &ctx.theme;
    let muted = Style::new().fg(t.muted);
    let sep = || Span::styled(" · ", Style::new().fg(t.border));
    let mut spans = vec![Span::styled(format!("@{}", pr.author.login), muted)];
    if pr.is_draft() {
        spans.push(sep());
        spans.push(Span::styled("draft", bold(t.muted)));
    }
    if let Some(checks) = &pr.checks {
        let (text, color) = match checks.state {
            CheckState::Success => (format!("✓ {}", checks.passed), t.success),
            CheckState::Failure => (format!("✗ {}", checks.failed), t.danger),
            CheckState::Pending => (format!("◐ {}", checks.pending), t.warning),
            CheckState::Neutral => ("·".to_owned(), t.muted),
        };
        spans.push(sep());
        spans.push(Span::styled(text, bold(color)));
    }
    if let Some(decision) = pr.review_decision {
        let (text, color) = match decision {
            ReviewDecision::Approved => ("approved", t.success),
            ReviewDecision::ChangesRequested => ("changes requested", t.warning),
            ReviewDecision::ReviewRequired => ("review required", t.muted),
        };
        spans.push(sep());
        spans.push(Span::styled(text, bold(color)));
    }
    if let (Some(add), Some(del)) = (pr.additions, pr.deletions) {
        spans.push(sep());
        spans.push(Span::styled(format!("+{add}"), Style::new().fg(t.success)));
        spans.push(Span::styled(format!(" −{del}"), Style::new().fg(t.danger)));
    }
    if pr.comments > 0 {
        spans.push(sep());
        spans.push(Span::styled(chips::comments(pr.comments), muted));
    }
    spans.push(sep());
    spans.push(Span::styled(sync::ago(pr.updated_at), muted));
    spans
}

pub fn pr_icon(pr: &PullRequest, ctx: &Ctx) -> Span<'static> {
    let t = &ctx.theme;
    let color = match pr.state {
        PrState::Open if pr.is_draft() => t.muted,
        PrState::Open => t.success,
        PrState::Merged => t.highlight,
        PrState::Closed => t.danger,
    };
    Span::styled("◈", bold(color))
}

impl PullsTab {
    pub fn new() -> Self {
        Self::default()
    }

    fn stop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    fn current(&self) -> View {
        VIEWS.get(self.view).copied().unwrap_or(View::Involved)
    }

    fn shown_data<'a>(
        &'a self,
        ctx: &'a Ctx,
    ) -> impl Iterator<Item = (&'a AccountId, &'a Data)> + 'a {
        let view = self.current().key();
        self.data
            .iter()
            .filter(move |((account, v), _)| *v == view && ctx.shows(account))
            .map(|((account, _), data)| (account, data))
    }

    fn visible<'a>(&'a self, ctx: &'a Ctx) -> Vec<(&'a AccountId, &'a PullRequest)> {
        let needle = self.filter.to_lowercase();
        let mut items: Vec<_> = self
            .shown_data(ctx)
            .flat_map(|(account, data)| {
                data.items
                    .iter()
                    .flat_map(|l| l.value.iter())
                    .map(move |pr| (account, pr))
            })
            // GitHub's search can be behind: it may still call a merged pull
            // request open. Each item's own state is not behind.
            .filter(|(_, pr)| self.current() == View::Closed || pr.is_open())
            .filter(|(_, pr)| {
                needle.is_empty()
                    || pr.title.to_lowercase().contains(&needle)
                    || pr.reference.to_string().to_lowercase().contains(&needle)
                    || pr.author.login.to_lowercase().contains(&needle)
            })
            .collect();
        // Two accounts can see the same pull request; keep the one it asks
        // the most of.
        items.sort_by(|(a, x), (b, y)| {
            self.group(a, x)
                .cmp(&self.group(b, y))
                .then_with(|| a.login.cmp(&b.login))
        });
        let mut seen = HashSet::new();
        items.retain(|(_, pr)| seen.insert(&pr.reference));
        items.sort_by_key(|(_, pr)| std::cmp::Reverse(pr.updated_at));
        items
    }

    /// Near the end of the list, asks for what comes next.
    fn load_more(&mut self, ctx: &Ctx) {
        if !self.filter.is_empty()
            || self.rows(ctx).len().saturating_sub(self.list.selected) > LOAD_MORE_WITHIN
        {
            return;
        }
        let view = self.current();
        for forge in ctx.scoped_forges().filter(|f| ctx.shows(f.account())) {
            let account = forge.account().clone();
            let Some(data) = self.data.get_mut(&(account.clone(), view.key())) else {
                continue;
            };
            // A cached list's cursor may be stale; wait for the fresh one.
            let Some(cursor) = data
                .items
                .as_ref()
                .filter(|loaded| loaded.fresh)
                .and_then(|loaded| loaded.value.next.clone())
            else {
                continue;
            };
            if data.loading_more || data.error.is_some() {
                continue;
            }
            let Some(query) = view.queries(&account.login, &ctx.scope).pop() else {
                continue;
            };
            data.loading_more = true;
            self.tasks.push(sync::spawn_more_pulls(
                Arc::clone(forge),
                view.key(),
                query,
                cursor,
                ctx.sender(),
            ));
        }
    }

    /// `12 shown`, or `50 / 312` while there is more to come.
    fn count_text(&self, shown: usize, ctx: &Ctx) -> String {
        let total: u32 = self
            .shown_data(ctx)
            .filter_map(|(_, data)| data.items.as_ref()?.value.total)
            .sum();
        let loading = self.shown_data(ctx).any(|(_, data)| data.loading_more);
        if self.filter.is_empty() && total as usize > shown {
            format!("{shown} / {total}{} ", if loading { " …" } else { "" })
        } else {
            format!("{shown} shown ")
        }
    }

    fn is_grouped(&self, ctx: &Ctx) -> bool {
        ctx.scope.repo().is_some() && self.current() == View::Involved
    }

    /// The list: flat, or under group headers in a repository's first view.
    fn slots<'a>(&'a self, ctx: &'a Ctx) -> Vec<Slot<'a>> {
        let mut items = self.visible(ctx);
        if !self.is_grouped(ctx) {
            return items.into_iter().map(|(a, pr)| Slot::Item(a, pr)).collect();
        }
        // Stable: keeps the time order inside each group.
        items.sort_by_key(|(account, pr)| self.group(account, pr));
        let mut slots = Vec::with_capacity(items.len() + 4);
        let mut current = None;
        for (index, (account, pr)) in items.iter().enumerate() {
            let group = self.group(account, pr);
            if current != Some(group) {
                let count = items[index..]
                    .iter()
                    .take_while(|(a, p)| self.group(a, p) == group)
                    .count();
                slots.push(Slot::Header(group, count));
                current = Some(group);
            }
            slots.push(Slot::Item(account, pr));
        }
        slots
    }

    fn header(group: Group, count: usize, ctx: &Ctx) -> Row {
        let t = &ctx.theme;
        let color = group.color(ctx);
        Row::Header(Line::from(vec![
            Span::styled("▸ ", bold(color)),
            Span::styled(group.title(), bold(color)),
            Span::styled(format!("  {count} "), Style::new().fg(t.muted)),
            Span::styled("─".repeat(200), Style::new().fg(t.border)),
        ]))
    }

    /// What this pull request asks of the user, shown in a repository's
    /// first view where everyone's pull requests are mixed.
    fn group(&self, account: &AccountId, pr: &PullRequest) -> Group {
        if self.asks.reasons(account, &pr.reference).is_empty() {
            Group::of(pr, &account.login)
        } else {
            Group::NeedsYou
        }
    }

    fn reasons(&self, account: &AccountId, pr: &PullRequest, ctx: &Ctx) -> Vec<FocusReason> {
        if self.is_grouped(ctx) {
            let mut reasons = focus::pr_reasons(pr, &account.login, false);
            reasons.extend_from_slice(self.asks.reasons(account, &pr.reference));
            reasons.sort();
            reasons.dedup();
            reasons
        } else {
            Vec::new()
        }
    }

    fn row(&self, account: &AccountId, pr: &PullRequest, ctx: &Ctx) -> Row {
        let t = &ctx.theme;
        let mut top = Vec::new();
        if let Some(badge) = chips::account_badge(account, ctx) {
            top.push(badge);
        }
        top.push(pr_icon(pr, ctx));
        top.extend(chips::item_ref(&pr.reference, ctx));
        top.push(Span::styled(pr.title.clone(), bold(t.fg)));
        let reasons = self.reasons(account, pr, ctx);
        let accent = match pr.checks.map(|c| c.state) {
            _ if !reasons.is_empty() => chips::reasons_color(&reasons, t),
            Some(CheckState::Failure) => t.danger,
            _ if pr.review_decision == Some(ReviewDecision::ChangesRequested) => t.warning,
            _ if pr.review_decision == Some(ReviewDecision::Approved) => t.success,
            _ => t.accent,
        };
        let mut bottom = Vec::new();
        for reason in reasons {
            bottom.push(chips::reason_span(reason, t));
            bottom.push(Span::styled(" · ", Style::new().fg(t.border)));
        }
        bottom.extend(pr_status_spans(pr, ctx));
        Row::item(Line::from(top), Line::from(bottom), accent)
    }

    fn rows(&self, ctx: &Ctx) -> Vec<Row> {
        self.slots(ctx)
            .into_iter()
            .map(|slot| match slot {
                Slot::Header(group, count) => Self::header(group, count, ctx),
                Slot::Item(account, pr) => self.row(account, pr, ctx),
            })
            .collect()
    }

    fn selected_pr(&self, ctx: &Ctx) -> Option<(AccountId, PullRequest)> {
        match self.slots(ctx).get(self.list.selected) {
            Some(Slot::Item(account, pr)) => Some(((*account).clone(), (*pr).clone())),
            _ => None,
        }
    }

    fn remember_selection(&mut self, ctx: &Ctx) {
        if let Some((account, pr)) = self.selected_pr(ctx) {
            self.selected = Some((account, pr.reference));
        }
    }

    fn restore_selection(&mut self, ctx: &Ctx) {
        let Some((account, item)) = &self.selected else {
            return;
        };
        let found = self.slots(ctx).iter().position(
            |slot| matches!(slot, Slot::Item(a, pr) if *a == account && &pr.reference == item),
        );
        if let Some(index) = found {
            self.list.selected = index;
        }
    }

    fn render_bar(&self, frame: &mut Frame, area: Rect, shown: usize, ctx: &Ctx) {
        let t = &ctx.theme;
        let in_repo = ctx.scope.repo().is_some();
        let mut spans = vec![Span::raw(" ")];
        for (index, view) in VIEWS.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw("  "));
            }
            if index == self.view {
                spans.push(Span::styled(
                    format!("◆ {}", view.title(in_repo)),
                    bold(t.accent_alt),
                ));
            } else {
                spans.push(Span::styled(view.title(in_repo), Style::new().fg(t.muted)));
            }
        }
        if self.editing || !self.filter.is_empty() {
            spans.push(Span::raw("   "));
            spans.push(Span::styled("/ ", bold(t.accent_alt)));
            spans.push(Span::styled(self.filter.clone(), Style::new().fg(t.fg)));
            if self.editing {
                spans.push(Span::styled("▏", bold(t.accent_alt)));
            }
        }
        let count = self.count_text(shown, ctx);
        let room = area
            .width
            .saturating_sub(u16::try_from(count.chars().count()).unwrap_or(0));
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect {
                width: room,
                ..area
            },
        );
        frame.render_widget(
            Paragraph::new(Span::styled(count, Style::new().fg(t.muted)))
                .alignment(Alignment::Right),
            area,
        );
    }

    fn render_empty(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let data: Vec<&Data> = self.shown_data(ctx).map(|(_, d)| d).collect();
        let loading =
            data.is_empty() || data.iter().any(|d| d.items.is_none() && d.error.is_none());
        let lines = if loading {
            vec![Line::from(vec![
                Span::styled(spinner(ctx), bold(t.accent)),
                Span::styled(" loading pull requests…", Style::new().fg(t.muted)),
            ])]
        } else if let Some(e) = data
            .iter()
            .find_map(|d| d.error.as_ref())
            .filter(|_| data.iter().all(|d| d.items.is_none()))
        {
            vec![
                Line::from(Span::styled("couldn't load pull requests", bold(t.danger))),
                Line::from(Span::styled(chips::problem(e), Style::new().fg(t.muted))),
            ]
        } else if !self.filter.is_empty() {
            vec![Line::from(Span::styled(
                "nothing matches · esc clears the filter",
                Style::new().fg(t.muted),
            ))]
        } else {
            vec![
                Line::from(Span::styled("no pull requests here", bold(t.fg))),
                Line::from(Span::styled(
                    "f switches between involved, mine, review requested and closed",
                    Style::new().fg(t.muted),
                )),
            ]
        };
        let [middle] = Layout::vertical([Constraint::Length(lines.len() as u16)])
            .flex(Flex::Center)
            .areas(area);
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), middle);
    }
}

impl Tab for PullsTab {
    fn title(&self) -> &'static str {
        "PRs"
    }

    fn count(&self, ctx: &Ctx) -> Option<usize> {
        Some(self.visible(ctx).len()).filter(|n| *n > 0)
    }

    fn mount(&mut self, ctx: &Ctx) {
        self.stop();
        let view = self.current();
        self.grouped = self.is_grouped(ctx);
        for forge in ctx.scoped_forges() {
            let account = forge.account().clone();
            self.data.entry((account.clone(), view.key())).or_default();
            self.tasks.push(sync::spawn_pulls(
                Arc::clone(forge),
                ctx.cache.clone(),
                view.key(),
                view.queries(&account.login, &ctx.scope),
                ctx.sender(),
            ));
        }
        if self.appeared.is_none() {
            self.appeared = Some(Moment::now());
        }
    }

    fn activated(&mut self, ctx: &Ctx) {
        let oldest = self
            .shown_data(ctx)
            .filter_map(|(_, data)| data.items.as_ref())
            .map(|loaded| loaded.fetched_at)
            .min();
        if oldest.is_some_and(|at| sync::is_stale(at, ctx.prefs.refresh.focus_minutes)) {
            self.mount(ctx);
        }
    }

    fn scope_changed(&mut self, ctx: &Ctx) {
        self.asks = FocusAsks::default();
        self.data.clear();
        self.view = 0;
        self.filter.clear();
        self.editing = false;
        self.list = ListState::default();
        self.selected = None;
        self.cursor = None;
        self.appeared = Some(Moment::now());
        self.mount(ctx);
    }

    fn command(&mut self, command: Command, ctx: &Ctx) -> Option<Action> {
        let rows = self.rows(ctx);
        let page = ListState::page(self.list_height) as isize;
        match command {
            Command::Down => {
                self.list.move_by(1, &rows);
                self.load_more(ctx);
            }
            Command::Up => self.list.move_by(-1, &rows),
            Command::Top => self.list.top(&rows),
            Command::Bottom => {
                self.list.bottom(&rows);
                self.load_more(ctx);
            }
            Command::PageDown => {
                self.list.move_by(page, &rows);
                self.load_more(ctx);
            }
            Command::PageUp => self.list.move_by(-page, &rows),
            Command::NextGroup => {
                self.list.jump_group(true, &rows);
                self.load_more(ctx);
            }
            Command::PrevGroup => self.list.jump_group(false, &rows),
            Command::Open => {
                let (account, pr) = self.selected_pr(ctx)?;
                let checks_first =
                    self.reasons(&account, &pr, ctx).first() == Some(&FocusReason::ChecksFailing);
                return Some(Action::ShowDetail(Detail::Pull {
                    account,
                    item: pr.reference.clone(),
                    summary: Some(Box::new(pr)),
                    checks_first,
                }));
            }
            Command::Browser => {
                let (_, pr) = self.selected_pr(ctx)?;
                return Some(Action::Toast(match browser::open(&pr.url) {
                    Ok(()) => Toast::success(format!("opened {}", pr.reference)),
                    Err(toast) => toast,
                }));
            }
            Command::CycleReason => {
                self.view = (self.view + 1) % VIEWS.len();
                self.list = ListState::default();
                self.appeared = Some(Moment::now());
                self.mount(ctx);
            }
            Command::Filter => self.editing = true,
            Command::Back if !self.filter.is_empty() => {
                self.filter.clear();
                self.restore_selection(ctx);
            }
            Command::Refresh => {
                self.mount(ctx);
                return Some(Action::Toast(Toast::info("refreshing…")));
            }
            _ => return None,
        }
        self.remember_selection(ctx);
        Some(Action::Redraw)
    }

    fn captures_input(&self) -> bool {
        self.editing
    }

    fn input(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let Event::Key(key) = event else {
            return None;
        };
        if let Some(c) = typed_char(key) {
            self.filter.push(c);
            self.list.top(&self.rows(ctx));
            return Some(Action::Redraw);
        }
        match key.code {
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Enter => {
                self.editing = false;
                if self.visible(ctx).len() == 1 {
                    self.list.top(&self.rows(ctx));
                    return self.command(Command::Open, ctx);
                }
            }
            KeyCode::Esc => {
                self.filter.clear();
                self.editing = false;
            }
            _ => return None,
        }
        self.list.top(&self.rows(ctx));
        Some(Action::Redraw)
    }

    fn update(&mut self, action: &Action, ctx: &Ctx) {
        self.asks.update(action, ctx);
        if let Action::Data(DataEvent::PullsMore {
            account,
            scope,
            view,
            cursor,
            result,
        }) = action
        {
            if *scope != ctx.scope {
                return;
            }
            if let Some(data) = self.data.get_mut(&(account.clone(), *view)) {
                data.loading_more = false;
                if let Some(loaded) = data
                    .items
                    .as_mut()
                    .filter(|loaded| loaded.value.next.as_ref() == Some(cursor))
                {
                    match result {
                        Ok(page) => Arc::make_mut(&mut loaded.value)
                            .extend(page.clone(), |pr| &pr.reference),
                        Err(e) => data.error = Some(e.clone()),
                    }
                }
            }
            self.restore_selection(ctx);
        }
        if let Action::Data(DataEvent::Pulls {
            account,
            scope,
            view,
            result,
        }) = action
        {
            if *scope != ctx.scope {
                return;
            }
            let data = self.data.entry((account.clone(), view)).or_default();
            match result {
                Ok(loaded) => {
                    if loaded.fresh {
                        data.error = None;
                    }
                    data.items = Some(loaded.clone());
                }
                Err(e) => data.error = Some(e.clone()),
            }
            self.restore_selection(ctx);
        }
    }

    fn render(&mut self, frame: &mut Frame, area: TabArea, ctx: &Ctx) {
        let t = &ctx.theme;
        if self.reveal_pending {
            self.reveal_pending = false;
            self.restore_selection(ctx);
        }
        let [bar, list_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area.list);
        let rows = self.rows(ctx);
        self.render_bar(
            frame,
            Rect { height: 1, ..bar },
            rows.iter().filter(|r| r.is_item()).count(),
            ctx,
        );
        self.list_height = list_area.height;

        if rows.is_empty() {
            self.render_empty(frame, list_area, ctx);
        } else {
            let appeared = if ctx.animations { self.appeared } else { None };
            let selected = self.list.selected.min(rows.len() - 1) as f32;
            let cursor = self.cursor.get_or_insert_with(|| Tween::settled(selected));
            if cursor.target() != selected {
                let glide = if ctx.animations {
                    CURSOR_GLIDE
                } else {
                    Duration::ZERO
                };
                cursor.retarget(selected, glide);
            }
            ItemList {
                rows: &rows,
                theme: t,
                appeared,
                cursor: Some(cursor.value()),
            }
            .render(frame, list_area, &mut self.list);
        }

        if let Some(preview) = area.preview {
            let divider = Rect {
                x: preview.x.saturating_sub(1),
                width: 1,
                ..preview
            };
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled("│", Style::new().fg(t.border));
                    usize::from(divider.height)
                ]),
                divider,
            );
            if let Some((account, pr)) = self.selected_pr(ctx) {
                let mut lines = vec![Line::from(vec![
                    pr_icon(&pr, ctx),
                    Span::styled(format!(" {}", pr.reference), bold(t.accent)),
                    Span::styled(
                        if ctx.multi_account() {
                            format!("  @{}", account.login)
                        } else {
                            String::new()
                        },
                        bold(ctx.account_color(&account)),
                    ),
                ])];
                lines.push(Line::from(Span::styled(pr.title.clone(), bold(t.fg))));
                lines.push(Line::raw(""));
                lines.extend(pr_fields(&pr, ctx));
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    "↵ open · conversation, files and checks",
                    Style::new().fg(t.muted),
                ));
                frame.render_widget(
                    Paragraph::new(lines).wrap(Wrap { trim: false }),
                    Rect {
                        x: preview.x + 1,
                        width: preview.width.saturating_sub(2),
                        ..preview
                    },
                );
            }
        }
    }

    fn status(&self, ctx: &Ctx) -> Vec<Span<'static>> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let oldest = self
            .shown_data(ctx)
            .filter_map(|(_, d)| d.items.as_ref())
            .min_by_key(|l| l.fetched_at);
        let mut spans = Vec::new();
        if let Some(loaded) = oldest {
            spans.push(Span::styled(
                if loaded.fresh {
                    format!("updated {}", sync::ago(loaded.fetched_at))
                } else {
                    format!("cached · {}", sync::ago(loaded.fetched_at))
                },
                muted,
            ));
            if let Some(e) = self.shown_data(ctx).find_map(|(_, d)| d.error.as_ref()) {
                spans.push(Span::styled(
                    format!("  ·  ! couldn't refresh ({})", chips::problem(e)),
                    Style::new().fg(t.warning),
                ));
            }
        }
        spans
    }

    fn hints(&self) -> &'static [(&'static [Command], &'static str)] {
        if self.grouped {
            &[
                (&[Command::Down, Command::Up], "move"),
                (&[Command::NextGroup, Command::PrevGroup], "groups"),
                (&[Command::Open], "open"),
                (&[Command::CycleReason], "view"),
                (&[Command::Filter], "filter"),
                (&[Command::Browser], "browser"),
            ]
        } else {
            &[
                (&[Command::Down, Command::Up], "move"),
                (&[Command::Open], "open"),
                (&[Command::CycleReason], "view"),
                (&[Command::Filter], "filter"),
                (&[Command::Browser], "browser"),
            ]
        }
    }

    fn jump_targets(&self) -> Vec<JumpTarget> {
        self.data
            .iter()
            .flat_map(|((account, _), data)| {
                data.items
                    .iter()
                    .flat_map(|l| l.value.iter())
                    .map(move |pr| JumpTarget {
                        account: account.clone(),
                        item: pr.reference.clone(),
                        title: pr.title.clone(),
                    })
            })
            .collect()
    }

    fn reveal(&mut self, account: &AccountId, item: &ItemRef) -> bool {
        let known = self.data.iter().any(|((a, view), d)| {
            a == account
                && *view == self.current().key()
                && d.items
                    .as_ref()
                    .is_some_and(|l| l.value.iter().any(|pr| &pr.reference == item))
        });
        if known {
            self.filter.clear();
            self.selected = Some((account.clone(), item.clone()));
            self.reveal_pending = true;
        }
        known
    }

    fn is_animating(&self) -> bool {
        let entering = self.appeared.is_some_and(|at| at.within(ENTRANCE));
        let gliding = self.cursor.is_some_and(|c| c.is_running());
        let loading = self
            .data
            .values()
            .any(|d| d.items.is_none() && d.error.is_none());
        entering || gliding || loading
    }

    fn action_target(&self, ctx: &Ctx) -> Option<(Target, Facts)> {
        let (account, pr) = self.selected_pr(ctx)?;
        let facts = Facts::of_pull(&pr, &account.login);
        Some((
            Target {
                account,
                item: pr.reference,
            },
            facts,
        ))
    }

    fn shown(&mut self) {
        self.appeared = Some(Moment::now());
    }
}

#[cfg(test)]
mod tests {
    use hit_core::Forges;
    use hit_forge_mock::{MockForge, MockOptions};

    use super::*;
    use crate::Services;

    async fn loaded() -> (PullsTab, Ctx, tokio::sync::mpsc::UnboundedReceiver<Action>) {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let mut tab = PullsTab::new();
        tab.mount(&ctx);
        while tab.visible(&ctx).is_empty() {
            let action = rx.recv().await.unwrap();
            tab.update(&action, &ctx);
        }
        (tab, ctx, rx)
    }

    #[tokio::test]
    async fn views_switch_and_filter() {
        let (mut tab, ctx, mut rx) = loaded().await;
        let involved = tab.visible(&ctx).len();
        assert!(involved > 0);
        assert!(tab.visible(&ctx).iter().all(|(_, pr)| pr.is_open()));

        tab.command(Command::CycleReason, &ctx); // mine
        while tab.visible(&ctx).is_empty() {
            let action = rx.recv().await.unwrap();
            tab.update(&action, &ctx);
        }
        assert!(
            tab.visible(&ctx)
                .iter()
                .all(|(_, pr)| pr.author.login == "you")
        );

        tab.command(Command::CycleReason, &ctx); // review requested
        while tab.visible(&ctx).is_empty() {
            let action = rx.recv().await.unwrap();
            tab.update(&action, &ctx);
        }
        assert!(
            tab.visible(&ctx)
                .iter()
                .all(|(_, pr)| pr.requested_reviewers.iter().any(|u| u.login == "you"))
        );
    }

    #[tokio::test]
    async fn a_review_asked_of_a_team_needs_you() {
        let (tab, ctx, _rx) = in_repo().await;
        assert!(
            View::Involved.queries("you", &ctx.scope)[0]
                .reviewer
                .is_some(),
            "the review search must win when results are merged"
        );
        let visible = tab.visible(&ctx);
        let (_, other) = visible
            .iter()
            .find(|(_, pr)| Group::of(pr, "you") == Group::Others)
            .unwrap();
        let mut pr = (*other).clone();
        pr.requested_reviewers = vec![hit_core::model::User::new("acme/core")];
        assert_eq!(Group::of(&pr, "you"), Group::Others);
        pr.review_requested_of_viewer = true;
        assert_eq!(Group::of(&pr, "you"), Group::NeedsYou);
    }

    #[tokio::test]
    async fn the_list_grows_near_its_end_and_says_how_much_is_left() {
        let (mut tab, ctx, mut rx) = in_repo().await;
        let view = tab.current().key();
        let account = ctx.forges.iter().next().unwrap().account().clone();

        // A list with more to come, as a long repository would have.
        let data = tab.data.get_mut(&(account.clone(), view)).unwrap();
        let loaded = data.items.as_mut().unwrap();
        let cursor = hit_core::query::Cursor("1".to_owned());
        let listing = Arc::make_mut(&mut loaded.value);
        listing.next = Some(cursor.clone());
        listing.total = Some(500);
        loaded.fresh = true;
        let shown = tab.visible(&ctx).len();
        assert_eq!(tab.count_text(shown, &ctx), format!("{shown} / 500 "));

        tab.command(Command::Bottom, &ctx);
        assert!(tab.data[&(account.clone(), view)].loading_more);
        assert!(tab.count_text(shown, &ctx).ends_with(" … "));

        // Asked once, however often the cursor moves meanwhile.
        let tasks = tab.tasks.len();
        tab.command(Command::Up, &ctx);
        tab.command(Command::Down, &ctx);
        assert_eq!(tab.tasks.len(), tasks);

        loop {
            let action = rx.recv().await.unwrap();
            let done = matches!(action, Action::Data(DataEvent::PullsMore { .. }));
            tab.update(&action, &ctx);
            if done {
                break;
            }
        }
        assert!(!tab.data[&(account, view)].loading_more);
    }

    #[tokio::test]
    async fn coming_back_to_old_data_refreshes_it_and_fresh_data_stays() {
        let forge = Arc::new(MockForge::demo(MockOptions::instant()));
        let mut forges = Forges::new();
        forges.add(Arc::clone(&forge) as Arc<dyn hit_core::Forge>);
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let mut tab = PullsTab::new();
        tab.mount(&ctx);
        while tab.visible(&ctx).is_empty() {
            let action = rx.recv().await.unwrap();
            tab.update(&action, &ctx);
        }
        assert_eq!(forge.calls().pulls, 1);

        tab.activated(&ctx);
        tokio::task::yield_now().await;
        assert_eq!(forge.calls().pulls, 1, "just loaded: no need");

        for data in tab.data.values_mut() {
            if let Some(loaded) = &mut data.items {
                loaded.fetched_at = hit_core::Timestamp::UNIX_EPOCH;
            }
        }
        tab.activated(&ctx);
        rx.recv().await.unwrap();
        assert_eq!(forge.calls().pulls, 2);
    }

    #[tokio::test]
    async fn what_action_required_says_counts_here_too() {
        let (mut tab, ctx, _rx) = in_repo().await;
        let account = ctx.forges.iter().next().unwrap().account().clone();
        let visible = tab.visible(&ctx);
        let (_, other) = visible
            .iter()
            .find(|(a, pr)| tab.group(a, pr) == Group::Others)
            .unwrap();
        let pr = (*other).clone();
        drop(visible);

        let item = hit_core::focus::FocusItem {
            subject: hit_core::focus::FocusSubject::PullRequest(pr.clone()),
            reasons: vec![FocusReason::Mentioned],
            updated_at: pr.updated_at,
        };
        tab.update(
            &Action::Data(DataEvent::Focus {
                account: account.clone(),
                scope: ctx.scope.clone(),
                result: Ok(Loaded {
                    value: Arc::new(vec![item]),
                    fetched_at: hit_core::Timestamp::now(),
                    fresh: true,
                }),
            }),
            &ctx,
        );

        assert_eq!(tab.group(&account, &pr), Group::NeedsYou);
        assert_eq!(tab.reasons(&account, &pr, &ctx), [FocusReason::Mentioned]);
    }

    #[tokio::test]
    async fn a_merged_pull_request_leaves_the_open_list_even_if_search_lags() {
        let (mut tab, ctx, _rx) = in_repo().await;
        let before = tab.visible(&ctx).len();
        for data in tab.data.values_mut() {
            if let Some(loaded) = &mut data.items {
                let listing = Arc::make_mut(&mut loaded.value);
                if let Some(pr) = listing.items.first_mut() {
                    pr.state = hit_core::model::PrState::Merged;
                }
            }
        }
        assert_eq!(tab.visible(&ctx).len(), before - 1);
    }

    #[tokio::test]
    async fn a_filter_never_pages_through_everything() {
        let (mut tab, ctx, _rx) = in_repo().await;
        tab.filter = "x".to_owned();
        let tasks = tab.tasks.len();
        tab.command(Command::Bottom, &ctx);
        assert_eq!(tab.tasks.len(), tasks);
    }

    async fn in_repo() -> (PullsTab, Ctx, tokio::sync::mpsc::UnboundedReceiver<Action>) {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        let repo = hit_core::RepoRef::new(
            hit_core::Host::new(hit_forge_mock::DEMO_HOST),
            "acme",
            "rocket",
        );
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            repo: Some(repo),
            ..Services::default()
        });
        let mut tab = PullsTab::new();
        tab.mount(&ctx);
        while tab.visible(&ctx).is_empty() {
            let action = rx.recv().await.unwrap();
            tab.update(&action, &ctx);
        }
        (tab, ctx, rx)
    }

    #[tokio::test]
    async fn a_repository_shows_what_needs_you_first() {
        let (tab, ctx, _rx) = in_repo().await;
        let shown = tab.visible(&ctx);
        assert!(
            shown
                .iter()
                .all(|(_, pr)| pr.reference.repo.to_string() == "acme/rocket")
        );
        let groups: Vec<Group> = tab
            .slots(&ctx)
            .iter()
            .filter_map(|slot| match slot {
                Slot::Header(group, _) => Some(*group),
                Slot::Item(..) => None,
            })
            .collect();
        assert_eq!(groups.first(), Some(&Group::NeedsYou), "{groups:?}");
        assert!(groups.is_sorted(), "{groups:?}");
        // The cursor starts on an item, not on the header.
        let mut list = ListState::default();
        list.fix(&tab.rows(&ctx));
        assert!(matches!(
            tab.slots(&ctx).get(list.selected),
            Some(Slot::Item(..))
        ));
    }

    #[tokio::test]
    async fn results_of_another_scope_are_dropped() {
        let (mut tab, ctx, _rx) = in_repo().await;
        let before = tab.visible(&ctx).len();
        let account = ctx.forges.iter().next().unwrap().account().clone();
        tab.update(
            &Action::Data(DataEvent::Pulls {
                account,
                scope: Scope::All,
                view: "involved",
                result: Ok(Loaded {
                    value: Arc::new(Listing {
                        items: Vec::new(),
                        next: None,
                        total: None,
                    }),
                    fetched_at: hit_core::Timestamp::now(),
                    fresh: true,
                }),
            }),
            &ctx,
        );
        assert_eq!(tab.visible(&ctx).len(), before);
    }

    #[tokio::test]
    async fn enter_opens_the_pull_request() {
        let (mut tab, ctx, _rx) = loaded().await;
        assert!(matches!(
            tab.command(Command::Open, &ctx),
            Some(Action::ShowDetail(Detail::Pull {
                summary: Some(_),
                ..
            }))
        ));
    }
}
