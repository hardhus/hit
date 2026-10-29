//! Focus: what needs you, across all shown accounts.

use std::{collections::HashMap, sync::Arc, time::Duration};

use crossterm::event::KeyCode;
use hit_config::SortOrder;
use hit_core::{
    AccountId, ForgeError, ItemRef,
    focus::{FocusItem, FocusReason, FocusSubject},
    model::{Mergeable, PrState, PullRequest, ReviewDecision},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use super::{JumpTarget, Tab, TabArea};
use crate::{
    Action, Ctx, DataEvent, Event,
    action::{Detail, Toast},
    anim::{Moment, Tween},
    browser,
    keymap::{Command, typed_char},
    sync::{self, Loaded},
    ui::{
        chips::{self, bold},
        list::{ItemList, ListState, Row},
    },
};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// How long the entrance animation keeps frames coming.
const ENTRANCE: Duration = Duration::from_millis(700);
/// How long the highlight takes to reach a newly selected row.
const CURSOR_GLIDE: Duration = Duration::from_millis(90);

#[derive(Default)]
struct AccountData {
    focus: Option<Loaded<Arc<Vec<FocusItem>>>>,
    unread: Option<Loaded<usize>>,
    error: Option<ForgeError>,
    refreshing: bool,
}

#[derive(Default)]
pub struct FocusTab {
    accounts: HashMap<AccountId, AccountData>,
    tasks: Vec<JoinHandle<()>>,
    list: ListState,
    /// Keeps the selection on the same item across refreshes.
    selected: Option<(AccountId, ItemRef)>,
    filter: String,
    editing: bool,
    appeared: Option<Moment>,
    /// Height of the list on the last frame, for paging.
    list_height: u16,
    /// The highlight glides to the selected row.
    cursor: Option<Tween>,
    /// Runtime overrides of `[focus] sort` / `group` (`s` / `v`).
    sort: Option<SortOrder>,
    group: Option<bool>,
    /// A jump from the palette is applied on the next frame.
    reveal_pending: bool,
}

/// Focus groups, in priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Review,
    Mine,
    Others,
}

impl Group {
    fn of(item: &FocusItem) -> Self {
        match item.reasons.first() {
            Some(FocusReason::ReviewRequested) => Self::Review,
            Some(
                FocusReason::ChangesRequested
                | FocusReason::ChecksFailing
                | FocusReason::ReadyToMerge,
            ) => Self::Mine,
            _ => Self::Others,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Review => "Waiting for your review",
            Self::Mine => "Your pull requests",
            Self::Others => "Mentions & assigned",
        }
    }

    fn color(self, ctx: &Ctx) -> ratatui::style::Color {
        match self {
            Self::Review => ctx.theme.accent,
            Self::Mine => ctx.theme.accent_alt,
            Self::Others => ctx.theme.info,
        }
    }
}

/// A list position: a group header or an item.
enum Slot<'a> {
    Header(Group, usize),
    Item(&'a AccountId, &'a FocusItem),
}

impl Drop for FocusTab {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn spinner(ctx: &Ctx) -> &'static str {
    let frame = if ctx.animations {
        (ctx.time() * 12.0) as usize % SPINNER.len()
    } else {
        0
    };
    SPINNER.get(frame).copied().unwrap_or("·")
}

impl FocusTab {
    pub fn new() -> Self {
        Self::default()
    }

    fn stop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    fn sort_order(&self, ctx: &Ctx) -> SortOrder {
        self.sort.unwrap_or(ctx.prefs.focus_sort)
    }

    fn grouped(&self, ctx: &Ctx) -> bool {
        self.group.unwrap_or(ctx.prefs.focus_group)
    }

    /// The list: items in the chosen order, under group headers if grouped.
    fn slots<'a>(&'a self, ctx: &Ctx) -> Vec<Slot<'a>> {
        let mut items = self.visible(ctx);
        if self.sort_order(ctx) == SortOrder::Oldest {
            items.reverse();
        }
        if !self.grouped(ctx) {
            return items.into_iter().map(|(a, i)| Slot::Item(a, i)).collect();
        }
        // Stable: keeps the time order inside each group.
        items.sort_by_key(|(_, item)| Group::of(item));
        let mut slots = Vec::with_capacity(items.len() + 3);
        let mut current = None;
        for (index, (account, item)) in items.iter().enumerate() {
            let group = Group::of(item);
            if current != Some(group) {
                let count = items[index..]
                    .iter()
                    .take_while(|(_, i)| Group::of(i) == group)
                    .count();
                slots.push(Slot::Header(group, count));
                current = Some(group);
            }
            slots.push(Slot::Item(account, item));
        }
        slots
    }

    fn rows(&self, ctx: &Ctx) -> Vec<Row> {
        self.slots(ctx)
            .into_iter()
            .map(|slot| match slot {
                Slot::Header(group, count) => Self::header(group, count, ctx),
                Slot::Item(account, item) => Self::row(account, item, ctx),
            })
            .collect()
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

    /// Shown items: account filter, then the text filter, newest first.
    fn visible<'a>(&'a self, ctx: &Ctx) -> Vec<(&'a AccountId, &'a FocusItem)> {
        let needle = self.filter.to_lowercase();
        let mut items: Vec<_> = self
            .accounts
            .iter()
            .filter(|(account, _)| ctx.shows(account))
            .flat_map(|(account, data)| {
                data.focus
                    .iter()
                    .flat_map(|loaded| loaded.value.iter())
                    .map(move |item| (account, item))
            })
            .filter(|(_, item)| {
                needle.is_empty()
                    || item.subject.title().to_lowercase().contains(&needle)
                    || item
                        .subject
                        .reference()
                        .to_string()
                        .to_lowercase()
                        .contains(&needle)
            })
            .collect();
        items.sort_by_key(|(_, item)| std::cmp::Reverse(item.updated_at));
        items
    }

    fn shown_data<'a>(&'a self, ctx: &'a Ctx) -> impl Iterator<Item = &'a AccountData> + 'a {
        self.accounts
            .iter()
            .filter(|(account, _)| ctx.shows(account))
            .map(|(_, data)| data)
    }

    fn remember_selection(&mut self, ctx: &Ctx) {
        let picked = match self.slots(ctx).get(self.list.selected) {
            Some(Slot::Item(account, item)) => {
                Some(((*account).clone(), item.subject.reference().clone()))
            }
            _ => None,
        };
        if picked.is_some() {
            self.selected = picked;
        }
    }

    /// Puts the cursor back on the remembered item after data or order
    /// changed.
    fn restore_selection(&mut self, ctx: &Ctx) {
        let Some((account, item)) = &self.selected else {
            return;
        };
        let found = self.slots(ctx).iter().position(|slot| {
            matches!(slot, Slot::Item(a, i) if *a == account && i.subject.reference() == item)
        });
        if let Some(index) = found {
            self.list.selected = index;
        }
    }

    fn selected_item(&self, ctx: &Ctx) -> Option<(AccountId, FocusItem)> {
        match self.slots(ctx).get(self.list.selected) {
            Some(Slot::Item(account, item)) => Some(((*account).clone(), (*item).clone())),
            _ => None,
        }
    }

    fn row(account: &AccountId, item: &FocusItem, ctx: &Ctx) -> Row {
        let t = &ctx.theme;
        let mut top = Vec::new();
        if let Some(badge) = chips::account_badge(account, ctx) {
            top.push(badge);
        }
        top.push(chips::subject_icon(&item.subject, t));
        top.push(Span::styled(
            format!(" {} ", item.subject.reference()),
            Style::new().fg(t.muted),
        ));
        top.push(Span::styled(item.subject.title().to_owned(), bold(t.fg)));

        let mut bottom = Vec::new();
        for (index, reason) in item.reasons.iter().enumerate() {
            if index > 0 {
                bottom.push(Span::styled(" · ", Style::new().fg(t.border)));
            }
            bottom.push(chips::reason_span(*reason, t));
        }
        let author = match &item.subject {
            FocusSubject::PullRequest(pr) => &pr.author.login,
            FocusSubject::Issue(issue) => &issue.author.login,
        };
        bottom.push(Span::styled(
            format!("  @{author} · {}", sync::ago(item.updated_at)),
            Style::new().fg(t.muted),
        ));
        Row::item(
            Line::from(top),
            Line::from(bottom),
            chips::reasons_color(&item.reasons, t),
        )
    }

    fn render_empty(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let data: Vec<&AccountData> = self.shown_data(ctx).collect();
        let loading = data.iter().any(|d| d.focus.is_none() && d.error.is_none());
        let error = data.iter().find_map(|d| d.error.as_ref());
        let lines = if !self.filter.is_empty() && data.iter().any(|d| d.focus.is_some()) {
            vec![
                Line::from(Span::styled("nothing matches", bold(t.fg))),
                Line::from(Span::styled(
                    format!("“{}” · esc clears the filter", self.filter),
                    Style::new().fg(t.muted),
                )),
            ]
        } else if loading {
            vec![Line::from(vec![
                Span::styled(spinner(ctx), bold(t.accent)),
                Span::styled(" loading what needs you…", Style::new().fg(t.muted)),
            ])]
        } else if let Some(e) = error {
            vec![
                Line::from(Span::styled(
                    "couldn't load Action Required",
                    bold(t.danger),
                )),
                Line::from(Span::styled(e.to_string(), Style::new().fg(t.muted))),
            ]
        } else {
            vec![
                t.gradient_line_owned("all clear ✨".to_owned(), 12, ctx.time() * 0.2),
                Line::from(Span::styled(
                    "nothing needs you right now",
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

/// The preview of an item, also used by the full-screen detail view.
pub fn preview_lines(account: &AccountId, item: &FocusItem, ctx: &Ctx) -> Vec<Line<'static>> {
    let t = &ctx.theme;
    let muted = Style::new().fg(t.muted);
    let mut header = vec![
        chips::subject_icon(&item.subject, t),
        Span::styled(format!(" {}", item.subject.reference()), bold(t.accent)),
    ];
    if ctx.multi_account() {
        header.push(Span::styled(
            format!("  @{}", account.login),
            bold(ctx.account_color(account)),
        ));
    }
    let mut lines = vec![
        Line::from(header),
        Line::from(Span::styled(item.subject.title().to_owned(), bold(t.fg))),
        Line::raw(""),
    ];

    let mut reasons = Vec::new();
    for (index, reason) in item.reasons.iter().enumerate() {
        if index > 0 {
            reasons.push(Span::raw("   "));
        }
        reasons.push(chips::reason_span(*reason, t));
    }
    lines.push(Line::from(reasons));
    lines.push(Line::raw(""));

    let field = |name: &str, value: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(format!("{name:<10}"), muted)];
        spans.extend(value);
        Line::from(spans)
    };

    match &item.subject {
        FocusSubject::PullRequest(pr) => lines.extend(pr_fields(pr, ctx)),
        FocusSubject::Issue(issue) => {
            let state = if issue.is_open() {
                Span::styled("open", bold(t.success))
            } else {
                Span::styled("closed", bold(t.highlight))
            };
            lines.push(field("state", vec![state]));
            lines.push(field(
                "author",
                vec![Span::styled(
                    format!(
                        "@{} · opened {} · updated {}",
                        issue.author.login,
                        sync::ago(issue.created_at),
                        sync::ago(issue.updated_at)
                    ),
                    Style::new().fg(t.fg),
                )],
            ));
            lines.push(field(
                "comments",
                vec![Span::styled(
                    issue.comments.to_string(),
                    Style::new().fg(t.fg),
                )],
            ));
            if !issue.assignees.is_empty() {
                let names: Vec<String> = issue
                    .assignees
                    .iter()
                    .map(|u| format!("@{}", u.login))
                    .collect();
                lines.push(field(
                    "assignees",
                    vec![Span::styled(names.join(", "), Style::new().fg(t.fg))],
                ));
            }
            if !issue.labels.is_empty() {
                let mut spans = Vec::new();
                for label in &issue.labels {
                    spans.push(chips::label_span(label, t));
                    spans.push(Span::raw(" "));
                }
                lines.push(field("labels", spans));
            }
        }
    }
    lines
}

/// The fields of a pull request (state, branches, changes, review, checks…),
/// shared by the Focus and Pull requests previews.
pub fn pr_fields(pr: &PullRequest, ctx: &Ctx) -> Vec<Line<'static>> {
    let t = &ctx.theme;
    let muted = Style::new().fg(t.muted);
    let field = |name: &str, value: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(format!("{name:<10}"), muted)];
        spans.extend(value);
        Line::from(spans)
    };
    let mut lines = Vec::new();
    let state = match pr.state {
        PrState::Open if pr.is_draft() => Span::styled("draft", bold(t.muted)),
        PrState::Open => Span::styled("open", bold(t.success)),
        PrState::Merged => Span::styled("merged", bold(t.highlight)),
        PrState::Closed => Span::styled("closed", bold(t.danger)),
    };
    lines.push(field(
        "state",
        vec![
            state,
            Span::styled(
                format!("  {} ← {}", pr.base_ref, pr.head_ref),
                Style::new().fg(t.fg),
            ),
        ],
    ));
    lines.push(field(
        "author",
        vec![Span::styled(
            format!(
                "@{} · opened {} · updated {}",
                pr.author.login,
                sync::ago(pr.created_at),
                sync::ago(pr.updated_at)
            ),
            Style::new().fg(t.fg),
        )],
    ));
    if let (Some(add), Some(del)) = (pr.additions, pr.deletions) {
        let files = pr.changed_files.unwrap_or(0);
        lines.push(field(
            "changes",
            vec![
                Span::styled(format!("+{add}"), bold(t.success)),
                Span::raw(" "),
                Span::styled(format!("−{del}"), bold(t.danger)),
                Span::styled(
                    format!("  {files} file{}", if files == 1 { "" } else { "s" }),
                    muted,
                ),
            ],
        ));
    }
    if let Some(decision) = pr.review_decision {
        let (text, color) = match decision {
            ReviewDecision::Approved => ("approved", t.success),
            ReviewDecision::ChangesRequested => ("changes requested", t.warning),
            ReviewDecision::ReviewRequired => ("review required", t.muted),
        };
        lines.push(field("review", vec![Span::styled(text, bold(color))]));
    }
    if let Some(checks) = &pr.checks {
        lines.push(field("checks", chips::checks_spans(checks, t)));
    }
    if pr.mergeable == Some(Mergeable::Conflicting) {
        lines.push(field(
            "merge",
            vec![Span::styled(
                "⚠ conflicts with the base branch",
                bold(t.warning),
            )],
        ));
    }
    if !pr.requested_reviewers.is_empty() {
        let names: Vec<String> = pr
            .requested_reviewers
            .iter()
            .map(|u| format!("@{}", u.login))
            .collect();
        lines.push(field(
            "reviewers",
            vec![Span::styled(names.join(", "), Style::new().fg(t.fg))],
        ));
    }
    if !pr.assignees.is_empty() {
        let names: Vec<String> = pr
            .assignees
            .iter()
            .map(|u| format!("@{}", u.login))
            .collect();
        lines.push(field(
            "assignees",
            vec![Span::styled(names.join(", "), Style::new().fg(t.fg))],
        ));
    }
    if !pr.labels.is_empty() {
        let mut spans = Vec::new();
        for label in &pr.labels {
            spans.push(chips::label_span(label, t));
            spans.push(Span::raw(" "));
        }
        lines.push(field("labels", spans));
    }
    lines
}

fn url(item: &FocusItem) -> &str {
    match &item.subject {
        FocusSubject::PullRequest(pr) => &pr.url,
        FocusSubject::Issue(issue) => &issue.url,
    }
}

impl Tab for FocusTab {
    fn title(&self) -> &'static str {
        "Focus"
    }

    fn count(&self, ctx: &Ctx) -> Option<usize> {
        let count: usize = self
            .shown_data(ctx)
            .filter_map(|d| d.focus.as_ref())
            .map(|f| f.value.len())
            .sum();
        Some(count)
    }

    fn mount(&mut self, ctx: &Ctx) {
        self.stop();
        let every = match ctx.prefs.refresh.focus_minutes {
            0 => None,
            minutes => Some(Duration::from_secs(u64::from(minutes) * 60)),
        };
        for forge in ctx.forges.iter() {
            self.accounts.entry(forge.account().clone()).or_default();
            if forge.focus().is_none() {
                continue;
            }
            self.tasks.push(sync::spawn_focus(
                Arc::clone(forge),
                ctx.cache.clone(),
                every,
                ctx.sender(),
            ));
            self.tasks.push(sync::spawn_unread(
                Arc::clone(forge),
                ctx.cache.clone(),
                ctx.prefs.refresh.notifications,
                ctx.sender(),
            ));
        }
        if self.appeared.is_none() {
            self.appeared = Some(Moment::now());
        }
    }

    fn command(&mut self, command: Command, ctx: &Ctx) -> Option<Action> {
        let rows = self.rows(ctx);
        let page = ListState::page(self.list_height) as isize;
        match command {
            Command::Down => self.list.move_by(1, &rows),
            Command::Up => self.list.move_by(-1, &rows),
            Command::Top => self.list.top(&rows),
            Command::Bottom => self.list.bottom(&rows),
            Command::PageDown => self.list.move_by(page, &rows),
            Command::PageUp => self.list.move_by(-page, &rows),
            Command::NextGroup => self.list.jump_group(true, &rows),
            Command::PrevGroup => self.list.jump_group(false, &rows),
            Command::Sort => {
                self.remember_selection(ctx);
                let next = match self.sort_order(ctx) {
                    SortOrder::Newest => SortOrder::Oldest,
                    SortOrder::Oldest => SortOrder::Newest,
                };
                self.sort = Some(next);
                self.restore_selection(ctx);
                return Some(Action::Toast(Toast::info(match next {
                    SortOrder::Newest => "newest first",
                    SortOrder::Oldest => "oldest first",
                })));
            }
            Command::ToggleGroups => {
                self.remember_selection(ctx);
                let grouped = !self.grouped(ctx);
                self.group = Some(grouped);
                self.restore_selection(ctx);
                return Some(Action::Toast(Toast::info(if grouped {
                    "grouped by what is asked of you"
                } else {
                    "one list"
                })));
            }
            Command::Open => {
                let (account, item) = self.selected_item(ctx)?;
                return Some(Action::ShowDetail(match item.subject {
                    FocusSubject::PullRequest(pr) => Detail::Pull {
                        account,
                        item: pr.reference.clone(),
                        summary: Some(Box::new(pr)),
                    },
                    FocusSubject::Issue(_) => Detail::Focus {
                        account,
                        item: Box::new(item),
                    },
                }));
            }
            Command::Browser => {
                let (_, item) = self.selected_item(ctx)?;
                return Some(Action::Toast(match browser::open(url(&item)) {
                    Ok(()) => Toast::success(format!("opened {}", item.subject.reference())),
                    Err(toast) => toast,
                }));
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
            self.remember_selection(ctx);
            return Some(Action::Redraw);
        }
        match key.code {
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Enter => {
                self.editing = false;
                // One match left: Enter opens it right away.
                if self.visible(ctx).len() == 1 {
                    self.list.top(&self.rows(ctx));
                    self.remember_selection(ctx);
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
        self.remember_selection(ctx);
        Some(Action::Redraw)
    }

    fn update(&mut self, action: &Action, ctx: &Ctx) {
        let Action::Data(event) = action else {
            return;
        };
        match event {
            DataEvent::Refreshing(account) => {
                self.accounts.entry(account.clone()).or_default().refreshing = true;
            }
            DataEvent::Focus { account, result } => {
                let data = self.accounts.entry(account.clone()).or_default();
                match result {
                    Ok(loaded) => {
                        if loaded.fresh {
                            data.error = None;
                            data.refreshing = false;
                        }
                        data.focus = Some(loaded.clone());
                    }
                    Err(e) => {
                        data.error = Some(e.clone());
                        data.refreshing = false;
                    }
                }
                self.restore_selection(ctx);
            }
            DataEvent::UnreadCount { account, result } => {
                let data = self.accounts.entry(account.clone()).or_default();
                match result {
                    Ok(loaded) => data.unread = Some(loaded.clone()),
                    Err(e) => data.error = Some(e.clone()),
                }
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame, area: TabArea, ctx: &Ctx) {
        let t = &ctx.theme;
        let show_filter = self.editing || !self.filter.is_empty();
        let [list_area, filter_area] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(u16::from(show_filter)),
        ])
        .areas(area.list);

        if show_filter {
            let mut spans = vec![
                Span::styled(" / ", bold(t.accent_alt)),
                Span::styled(self.filter.clone(), Style::new().fg(t.fg)),
            ];
            if self.editing {
                spans.push(Span::styled("▏", bold(t.accent_alt)));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), filter_area);
        }

        self.list_height = list_area.height;
        if self.reveal_pending {
            self.reveal_pending = false;
            self.restore_selection(ctx);
        }
        let rows = self.rows(ctx);
        if !rows.iter().any(Row::is_item) {
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
            let inner = Rect {
                x: preview.x + 1,
                width: preview.width.saturating_sub(2),
                ..preview
            };
            if let Some((account, item)) = self.selected_item(ctx) {
                frame.render_widget(
                    Paragraph::new(preview_lines(&account, &item, ctx)).wrap(Wrap { trim: false }),
                    inner,
                );
            }
        }
    }

    fn status(&self, ctx: &Ctx) -> Vec<Span<'static>> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let data: Vec<&AccountData> = self.shown_data(ctx).collect();
        let oldest = data
            .iter()
            .filter_map(|d| d.focus.as_ref())
            .min_by_key(|f| f.fetched_at);
        let refreshing = data.iter().any(|d| d.refreshing);
        let error = data.iter().find_map(|d| d.error.as_ref());
        let unread: usize = data
            .iter()
            .filter_map(|d| d.unread.as_ref())
            .map(|u| u.value)
            .sum();
        let need: usize = data
            .iter()
            .filter_map(|d| d.focus.as_ref())
            .map(|f| f.value.len())
            .sum();

        let mut spans = Vec::new();
        match (oldest, refreshing, error) {
            (Some(focus), true, _) => {
                spans.push(Span::styled(format!("{} ", spinner(ctx)), bold(t.accent)));
                spans.push(Span::styled(
                    format!("refreshing · updated {}", sync::ago(focus.fetched_at)),
                    muted,
                ));
            }
            (Some(focus), false, Some(e)) => spans.push(Span::styled(
                format!(
                    "! couldn't refresh ({e}) · data from {}",
                    sync::ago(focus.fetched_at)
                ),
                Style::new().fg(t.warning),
            )),
            (Some(focus), false, None) if !focus.fresh => spans.push(Span::styled(
                format!("cached · {}", sync::ago(focus.fetched_at)),
                muted,
            )),
            (Some(focus), false, None) => spans.push(Span::styled(
                format!("updated {}", sync::ago(focus.fetched_at)),
                muted,
            )),
            (None, ..) => {}
        }
        if oldest.is_some() {
            spans.push(Span::styled("  ·  ", Style::new().fg(t.border)));
            spans.push(Span::styled(format!("{need}"), bold(t.accent_alt)));
            spans.push(Span::styled(" need you", muted));
            if unread > 0 {
                spans.push(Span::styled("  ·  ", Style::new().fg(t.border)));
                spans.push(Span::styled(format!("{unread}"), bold(t.accent)));
                spans.push(Span::styled(" unread", muted));
            }
        }
        spans
    }

    fn hints(&self) -> &'static [(&'static [Command], &'static str)] {
        &[
            (&[Command::Down, Command::Up], "move"),
            (&[Command::Open], "open"),
            (&[Command::Browser], "browser"),
            (&[Command::NextGroup, Command::PrevGroup], "groups"),
            (&[Command::Filter], "filter"),
            (&[Command::Sort], "sort"),
        ]
    }

    fn jump_targets(&self) -> Vec<JumpTarget> {
        self.accounts
            .iter()
            .flat_map(|(account, data)| {
                data.focus
                    .iter()
                    .flat_map(|f| f.value.iter())
                    .map(move |item| JumpTarget {
                        account: account.clone(),
                        item: item.subject.reference().clone(),
                        title: item.subject.title().to_owned(),
                    })
            })
            .collect()
    }

    fn reveal(&mut self, account: &AccountId, item: &ItemRef) -> bool {
        self.filter.clear();
        self.editing = false;
        self.selected = Some((account.clone(), item.clone()));
        self.reveal_pending = true;
        true
    }

    fn is_animating(&self) -> bool {
        let entering = self.appeared.is_some_and(|at| at.within(ENTRANCE));
        let gliding = self.cursor.is_some_and(|c| c.is_running());
        let spinning = self
            .accounts
            .values()
            .any(|d| d.refreshing || (d.focus.is_none() && d.error.is_none()));
        entering || gliding || spinning
    }

    fn shown(&mut self) {
        self.appeared = Some(Moment::now());
    }
}

#[cfg(test)]
mod tests {
    use hit_core::Forges;
    use hit_forge_mock::{MockForge, MockOptions};
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::{
        Services,
        layout::{LayoutStrategy, TabsLayout},
    };

    fn render(tab: &mut FocusTab, ctx: &Ctx, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
        terminal
            .draw(|frame| {
                let regions = TabsLayout::default().regions(frame.area());
                tab.render(
                    frame,
                    TabArea {
                        list: regions.list,
                        preview: regions.preview,
                    },
                    ctx,
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn loaded_tab() -> (FocusTab, Ctx) {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let mut tab = FocusTab::new();
        tab.mount(&ctx);
        while tab.count(&ctx) == Some(0) {
            let action = rx.recv().await.unwrap();
            tab.update(&action, &ctx);
        }
        (tab, ctx)
    }

    #[tokio::test]
    async fn lists_items_with_reasons_and_preview() {
        let (mut tab, ctx) = loaded_tab().await;
        let screen = render(&mut tab, &ctx, 160);
        assert!(screen.contains("review requested"), "{screen}");
        assert!(screen.contains("CI failing"));
        // Preview of the first (most recent) item on the right.
        assert!(
            screen.contains("checks") || screen.contains("comments"),
            "{screen}"
        );
        let narrow = render(&mut tab, &ctx, 80);
        assert!(!narrow.contains("checks    "), "no preview when narrow");
    }

    #[tokio::test]
    async fn filter_narrows_and_esc_clears() {
        let (mut tab, ctx) = loaded_tab().await;
        let all = tab.visible(&ctx).len();
        tab.command(Command::Filter, &ctx);
        assert!(tab.captures_input());
        for c in "dark".chars() {
            tab.input(
                &Event::Key(crossterm::event::KeyEvent::from(KeyCode::Char(c))),
                &ctx,
            );
        }
        assert_eq!(tab.visible(&ctx).len(), 1);
        // A single match: Enter opens it directly.
        assert!(matches!(
            tab.input(&Event::Key(KeyCode::Enter.into()), &ctx),
            Some(Action::ShowDetail(_))
        ));
        assert!(!tab.captures_input());
        tab.command(Command::Back, &ctx);
        assert_eq!(tab.visible(&ctx).len(), all);
    }

    #[tokio::test]
    async fn open_and_navigation() {
        let (mut tab, ctx) = loaded_tab().await;
        tab.command(Command::Bottom, &ctx);
        let last = tab.rows(&ctx).len() - 1;
        assert_eq!(tab.list.selected, last);
        tab.command(Command::Top, &ctx);
        assert_eq!(tab.list.selected, 1, "row 0 is the first group's header");
        assert!(matches!(
            tab.command(Command::Open, &ctx),
            Some(Action::ShowDetail(
                Detail::Pull { .. } | Detail::Focus { .. }
            ))
        ));
        assert!(
            tab.status(&ctx)
                .iter()
                .any(|s| s.content.contains("need you"))
        );
    }

    #[tokio::test]
    async fn groups_in_priority_order_and_sorting() {
        let (mut tab, ctx) = loaded_tab().await;
        let groups: Vec<Group> = tab
            .slots(&ctx)
            .iter()
            .filter_map(|s| match s {
                Slot::Header(group, _) => Some(*group),
                Slot::Item(..) => None,
            })
            .collect();
        assert_eq!(groups, [Group::Review, Group::Mine, Group::Others]);
        let screen = render(&mut tab, &ctx, 160);
        assert!(screen.contains("Waiting for your review"), "{screen}");

        // Sorting keeps the selected item selected.
        tab.command(Command::Down, &ctx);
        let before = tab
            .selected_item(&ctx)
            .unwrap()
            .1
            .subject
            .reference()
            .clone();
        tab.command(Command::Sort, &ctx);
        assert_eq!(tab.sort_order(&ctx), SortOrder::Oldest);
        let after = tab
            .selected_item(&ctx)
            .unwrap()
            .1
            .subject
            .reference()
            .clone();
        assert_eq!(before, after);

        // Flat list: no headers.
        tab.command(Command::ToggleGroups, &ctx);
        assert!(tab.rows(&ctx).iter().all(Row::is_item));

        // Group jumps land on items.
        tab.command(Command::ToggleGroups, &ctx);
        tab.command(Command::Top, &ctx);
        tab.command(Command::NextGroup, &ctx);
        assert!(matches!(
            tab.slots(&ctx).get(tab.list.selected),
            Some(Slot::Item(..))
        ));
    }
}
