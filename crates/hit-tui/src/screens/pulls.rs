//! Pull requests: ready-made views (involved, mine, review, closed) across
//! all shown accounts.

use std::{collections::HashMap, sync::Arc, time::Duration};

use crossterm::event::KeyCode;
use hit_core::{
    AccountId, ForgeError, ItemRef,
    model::{CheckState, PrState, PullRequest, ReviewDecision},
    query::{PrQuery, StateFilter},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use super::{JumpTarget, Tab, TabArea, focus::spinner, pr_fields};
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

const ENTRANCE: Duration = Duration::from_millis(700);
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

    fn title(self) -> &'static str {
        match self {
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
}

#[derive(Default)]
struct Data {
    items: Option<Loaded<Arc<Vec<PullRequest>>>>,
    error: Option<ForgeError>,
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
        spans.push(Span::styled(format!("{} comments", pr.comments), muted));
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
            .filter(|(_, pr)| {
                needle.is_empty()
                    || pr.title.to_lowercase().contains(&needle)
                    || pr.reference.to_string().to_lowercase().contains(&needle)
                    || pr.author.login.to_lowercase().contains(&needle)
            })
            .collect();
        items.sort_by_key(|(_, pr)| std::cmp::Reverse(pr.updated_at));
        items
    }

    fn rows(&self, ctx: &Ctx) -> Vec<Row> {
        let t = &ctx.theme;
        self.visible(ctx)
            .into_iter()
            .map(|(account, pr)| {
                let mut top = Vec::new();
                if let Some(badge) = chips::account_badge(account, ctx) {
                    top.push(badge);
                }
                top.push(pr_icon(pr, ctx));
                top.push(Span::styled(
                    format!(" {} ", pr.reference),
                    Style::new().fg(t.muted),
                ));
                top.push(Span::styled(pr.title.clone(), bold(t.fg)));
                let accent = match pr.checks.map(|c| c.state) {
                    Some(CheckState::Failure) => t.danger,
                    _ if pr.review_decision == Some(ReviewDecision::ChangesRequested) => t.warning,
                    _ if pr.review_decision == Some(ReviewDecision::Approved) => t.success,
                    _ => t.accent,
                };
                Row::item(
                    Line::from(top),
                    Line::from(pr_status_spans(pr, ctx)),
                    accent,
                )
            })
            .collect()
    }

    fn selected_pr(&self, ctx: &Ctx) -> Option<(AccountId, PullRequest)> {
        self.visible(ctx)
            .get(self.list.selected)
            .map(|(a, pr)| ((*a).clone(), (*pr).clone()))
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
        if let Some(index) = self
            .visible(ctx)
            .iter()
            .position(|(a, pr)| *a == account && &pr.reference == item)
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
                Line::from(Span::styled(e.to_string(), Style::new().fg(t.muted))),
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
        for forge in ctx.forges.iter() {
            let account = forge.account().clone();
            self.data.entry((account.clone(), view.key())).or_default();
            self.tasks.push(sync::spawn_pulls(
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
                let (account, pr) = self.selected_pr(ctx)?;
                return Some(Action::ShowDetail(Detail::Pull {
                    account,
                    item: pr.reference.clone(),
                    summary: Some(Box::new(pr)),
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
        if let Action::Data(DataEvent::Pulls {
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
