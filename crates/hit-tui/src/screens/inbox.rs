//! Inbox: every notification, with quick filters and marking read.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use crossterm::event::KeyCode;
use hit_config::MarkRead;
use hit_core::{
    AccountId, ForgeError,
    model::{Notification, NotificationId, NotificationReason, SubjectKind},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use super::{Tab, TabArea, focus::spinner};
use crate::{
    Action, Ctx, DataEvent, Event, Theme,
    action::{Detail, Toast},
    anim::{Moment, Tween},
    browser,
    keymap::{Command, typed_char},
    sync::{self, Loaded},
    ui::{
        chips::{self, bold},
        list::{ItemList, ListState, Row},
        popup::Popup,
    },
};

const ENTRANCE: Duration = Duration::from_millis(700);
const CURSOR_GLIDE: Duration = Duration::from_millis(90);

/// Whether a notification reason passes a reason filter.
type ReasonFilter = fn(&NotificationReason) -> bool;

/// Reason filters, in the order `f` cycles through them.
const REASONS: [(&str, ReasonFilter); 8] = [
    ("review requested", |r| {
        matches!(r, NotificationReason::ReviewRequested)
    }),
    ("mentions", |r| {
        matches!(
            r,
            NotificationReason::Mention | NotificationReason::TeamMention
        )
    }),
    ("assigned", |r| matches!(r, NotificationReason::Assign)),
    ("your threads", |r| matches!(r, NotificationReason::Author)),
    ("comments", |r| matches!(r, NotificationReason::Comment)),
    ("CI activity", |r| {
        matches!(r, NotificationReason::CiActivity)
    }),
    ("subscribed", |r| {
        matches!(r, NotificationReason::Subscribed)
    }),
    ("security", |r| {
        matches!(r, NotificationReason::SecurityAlert)
    }),
];

type Key = (AccountId, NotificationId);

#[derive(Default)]
struct AccountInbox {
    items: Option<Loaded<Arc<Vec<Notification>>>>,
    /// Which mode `items` was fetched in.
    unread_only: bool,
    error: Option<ForgeError>,
    refreshing: bool,
}

#[derive(Default)]
pub struct InboxTab {
    accounts: HashMap<AccountId, AccountInbox>,
    tasks: Vec<JoinHandle<()>>,
    unread_only: Option<bool>,
    participating: bool,
    reason: Option<usize>,
    filter: String,
    editing: bool,
    /// Marked read here (optimistically) since the last refresh.
    read: HashSet<Key>,
    /// Opened since entering the tab (for `mark_read = "leave"`).
    opened: HashSet<Key>,
    confirm_all: Option<Moment>,
    list: ListState,
    selected: Option<Key>,
    cursor: Option<Tween>,
    appeared: Option<Moment>,
    list_height: u16,
}

impl Drop for InboxTab {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Text and color for a notification reason.
pub fn reason_style(reason: &NotificationReason, t: &Theme) -> (String, Color) {
    use NotificationReason as R;
    match reason {
        R::ReviewRequested => ("review requested".into(), t.accent),
        R::Mention => ("mention".into(), t.info),
        R::TeamMention => ("team mention".into(), t.info),
        R::Assign => ("assigned".into(), t.highlight),
        R::Author => ("your thread".into(), t.fg),
        R::Comment => ("comment".into(), t.muted),
        R::StateChange => ("state change".into(), t.success),
        R::CiActivity => ("CI activity".into(), t.danger),
        R::Subscribed => ("subscribed".into(), t.muted),
        R::SecurityAlert => ("security alert".into(), t.danger),
        R::Other(other) => (other.replace('_', " "), t.muted),
    }
}

fn kind_icon(kind: &SubjectKind, t: &Theme) -> Span<'static> {
    let (icon, color) = match kind {
        SubjectKind::PullRequest => ("◈", t.success),
        SubjectKind::Issue => ("○", t.success),
        SubjectKind::Release => ("▲", t.accent_alt),
        SubjectKind::Commit => ("●", t.muted),
        SubjectKind::Discussion => ("◇", t.info),
        SubjectKind::CheckSuite => ("✓", t.warning),
        SubjectKind::Other(_) => ("!", t.danger),
    };
    Span::styled(icon, bold(color))
}

fn title_ref(n: &Notification) -> String {
    match &n.subject.item {
        Some(item) => item.to_string(),
        None => n.repo.to_string(),
    }
}

/// The detail of a notification (for the full-screen view).
pub fn notification_lines(account: &AccountId, n: &Notification, ctx: &Ctx) -> Vec<Line<'static>> {
    let t = &ctx.theme;
    let muted = Style::new().fg(t.muted);
    let (reason, color) = reason_style(&n.reason, t);
    let mut header = vec![
        kind_icon(&n.subject.kind, t),
        Span::styled(format!(" {}", title_ref(n)), bold(t.accent)),
    ];
    if ctx.multi_account() {
        header.push(Span::styled(
            format!("  @{}", account.login),
            bold(ctx.account_color(account)),
        ));
    }
    let field = |name: &str, value: Span<'static>| {
        Line::from(vec![Span::styled(format!("{name:<10}"), muted), value])
    };
    vec![
        Line::from(header),
        Line::from(Span::styled(n.subject.title.clone(), bold(t.fg))),
        Line::raw(""),
        field("reason", Span::styled(reason, bold(color))),
        field(
            "state",
            if n.unread {
                Span::styled("unread", bold(t.accent_alt))
            } else {
                Span::styled("read", Style::new().fg(t.muted))
            },
        ),
        field(
            "updated",
            Span::styled(sync::ago(n.updated_at), Style::new().fg(t.fg)),
        ),
        field(
            "link",
            Span::styled(n.subject.url.clone().unwrap_or_default(), muted),
        ),
    ]
}

impl InboxTab {
    pub fn new() -> Self {
        Self::default()
    }

    fn stop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    fn unread_only(&self, ctx: &Ctx) -> bool {
        self.unread_only.unwrap_or(ctx.prefs.inbox.unread_only)
    }

    fn is_read(&self, account: &AccountId, n: &Notification) -> bool {
        !n.unread || self.read.contains(&(account.clone(), n.id.clone()))
    }

    fn filters_active(&self) -> bool {
        self.participating || self.reason.is_some() || !self.filter.is_empty()
    }

    fn visible<'a>(&'a self, ctx: &Ctx) -> Vec<(&'a AccountId, &'a Notification)> {
        let unread_only = self.unread_only(ctx);
        let needle = self.filter.to_lowercase();
        let reason = self.reason.and_then(|i| REASONS.get(i)).map(|(_, f)| *f);
        let mut items: Vec<_> = self
            .accounts
            .iter()
            .filter(|(account, data)| ctx.shows(account) && data.unread_only == unread_only)
            .flat_map(|(account, data)| {
                data.items
                    .iter()
                    .flat_map(|l| l.value.iter())
                    .map(move |n| (account, n))
            })
            .filter(|(_, n)| !self.participating || n.reason.is_participating())
            .filter(|(_, n)| reason.is_none_or(|f| f(&n.reason)))
            .filter(|(_, n)| {
                needle.is_empty()
                    || n.subject.title.to_lowercase().contains(&needle)
                    || title_ref(n).to_lowercase().contains(&needle)
            })
            .collect();
        items.sort_by_key(|(_, n)| std::cmp::Reverse(n.updated_at));
        items
    }

    fn rows(&self, ctx: &Ctx) -> Vec<Row> {
        self.visible(ctx)
            .into_iter()
            .map(|(account, n)| self.row(account, n, ctx))
            .collect()
    }

    fn row(&self, account: &AccountId, n: &Notification, ctx: &Ctx) -> Row {
        let t = &ctx.theme;
        let read = self.is_read(account, n);
        let mut top = Vec::new();
        if let Some(badge) = chips::account_badge(account, ctx) {
            top.push(badge);
        }
        top.push(if read {
            Span::styled("○ ", Style::new().fg(t.muted))
        } else {
            Span::styled("● ", bold(t.accent_alt))
        });
        top.push(kind_icon(&n.subject.kind, t));
        top.push(Span::styled(
            format!(" {} ", title_ref(n)),
            Style::new().fg(t.muted),
        ));
        top.push(Span::styled(n.subject.title.clone(), bold(t.fg)));

        let (reason, color) = reason_style(&n.reason, t);
        let mut bottom = vec![Span::styled(reason, bold(color))];
        if read && n.unread {
            bottom.push(Span::styled("  ✓ read", bold(t.success)));
        }
        bottom.push(Span::styled(
            format!("  · {}", sync::ago(n.updated_at)),
            Style::new().fg(t.muted),
        ));
        let mut row = Row::item(Line::from(top), Line::from(bottom), color);
        if let Row::Item { dimmed, .. } = &mut row {
            *dimmed = read && n.unread;
        }
        row
    }

    fn selected(&self, ctx: &Ctx) -> Option<(AccountId, Notification)> {
        self.visible(ctx)
            .get(self.list.selected)
            .map(|(a, n)| ((*a).clone(), (*n).clone()))
    }

    fn remember_selection(&mut self, ctx: &Ctx) {
        if let Some((account, n)) = self.selected(ctx) {
            self.selected = Some((account, n.id));
        }
    }

    fn restore_selection(&mut self, ctx: &Ctx) {
        let Some((account, id)) = &self.selected else {
            return;
        };
        if let Some(index) = self
            .visible(ctx)
            .iter()
            .position(|(a, n)| *a == account && &n.id == id)
        {
            self.list.selected = index;
        }
    }

    /// Optimistically marks `keys` read and sends exactly those to the forge.
    fn mark(&mut self, keys: Vec<Key>, ctx: &Ctx) {
        let mut by_account: HashMap<AccountId, Vec<NotificationId>> = HashMap::new();
        for (account, id) in keys {
            if self.read.insert((account.clone(), id.clone())) {
                by_account.entry(account).or_default().push(id);
            }
        }
        for forge in ctx.forges.iter() {
            if let Some(ids) = by_account.remove(forge.account()) {
                sync::spawn_mark_read(Arc::clone(forge), ids, ctx.sender());
            }
        }
    }

    /// Marks an opened notification per `[inbox] mark_read`.
    fn on_opened(&mut self, account: &AccountId, n: &Notification, ctx: &Ctx) {
        if !n.unread {
            return;
        }
        let key = (account.clone(), n.id.clone());
        match ctx.prefs.inbox.mark_read {
            MarkRead::Open => self.mark(vec![key], ctx),
            MarkRead::Leave => {
                self.opened.insert(key);
            }
            MarkRead::Manual => {}
        }
    }

    fn render_filters(&self, frame: &mut Frame, area: Rect, shown: usize, ctx: &Ctx) {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let chip = |on: bool, text: String, color: Color| {
            if on {
                Span::styled(text, bold(color))
            } else {
                Span::styled(text, muted)
            }
        };
        let unread = self.unread_only(ctx);
        let mut spans = vec![
            Span::raw(" "),
            chip(
                true,
                if unread { "✉ unread" } else { "✉ all" }.to_owned(),
                t.accent_alt,
            ),
            Span::raw("   "),
            chip(
                self.participating,
                if self.participating {
                    "● for you"
                } else {
                    "○ everything"
                }
                .to_owned(),
                t.accent,
            ),
            Span::raw("   "),
            chip(
                self.reason.is_some(),
                match self.reason.and_then(|i| REASONS.get(i)) {
                    Some((name, _)) => format!("◆ {name}"),
                    None => "◇ all reasons".to_owned(),
                },
                t.info,
            ),
        ];
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
            Paragraph::new(Span::styled(format!("{shown} shown "), muted))
                .alignment(Alignment::Right),
            area,
        );
    }

    fn render_empty(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let unread_only = self.unread_only(ctx);
        let data: Vec<&AccountInbox> = self
            .accounts
            .iter()
            .filter(|(a, d)| ctx.shows(a) && d.unread_only == unread_only)
            .map(|(_, d)| d)
            .collect();
        let loading =
            data.is_empty() || data.iter().any(|d| d.items.is_none() && d.error.is_none());
        let error = data.iter().find_map(|d| d.error.as_ref());
        let lines = if loading {
            vec![Line::from(vec![
                Span::styled(spinner(ctx), bold(t.accent)),
                Span::styled(" loading notifications…", Style::new().fg(t.muted)),
            ])]
        } else if let Some(e) = error.filter(|_| data.iter().all(|d| d.items.is_none())) {
            vec![
                Line::from(Span::styled("couldn't load notifications", bold(t.danger))),
                Line::from(Span::styled(e.to_string(), Style::new().fg(t.muted))),
            ]
        } else if self.filters_active() {
            vec![
                Line::from(Span::styled("nothing matches these filters", bold(t.fg))),
                Line::from(Span::styled(
                    "p, f and / change them · esc clears the text",
                    Style::new().fg(t.muted),
                )),
            ]
        } else {
            vec![
                t.gradient_line_owned("inbox zero ✨".to_owned(), 12, ctx.time() * 0.2),
                Line::from(Span::styled(
                    if unread_only {
                        "no unread notifications · u shows all"
                    } else {
                        "no notifications"
                    },
                    Style::new().fg(t.muted),
                )),
            ]
        };
        let [middle] = Layout::vertical([Constraint::Length(lines.len() as u16)])
            .flex(Flex::Center)
            .areas(area);
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), middle);
    }

    fn render_confirm(&self, frame: &mut Frame, area: Rect, opened: Moment, ctx: &Ctx) {
        let t = &ctx.theme;
        let count = self
            .visible(ctx)
            .iter()
            .filter(|(a, n)| !self.is_read(a, n))
            .count();
        let inner = Popup {
            title: "mark all read",
            footer: Some("y yes · any other key cancels"),
            width: 52,
            height: 5,
            border: t.warning,
            theme: t,
            opened: ctx.animations.then_some(opened),
        }
        .render(frame, area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Mark ", Style::new().fg(t.fg)),
                Span::styled(count.to_string(), bold(t.accent_alt)),
                Span::styled(" shown notifications as read?", Style::new().fg(t.fg)),
            ]))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
            Rect {
                y: inner.y + inner.height.saturating_sub(1) / 2,
                height: 1,
                ..inner
            },
        );
    }
}

impl Tab for InboxTab {
    fn title(&self) -> &'static str {
        "Inbox"
    }

    fn count(&self, ctx: &Ctx) -> Option<usize> {
        let unread_only = self.unread_only(ctx);
        let count = self
            .accounts
            .iter()
            .filter(|(a, d)| ctx.shows(a) && d.unread_only == unread_only)
            .flat_map(|(a, d)| {
                d.items
                    .iter()
                    .flat_map(|l| l.value.iter())
                    .map(move |n| (a, n))
            })
            .filter(|(a, n)| !self.is_read(a, n))
            .count();
        Some(count)
    }

    fn mount(&mut self, ctx: &Ctx) {
        self.stop();
        let unread_only = self.unread_only(ctx);
        for forge in ctx.forges.iter() {
            let data = self.accounts.entry(forge.account().clone()).or_default();
            if data.unread_only != unread_only {
                *data = AccountInbox {
                    unread_only,
                    ..AccountInbox::default()
                };
            }
            data.refreshing = data.items.is_some();
            self.tasks.push(sync::spawn_inbox(
                Arc::clone(forge),
                ctx.cache.clone(),
                unread_only,
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
            Command::Open => {
                let (account, n) = self.selected(ctx)?;
                self.on_opened(&account, &n, ctx);
                return Some(Action::ShowDetail(Detail::Notification {
                    account,
                    notification: Box::new(n),
                }));
            }
            Command::Browser => {
                let (account, n) = self.selected(ctx)?;
                let url = n.subject.url.clone().unwrap_or_default();
                let result = browser::open(&url);
                if result.is_ok() {
                    self.on_opened(&account, &n, ctx);
                }
                return Some(Action::Toast(match result {
                    Ok(()) => Toast::success(format!("opened {}", title_ref(&n))),
                    Err(toast) => toast,
                }));
            }
            Command::MarkRead => {
                let (account, n) = self.selected(ctx)?;
                if self.is_read(&account, &n) {
                    return Some(Action::Toast(Toast::info("already read")));
                }
                self.mark(vec![(account, n.id)], ctx);
                // Move on to the next one, like archiving mail.
                self.list.move_by(1, &rows);
            }
            Command::MarkAllRead => {
                if self.visible(ctx).iter().any(|(a, n)| !self.is_read(a, n)) {
                    self.confirm_all = Some(Moment::now());
                } else {
                    return Some(Action::Toast(Toast::info("nothing unread here")));
                }
            }
            Command::ToggleUnread => {
                self.unread_only = Some(!self.unread_only(ctx));
                self.read.clear();
                self.list = ListState::default();
                self.appeared = Some(Moment::now());
                self.mount(ctx);
            }
            Command::Participating => {
                self.participating = !self.participating;
                self.list.top(&self.rows(ctx));
            }
            Command::CycleReason => {
                self.reason = match self.reason {
                    None => Some(0),
                    Some(i) if i + 1 < REASONS.len() => Some(i + 1),
                    Some(_) => None,
                };
                self.list.top(&self.rows(ctx));
            }
            Command::Filter => self.editing = true,
            Command::Back if self.filters_active() => {
                self.filter.clear();
                self.participating = false;
                self.reason = None;
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
        self.editing || self.confirm_all.is_some()
    }

    fn input(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let Event::Key(key) = event else {
            return None;
        };
        if self.confirm_all.take().is_some() {
            if typed_char(key) == Some('y') {
                let visible = self.visible(ctx);
                let keys: Vec<Key> = visible
                    .iter()
                    .filter(|(a, n)| !self.is_read(a, n))
                    .map(|(a, n)| ((*a).clone(), n.id.clone()))
                    .collect();
                // Only what is on screen: never a blanket "mark all", which
                // would also hit new or not-yet-loaded notifications.
                let count = keys.len();
                self.mark(keys, ctx);
                return Some(Action::Toast(Toast::success(format!(
                    "marking {count} read…"
                ))));
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
        let Action::Data(event) = action else {
            return;
        };
        match event {
            DataEvent::Notifications {
                account,
                unread_only,
                result,
            } => {
                if *unread_only != self.unread_only(ctx) {
                    return;
                }
                let data = self.accounts.entry(account.clone()).or_default();
                data.unread_only = *unread_only;
                match result {
                    Ok(loaded) => {
                        if loaded.fresh {
                            data.error = None;
                            data.refreshing = false;
                            // The forge now knows what was marked; stop
                            // overriding it locally.
                            let fresh_ids: HashSet<&NotificationId> = loaded
                                .value
                                .iter()
                                .filter(|n| n.unread)
                                .map(|n| &n.id)
                                .collect();
                            self.read
                                .retain(|(a, id)| a != account || fresh_ids.contains(id));
                        }
                        data.items = Some(loaded.clone());
                    }
                    Err(e) => {
                        data.error = Some(e.clone());
                        data.refreshing = false;
                    }
                }
                self.restore_selection(ctx);
            }
            DataEvent::MarkedRead {
                account,
                ids,
                result: Err(e),
            } => {
                for id in ids {
                    self.read.remove(&(account.clone(), id.clone()));
                }
                let message = match e {
                    ForgeError::ReadOnly(_) => {
                        "read-only: marking read is off (set allow_mark_read = true under [auth])"
                            .to_owned()
                    }
                    other => format!("couldn't mark read: {other}"),
                };
                let _ = ctx.sender().send(Action::Toast(Toast::error(message)));
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame, area: TabArea, ctx: &Ctx) {
        let t = &ctx.theme;
        let [bar, list_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area.list);
        let rows = self.rows(ctx);
        self.render_filters(frame, Rect { height: 1, ..bar }, rows.len(), ctx);
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
            if let Some((account, n)) = self.selected(ctx) {
                frame.render_widget(
                    Paragraph::new(notification_lines(&account, &n, ctx))
                        .wrap(Wrap { trim: false }),
                    Rect {
                        x: preview.x + 1,
                        width: preview.width.saturating_sub(2),
                        ..preview
                    },
                );
            }
        }

        if let Some(opened) = self.confirm_all {
            let full = area.preview.map_or(area.list, |p| area.list.union(p));
            self.render_confirm(frame, full, opened, ctx);
        }
    }

    fn status(&self, ctx: &Ctx) -> Vec<Span<'static>> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let unread_only = self.unread_only(ctx);
        let data: Vec<&AccountInbox> = self
            .accounts
            .iter()
            .filter(|(a, d)| ctx.shows(a) && d.unread_only == unread_only)
            .map(|(_, d)| d)
            .collect();
        let oldest = data
            .iter()
            .filter_map(|d| d.items.as_ref())
            .min_by_key(|l| l.fetched_at);
        let mut spans = Vec::new();
        if let Some(loaded) = oldest {
            if data.iter().any(|d| d.refreshing) {
                spans.push(Span::styled(format!("{} ", spinner(ctx)), bold(t.accent)));
                spans.push(Span::styled(
                    format!("refreshing · updated {}", sync::ago(loaded.fetched_at)),
                    muted,
                ));
            } else if let Some(e) = data.iter().find_map(|d| d.error.as_ref()) {
                spans.push(Span::styled(
                    format!(
                        "! couldn't refresh ({e}) · data from {}",
                        sync::ago(loaded.fetched_at)
                    ),
                    Style::new().fg(t.warning),
                ));
            } else if loaded.fresh {
                spans.push(Span::styled(
                    format!("updated {}", sync::ago(loaded.fetched_at)),
                    muted,
                ));
            } else {
                spans.push(Span::styled(
                    format!("cached · {}", sync::ago(loaded.fetched_at)),
                    muted,
                ));
            }
            spans.push(Span::styled("  ·  ", Style::new().fg(t.border)));
            spans.push(Span::styled(
                self.count(ctx).unwrap_or(0).to_string(),
                bold(t.accent_alt),
            ));
            spans.push(Span::styled(" unread", muted));
            let mode = match ctx.prefs.inbox.mark_read {
                MarkRead::Manual => None,
                MarkRead::Open => Some("marks read when opened"),
                MarkRead::Leave => Some("marks opened ones read when you leave"),
            };
            if let Some(mode) = mode {
                spans.push(Span::styled(format!("  ·  {mode}"), muted));
            }
        }
        spans
    }

    fn hints(&self) -> &'static [(&'static [Command], &'static str)] {
        &[
            (&[Command::Down, Command::Up], "move"),
            (&[Command::Open], "open"),
            (&[Command::MarkRead], "read"),
            (&[Command::MarkAllRead], "all read"),
            (&[Command::ToggleUnread], "unread/all"),
            (&[Command::CycleReason], "reason"),
            (&[Command::Participating], "for you"),
            (&[Command::Filter], "filter"),
        ]
    }

    fn is_animating(&self) -> bool {
        let entering = self.appeared.is_some_and(|at| at.within(ENTRANCE));
        let gliding = self.cursor.is_some_and(|c| c.is_running());
        let popping = Popup::is_animating(self.confirm_all);
        let spinning = self
            .accounts
            .values()
            .any(|d| d.refreshing || (d.items.is_none() && d.error.is_none()));
        entering || gliding || popping || spinning
    }

    fn shown(&mut self) {
        self.appeared = Some(Moment::now());
    }

    fn hidden(&mut self, ctx: &Ctx) {
        if ctx.prefs.inbox.mark_read == MarkRead::Leave && !self.opened.is_empty() {
            let keys: Vec<Key> = self.opened.drain().collect();
            self.mark(keys, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use hit_core::Forges;
    use hit_forge_mock::{MockForge, MockOptions};
    use ratatui::{Terminal, backend::TestBackend};
    use tokio::sync::mpsc;

    use super::*;
    use crate::{
        Services,
        layout::{LayoutStrategy, TabsLayout},
    };

    fn render(tab: &mut InboxTab, ctx: &Ctx) -> String {
        let mut terminal = Terminal::new(TestBackend::new(160, 34)).unwrap();
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

    async fn loaded(failure_rate: f32) -> (InboxTab, Ctx, mpsc::UnboundedReceiver<Action>) {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions {
            failure_rate,
            ..MockOptions::instant()
        })));
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let mut tab = InboxTab::new();
        // Load with a healthy forge first, then swap in the failure rate.
        tab.mount(&ctx);
        loop {
            let action = rx.recv().await.unwrap();
            let done = matches!(action, Action::Data(DataEvent::Notifications { .. }));
            tab.update(&action, &ctx);
            if done {
                break;
            }
        }
        (tab, ctx, rx)
    }

    fn press(tab: &mut InboxTab, ctx: &Ctx, command: Command) -> Option<Action> {
        tab.command(command, ctx)
    }

    #[tokio::test]
    async fn shows_unread_with_filter_chips() {
        let (mut tab, ctx, _rx) = loaded(0.0).await;
        let screen = render(&mut tab, &ctx);
        assert!(screen.contains("✉ unread"), "{screen}");
        assert!(screen.contains("◇ all reasons"));
        assert!(screen.contains("review requested"));
        let unread = tab.count(&ctx).unwrap();
        assert!(unread > 0);
        assert_eq!(tab.visible(&ctx).len(), unread);
    }

    #[tokio::test]
    async fn filters_narrow_the_list() {
        let (mut tab, ctx, _rx) = loaded(0.0).await;
        let all = tab.visible(&ctx).len();
        press(&mut tab, &ctx, Command::CycleReason);
        let reviews = tab.visible(&ctx).len();
        assert!(reviews > 0 && reviews < all);
        assert!(
            tab.visible(&ctx)
                .iter()
                .all(|(_, n)| n.reason == NotificationReason::ReviewRequested)
        );
        press(&mut tab, &ctx, Command::Back);
        press(&mut tab, &ctx, Command::Participating);
        assert!(
            tab.visible(&ctx)
                .iter()
                .all(|(_, n)| n.reason.is_participating())
        );
    }

    #[tokio::test]
    async fn mark_read_is_optimistic_and_reported() {
        let (mut tab, ctx, mut rx) = loaded(0.0).await;
        let before = tab.count(&ctx).unwrap();
        press(&mut tab, &ctx, Command::MarkRead);
        assert_eq!(tab.count(&ctx).unwrap(), before - 1, "updates immediately");
        let screen = render(&mut tab, &ctx);
        assert!(screen.contains("✓ read"));
        // The forge confirms.
        loop {
            if let Action::Data(event @ DataEvent::MarkedRead { .. }) = rx.recv().await.unwrap() {
                assert!(matches!(
                    event,
                    DataEvent::MarkedRead { result: Ok(()), .. }
                ));
                break;
            }
        }
    }

    #[tokio::test]
    async fn failed_mark_read_is_undone() {
        let (mut tab, ctx, _rx) = loaded(0.0).await;
        let before = tab.count(&ctx).unwrap();
        let (account, n) = tab.selected(&ctx).unwrap();
        press(&mut tab, &ctx, Command::MarkRead);
        tab.update(
            &Action::Data(DataEvent::MarkedRead {
                account,
                ids: vec![n.id],
                result: Err(ForgeError::ReadOnly("PATCH request".into())),
            }),
            &ctx,
        );
        assert_eq!(tab.count(&ctx).unwrap(), before);
    }

    #[tokio::test]
    async fn mark_all_only_marks_what_is_shown() {
        let (mut tab, ctx, mut rx) = loaded(0.0).await;
        press(&mut tab, &ctx, Command::CycleReason); // review requests only
        let shown = tab.visible(&ctx).len();
        let forge = Arc::clone(ctx.forges.iter().next().unwrap());
        let unread_before = forge
            .notifications()
            .unwrap()
            .list(
                &hit_core::query::NotificationQuery {
                    unread_only: true,
                    ..Default::default()
                },
                &hit_core::query::PageRequest::first(100),
            )
            .await
            .unwrap()
            .items
            .len();

        press(&mut tab, &ctx, Command::MarkAllRead);
        tab.input(&Event::Key(KeyCode::Char('y').into()), &ctx);
        loop {
            if let Action::Data(DataEvent::MarkedRead { ids, result, .. }) =
                rx.recv().await.unwrap()
            {
                assert!(result.is_ok());
                assert_eq!(ids.len(), shown);
                break;
            }
        }
        let unread_after = forge
            .notifications()
            .unwrap()
            .list(
                &hit_core::query::NotificationQuery {
                    unread_only: true,
                    ..Default::default()
                },
                &hit_core::query::PageRequest::first(100),
            )
            .await
            .unwrap()
            .items
            .len();
        assert_eq!(unread_after, unread_before - shown, "others stay unread");
    }

    #[tokio::test]
    async fn mark_all_asks_first() {
        let (mut tab, ctx, _rx) = loaded(0.0).await;
        press(&mut tab, &ctx, Command::MarkAllRead);
        assert!(tab.captures_input());
        let screen = render(&mut tab, &ctx);
        assert!(screen.contains("as read?"), "{screen}");
        // Anything but y cancels.
        tab.input(&Event::Key(KeyCode::Char('n').into()), &ctx);
        assert!(!tab.captures_input());
        assert!(tab.count(&ctx).unwrap() > 0);
        press(&mut tab, &ctx, Command::MarkAllRead);
        tab.input(&Event::Key(KeyCode::Char('y').into()), &ctx);
        assert_eq!(tab.count(&ctx).unwrap(), 0);
    }
}
