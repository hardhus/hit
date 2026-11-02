//! Issues: ready-made views (involved, assigned, created, mentioned, closed)
//! across all shown accounts, with a label picker.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};

use crossterm::event::KeyCode;
use hit_core::{
    AccountId, ForgeError, ItemRef,
    model::Issue,
    query::{IssueQuery, StateFilter},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use super::{JumpTarget, Tab, TabArea, focus::spinner, issue_fields};
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
        picker::{Picker, PickerEvent, PickerItem},
    },
};

const ENTRANCE: Duration = Duration::from_millis(700);
const CURSOR_GLIDE: Duration = Duration::from_millis(90);

/// The ready-made views `f` cycles through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Involved,
    Assigned,
    Created,
    Mentioned,
    Closed,
}

const VIEWS: [View; 5] = [
    View::Involved,
    View::Assigned,
    View::Created,
    View::Mentioned,
    View::Closed,
];

impl View {
    fn key(self) -> &'static str {
        match self {
            Self::Involved => "involved",
            Self::Assigned => "assigned",
            Self::Created => "created",
            Self::Mentioned => "mentioned",
            Self::Closed => "closed",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Involved => "involved",
            Self::Assigned => "assigned",
            Self::Created => "created",
            Self::Mentioned => "mentioned",
            Self::Closed => "recently closed",
        }
    }

    fn query(self, me: &str) -> IssueQuery {
        let mut query = IssueQuery::default();
        match self {
            Self::Involved => {}
            Self::Assigned => query.assignee = Some(me.to_owned()),
            Self::Created => query.author = Some(me.to_owned()),
            Self::Mentioned => query.mentioned = Some(me.to_owned()),
            Self::Closed => query.state = StateFilter::Closed,
        }
        query
    }
}

#[derive(Default)]
struct Data {
    items: Option<Loaded<Arc<Vec<Issue>>>>,
    error: Option<ForgeError>,
}

#[derive(Default)]
pub struct IssuesTab {
    /// Per account and view.
    data: HashMap<(AccountId, &'static str), Data>,
    tasks: Vec<JoinHandle<()>>,
    view: usize,
    filter: String,
    editing: bool,
    /// Shown issues carry at least one of these.
    labels: BTreeSet<String>,
    picker: Option<Picker>,
    list: ListState,
    selected: Option<(AccountId, ItemRef)>,
    reveal_pending: bool,
    cursor: Option<Tween>,
    appeared: Option<Moment>,
    list_height: u16,
}

impl Drop for IssuesTab {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `@author · bug docs · 3 comments · 2h ago` for list rows.
pub fn issue_status_spans(issue: &Issue, ctx: &Ctx) -> Vec<Span<'static>> {
    let t = &ctx.theme;
    let muted = Style::new().fg(t.muted);
    let sep = || Span::styled(" · ", Style::new().fg(t.border));
    let mut spans = vec![Span::styled(format!("@{}", issue.author.login), muted)];
    if !issue.labels.is_empty() {
        spans.push(sep());
        for (index, label) in issue.labels.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(chips::label_span(label, t));
        }
    }
    if !issue.assignees.is_empty() {
        let names: Vec<String> = issue
            .assignees
            .iter()
            .map(|u| format!("→ @{}", u.login))
            .collect();
        spans.push(sep());
        spans.push(Span::styled(names.join(" "), muted));
    }
    if issue.comments > 0 {
        spans.push(sep());
        spans.push(Span::styled(format!("{} comments", issue.comments), muted));
    }
    spans.push(sep());
    spans.push(Span::styled(sync::ago(issue.updated_at), muted));
    spans
}

impl IssuesTab {
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

    /// Everything in the current view, before the label and text filters.
    fn loaded<'a>(&'a self, ctx: &'a Ctx) -> impl Iterator<Item = (&'a AccountId, &'a Issue)> {
        self.shown_data(ctx).flat_map(|(account, data)| {
            data.items
                .iter()
                .flat_map(|l| l.value.iter())
                .map(move |issue| (account, issue))
        })
    }

    fn visible<'a>(&'a self, ctx: &'a Ctx) -> Vec<(&'a AccountId, &'a Issue)> {
        let needle = self.filter.to_lowercase();
        let mut items: Vec<_> = self
            .loaded(ctx)
            .filter(|(_, issue)| {
                self.labels.is_empty() || issue.labels.iter().any(|l| self.labels.contains(&l.name))
            })
            .filter(|(_, issue)| {
                needle.is_empty()
                    || issue.title.to_lowercase().contains(&needle)
                    || issue.reference.to_string().to_lowercase().contains(&needle)
                    || issue.author.login.to_lowercase().contains(&needle)
                    || issue
                        .labels
                        .iter()
                        .any(|l| l.name.to_lowercase().contains(&needle))
            })
            .collect();
        items.sort_by_key(|(_, issue)| std::cmp::Reverse(issue.updated_at));
        items
    }

    /// The labels on loaded issues, most used first. Picked labels stay in
    /// the list even when no loaded issue has them.
    fn label_items(&self, ctx: &Ctx) -> Vec<PickerItem> {
        let mut seen: HashMap<String, (hit_core::model::Label, usize)> = HashMap::new();
        for (_, issue) in self.loaded(ctx) {
            for label in &issue.labels {
                seen.entry(label.name.clone())
                    .or_insert_with(|| (label.clone(), 0))
                    .1 += 1;
            }
        }
        let mut items: Vec<PickerItem> = seen
            .into_values()
            .map(|(label, count)| PickerItem {
                picked: self.labels.contains(&label.name),
                key: label.name.clone(),
                label: chips::label_span(&label, &ctx.theme),
                count,
            })
            .collect();
        for name in &self.labels {
            if !items.iter().any(|i| &i.key == name) {
                items.push(PickerItem {
                    key: name.clone(),
                    label: Span::styled(format!(" {name} "), Style::new().fg(ctx.theme.muted)),
                    count: 0,
                    picked: true,
                });
            }
        }
        items.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
        items
    }

    fn rows(&self, ctx: &Ctx) -> Vec<Row> {
        let t = &ctx.theme;
        self.visible(ctx)
            .into_iter()
            .map(|(account, issue)| {
                let mut top = Vec::new();
                if let Some(badge) = chips::account_badge(account, ctx) {
                    top.push(badge);
                }
                top.push(chips::issue_icon(issue, t));
                top.push(Span::styled(
                    format!(" {} ", issue.reference),
                    Style::new().fg(t.muted),
                ));
                top.push(Span::styled(issue.title.clone(), bold(t.fg)));
                // The first label's color says what kind of issue it is.
                let accent = issue
                    .labels
                    .iter()
                    .find_map(|l| l.color)
                    .map_or(t.accent, |c| Color::Rgb(c.0, c.1, c.2));
                Row::item(
                    Line::from(top),
                    Line::from(issue_status_spans(issue, ctx)),
                    accent,
                )
            })
            .collect()
    }

    fn selected_issue(&self, ctx: &Ctx) -> Option<(AccountId, Issue)> {
        self.visible(ctx)
            .get(self.list.selected)
            .map(|(a, issue)| ((*a).clone(), (*issue).clone()))
    }

    fn remember_selection(&mut self, ctx: &Ctx) {
        if let Some((account, issue)) = self.selected_issue(ctx) {
            self.selected = Some((account, issue.reference));
        }
    }

    fn restore_selection(&mut self, ctx: &Ctx) {
        let Some((account, item)) = &self.selected else {
            return;
        };
        if let Some(index) = self
            .visible(ctx)
            .iter()
            .position(|(a, issue)| *a == account && &issue.reference == item)
        {
            self.list.selected = index;
        }
    }

    fn render_bar(&self, frame: &mut Frame, area: Rect, shown: usize, ctx: &Ctx) {
        let t = &ctx.theme;
        let mut spans = vec![Span::raw(" ")];
        for (index, view) in VIEWS.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw("  "));
            }
            if index == self.view {
                spans.push(Span::styled(
                    format!("◆ {}", view.title()),
                    bold(t.accent_alt),
                ));
            } else {
                spans.push(Span::styled(view.title(), Style::new().fg(t.muted)));
            }
        }
        if !self.labels.is_empty() {
            spans.push(Span::raw("   "));
            spans.push(Span::styled("# ", bold(t.accent_alt)));
            let names: Vec<&str> = self.labels.iter().map(String::as_str).collect();
            spans.push(Span::styled(names.join(" · "), Style::new().fg(t.fg)));
        }
        if self.editing || !self.filter.is_empty() {
            spans.push(Span::raw("   "));
            spans.push(Span::styled("/ ", bold(t.accent_alt)));
            spans.push(Span::styled(self.filter.clone(), Style::new().fg(t.fg)));
            if self.editing {
                spans.push(Span::styled("▏", bold(t.accent_alt)));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{shown} shown "),
                Style::new().fg(t.muted),
            ))
            .alignment(Alignment::Right),
            area,
        );
    }

    fn render_empty(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let data: Vec<&Data> = self.shown_data(ctx).map(|(_, d)| d).collect();
        let loading =
            data.is_empty() || data.iter().any(|d| d.items.is_none() && d.error.is_none());
        let lines = if loading {
            vec![Line::from(vec![
                Span::styled(spinner(ctx), bold(t.accent)),
                Span::styled(" loading issues…", muted),
            ])]
        } else if let Some(e) = data
            .iter()
            .find_map(|d| d.error.as_ref())
            .filter(|_| data.iter().all(|d| d.items.is_none()))
        {
            vec![
                Line::from(Span::styled("couldn't load issues", bold(t.danger))),
                Line::from(Span::styled(e.to_string(), muted)),
            ]
        } else if !self.filter.is_empty() || !self.labels.is_empty() {
            vec![Line::from(Span::styled(
                "nothing matches · esc clears the filters",
                muted,
            ))]
        } else {
            vec![
                Line::from(Span::styled("no issues here", bold(t.fg))),
                Line::from(Span::styled(
                    "f switches between involved, assigned, created, mentioned and closed",
                    muted,
                )),
            ]
        };
        let [middle] = Layout::vertical([Constraint::Length(lines.len() as u16)])
            .flex(Flex::Center)
            .areas(area);
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), middle);
    }

    fn render_preview(&self, frame: &mut Frame, preview: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
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
        let Some((account, issue)) = self.selected_issue(ctx) else {
            return;
        };
        let mut header = vec![
            chips::issue_icon(&issue, t),
            Span::styled(format!(" {}", issue.reference), bold(t.accent)),
        ];
        if ctx.multi_account() {
            header.push(Span::styled(
                format!("  @{}", account.login),
                bold(ctx.account_color(&account)),
            ));
        }
        let mut lines = vec![
            Line::from(header),
            Line::from(Span::styled(issue.title.clone(), bold(t.fg))),
            Line::raw(""),
        ];
        lines.extend(issue_fields(&issue, ctx));
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "↵ open · description and comments",
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

    fn toggle_label(&mut self, name: String, ctx: &Ctx) {
        self.remember_selection(ctx);
        if !self.labels.remove(&name) {
            self.labels.insert(name);
        }
        self.list.top(&self.rows(ctx));
        self.restore_selection(ctx);
    }
}

impl Tab for IssuesTab {
    fn title(&self) -> &'static str {
        "Issues"
    }

    fn count(&self, ctx: &Ctx) -> Option<usize> {
        Some(self.visible(ctx).len()).filter(|n| *n > 0)
    }

    fn mount(&mut self, ctx: &Ctx) {
        self.stop();
        let view = self.current();
        for forge in ctx.forges.iter() {
            let account = forge.account().clone();
            self.data.entry((account.clone(), view.key())).or_default();
            self.tasks.push(sync::spawn_issues(
                Arc::clone(forge),
                ctx.cache.clone(),
                view.key(),
                view.query(&account.login),
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
            Command::Open => {
                let (account, issue) = self.selected_issue(ctx)?;
                return Some(Action::ShowDetail(Detail::Issue {
                    account,
                    item: issue.reference.clone(),
                    summary: Some(Box::new(issue)),
                }));
            }
            Command::Browser => {
                let (_, issue) = self.selected_issue(ctx)?;
                return Some(Action::Toast(match browser::open(&issue.url) {
                    Ok(()) => Toast::success(format!("opened {}", issue.reference)),
                    Err(toast) => toast,
                }));
            }
            Command::CycleReason => {
                self.view = (self.view + 1) % VIEWS.len();
                self.list = ListState::default();
                self.appeared = Some(Moment::now());
                self.mount(ctx);
            }
            Command::Labels => self.picker = Some(Picker::new()),
            Command::Filter => self.editing = true,
            Command::Back if !self.filter.is_empty() => {
                self.filter.clear();
                self.restore_selection(ctx);
            }
            Command::Back if !self.labels.is_empty() => {
                self.labels.clear();
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
        self.editing || self.picker.is_some()
    }

    fn input(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let Event::Key(key) = event else {
            return None;
        };
        if self.picker.is_some() {
            let items = self.label_items(ctx);
            let event = self.picker.as_mut()?.input(key, &items);
            match event {
                PickerEvent::Toggle(name) => self.toggle_label(name, ctx),
                PickerEvent::ClearAll => {
                    self.labels.clear();
                    self.restore_selection(ctx);
                }
                PickerEvent::Close => self.picker = None,
                PickerEvent::Changed | PickerEvent::Ignored => {}
            }
            return Some(Action::Redraw);
        }
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
        if let Action::Data(DataEvent::Issues {
            account,
            view,
            result,
        }) = action
        {
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
        self.render_bar(frame, Rect { height: 1, ..bar }, rows.len(), ctx);
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
            self.render_preview(frame, preview, ctx);
        }

        if let Some(picker) = &self.picker {
            let full = area.preview.map_or(area.list, |p| area.list.union(p));
            let items = self.label_items(ctx);
            picker.render(frame, full, "labels", &items, t, ctx.animations);
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
                    format!("  ·  ! couldn't refresh ({e})"),
                    Style::new().fg(t.warning),
                ));
            }
        }
        spans
    }

    fn hints(&self) -> &'static [(&'static [Command], &'static str)] {
        &[
            (&[Command::Down, Command::Up], "move"),
            (&[Command::Open], "open"),
            (&[Command::CycleReason], "view"),
            (&[Command::Labels], "labels"),
            (&[Command::Filter], "filter"),
            (&[Command::Browser], "browser"),
        ]
    }

    fn jump_targets(&self) -> Vec<JumpTarget> {
        self.data
            .iter()
            .flat_map(|((account, _), data)| {
                data.items
                    .iter()
                    .flat_map(|l| l.value.iter())
                    .map(move |issue| JumpTarget {
                        account: account.clone(),
                        item: issue.reference.clone(),
                        title: issue.title.clone(),
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
                    .is_some_and(|l| l.value.iter().any(|issue| &issue.reference == item))
        });
        if known {
            self.filter.clear();
            self.labels.clear();
            self.selected = Some((account.clone(), item.clone()));
            self.reveal_pending = true;
        }
        known
    }

    fn is_animating(&self) -> bool {
        let entering = self.appeared.is_some_and(|at| at.within(ENTRANCE));
        let gliding = self.cursor.is_some_and(|c| c.is_running());
        let popping = self.picker.as_ref().is_some_and(Picker::is_animating);
        let loading = self
            .data
            .values()
            .any(|d| d.items.is_none() && d.error.is_none());
        entering || gliding || popping || loading
    }

    fn shown(&mut self) {
        self.appeared = Some(Moment::now());
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEvent, KeyModifiers};
    use hit_core::Forges;
    use hit_forge_mock::{MockForge, MockOptions};

    use super::*;
    use crate::Services;

    async fn loaded() -> (IssuesTab, Ctx, tokio::sync::mpsc::UnboundedReceiver<Action>) {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let mut tab = IssuesTab::new();
        tab.mount(&ctx);
        while tab.visible(&ctx).is_empty() {
            let action = rx.recv().await.unwrap();
            tab.update(&action, &ctx);
        }
        (tab, ctx, rx)
    }

    async fn next_view(
        tab: &mut IssuesTab,
        ctx: &Ctx,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<Action>,
    ) {
        tab.command(Command::CycleReason, ctx);
        while tab.visible(ctx).is_empty() {
            let action = rx.recv().await.unwrap();
            tab.update(&action, ctx);
        }
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[tokio::test]
    async fn views_switch() {
        let (mut tab, ctx, mut rx) = loaded().await;
        assert!(tab.visible(&ctx).iter().all(|(_, i)| i.is_open()));

        next_view(&mut tab, &ctx, &mut rx).await; // assigned
        assert!(
            tab.visible(&ctx)
                .iter()
                .all(|(_, i)| i.assignees.iter().any(|u| u.login == "you"))
        );
        next_view(&mut tab, &ctx, &mut rx).await; // created
        assert!(
            tab.visible(&ctx)
                .iter()
                .all(|(_, i)| i.author.login == "you")
        );
        next_view(&mut tab, &ctx, &mut rx).await; // mentioned
        assert_eq!(tab.visible(&ctx).len(), 1);
        next_view(&mut tab, &ctx, &mut rx).await; // closed
        assert!(tab.visible(&ctx).iter().all(|(_, i)| !i.is_open()));
    }

    #[tokio::test]
    async fn label_picker_filters_by_any_picked_label() {
        let (mut tab, ctx, _rx) = loaded().await;
        let all = tab.visible(&ctx).len();

        tab.command(Command::Labels, &ctx);
        assert!(tab.captures_input());
        let items = tab.label_items(&ctx);
        assert_eq!(items[0].key, "bug", "most used first");

        // Search "docs", pick it.
        for c in "docs".chars() {
            tab.input(&key(KeyCode::Char(c)), &ctx);
        }
        tab.input(&key(KeyCode::Enter), &ctx);
        assert!(
            tab.visible(&ctx)
                .iter()
                .all(|(_, i)| i.labels.iter().any(|l| l.name == "docs"))
        );
        let docs = tab.visible(&ctx).len();
        assert!(docs > 0 && docs < all);

        // Add "bug" too: issues with either.
        tab.input(&key(KeyCode::Esc), &ctx); // clears the search
        tab.input(&key(KeyCode::Enter), &ctx); // "bug" is first
        assert!(tab.visible(&ctx).len() > docs);

        tab.input(&key(KeyCode::Esc), &ctx); // closes
        assert!(!tab.captures_input());

        // Esc outside the picker clears the labels.
        tab.command(Command::Back, &ctx);
        assert_eq!(tab.visible(&ctx).len(), all);
    }

    #[tokio::test]
    async fn enter_opens_the_issue() {
        let (mut tab, ctx, _rx) = loaded().await;
        assert!(matches!(
            tab.command(Command::Open, &ctx),
            Some(Action::ShowDetail(Detail::Issue {
                summary: Some(_),
                ..
            }))
        ));
    }
}
