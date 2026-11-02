//! An issue, full screen: summary on top, then the description and comments.

use std::{sync::Arc, time::Duration};

use hit_core::{
    AccountId, ForgeError, ItemRef,
    model::{Issue, IssueDetail},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use super::thread;
use crate::{
    Action, Ctx, DataEvent, Event, View,
    action::Toast,
    anim::Moment,
    browser,
    keymap::{Command, Scope},
    screens::{issue_fields, spinner},
    sync::{self, Loaded},
    ui::chips::{self, bold},
};

const FADE: Duration = Duration::from_millis(160);

pub struct IssueView {
    account: AccountId,
    item: ItemRef,
    summary: Option<Issue>,
    detail: Option<Loaded<Arc<IssueDetail>>>,
    error: Option<ForgeError>,
    task: Option<JoinHandle<()>>,
    scroll: u16,
    opened: Moment,
}

impl IssueView {
    pub fn new(account: AccountId, item: ItemRef, summary: Option<Issue>) -> Self {
        Self {
            account,
            item,
            summary,
            detail: None,
            error: None,
            task: None,
            scroll: 0,
            opened: Moment::now(),
        }
    }

    fn issue(&self) -> Option<&Issue> {
        self.detail
            .as_ref()
            .map(|d| &d.value.issue)
            .or(self.summary.as_ref())
    }

    fn header(&self, ctx: &Ctx) -> Vec<Line<'static>> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let Some(issue) = self.issue() else {
            return vec![Line::from(vec![
                Span::styled(spinner(ctx), bold(t.accent)),
                Span::styled(format!(" loading {}…", self.item), muted),
            ])];
        };
        let mut meta = vec![
            chips::issue_icon(issue, t),
            Span::styled(format!(" {}", issue.reference), bold(t.accent)),
            Span::raw("  "),
            chips::issue_state_span(issue, t),
            Span::styled(
                format!(
                    "  · @{} opened {} · {} comments",
                    issue.author.login,
                    sync::ago(issue.created_at),
                    issue.comments
                ),
                muted,
            ),
        ];
        if ctx.multi_account() {
            meta.push(Span::styled(
                format!("  @{}", self.account.login),
                bold(ctx.account_color(&self.account)),
            ));
        }
        let mut labels = Vec::new();
        for label in &issue.labels {
            labels.push(chips::label_span(label, t));
            labels.push(Span::raw(" "));
        }
        vec![
            Line::from(Span::styled(issue.title.clone(), bold(t.fg))),
            Line::from(meta),
            Line::from(labels),
        ]
    }

    fn hints(&self, ctx: &Ctx) -> Line<'static> {
        crate::ui::hints::line(
            &[
                (&[Command::Down, Command::Up], "scroll"),
                (&[Command::Browser], "browser"),
                (&[Command::Refresh], "refresh"),
                (&[Command::Back], "back"),
            ],
            &ctx.keymap,
            &ctx.theme,
        )
    }
}

impl Drop for IssueView {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl View for IssueView {
    fn mount(&mut self, ctx: &Ctx) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let Some(forge) = ctx.forges.by_account(&self.account) else {
            self.error = Some(ForgeError::NotFound);
            return;
        };
        self.task = Some(sync::spawn_issue(
            Arc::clone(forge),
            ctx.cache.clone(),
            self.item.clone(),
            ctx.sender(),
        ));
    }

    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let Event::Key(key) = event else {
            return None;
        };
        match ctx.keymap.lookup(key, &[Scope::List, Scope::Global])? {
            Command::Back => return Some(Action::Back),
            Command::Quit => return Some(Action::Quit),
            Command::Down => self.scroll = self.scroll.saturating_add(1),
            Command::Up => self.scroll = self.scroll.saturating_sub(1),
            Command::PageDown => self.scroll = self.scroll.saturating_add(15),
            Command::PageUp => self.scroll = self.scroll.saturating_sub(15),
            Command::Top => self.scroll = 0,
            Command::Bottom => self.scroll = u16::MAX,
            Command::Browser => {
                let url = self.issue().map(|i| i.url.clone())?;
                return Some(Action::Toast(match browser::open(&url) {
                    Ok(()) => Toast::success(format!("opened {}", self.item)),
                    Err(toast) => toast,
                }));
            }
            Command::Refresh => {
                self.mount(ctx);
                return Some(Action::Toast(Toast::info("refreshing…")));
            }
            _ => return None,
        }
        Some(Action::Redraw)
    }

    fn update(&mut self, action: &Action, _ctx: &Ctx) -> Option<Action> {
        if let Action::Data(DataEvent::IssueDetail { item, result }) = action
            && *item == self.item
        {
            match result {
                Ok(loaded) => {
                    self.detail = Some(loaded.clone());
                    self.error = None;
                }
                Err(e) => self.error = Some(e.clone()),
            }
        }
        None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.accent))
            .style(t.base())
            .title(Span::styled(format!(" {} ", self.item), bold(t.accent)));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let inner = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };
        let [header, body, hints] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        frame.render_widget(
            Paragraph::new(self.header(ctx)).wrap(Wrap { trim: true }),
            header,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                "─".repeat(usize::from(body.width)),
                Style::new().fg(t.border),
            )),
            Rect { height: 1, ..body },
        );
        let body = Rect {
            y: body.y + 2,
            height: body.height.saturating_sub(2),
            ..body
        };
        let lines = match &self.detail {
            Some(d) => thread::lines(
                &d.value.body,
                &d.value.comments,
                d.value.earlier_comments,
                &[],
                &[],
                ctx,
            ),
            None => thread::placeholder(self.error.as_ref(), ctx),
        };
        let side = self.issue().map(|i| issue_fields(i, ctx));
        let dim = ctx.animations && self.opened.within(FADE);
        thread::render(frame, body, lines, side, &mut self.scroll, dim, ctx);
        frame.render_widget(Paragraph::new(self.hints(ctx)), hints);
    }

    fn is_animating(&self) -> bool {
        self.opened.within(FADE) || (self.detail.is_none() && self.error.is_none())
    }
}

#[cfg(test)]
mod tests {
    use hit_core::{Forges, Host, RepoRef};
    use hit_forge_mock::{MockForge, MockOptions};
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::Services;

    fn render(view: &mut IssueView, ctx: &Ctx, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| view.render(frame, frame.area(), ctx))
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn description_comments_and_fields() {
        let forge = Arc::new(MockForge::demo(MockOptions::instant()));
        let account = hit_core::Forge::account(forge.as_ref()).clone();
        let mut forges = Forges::new();
        forges.add(forge);
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });
        let item = ItemRef::issue(RepoRef::new(Host::new("demo.hit"), "acme", "rocket"), 57);
        let mut view = IssueView::new(account, item, None);
        view.mount(&ctx);
        while view.detail.is_none() {
            let action = rx.recv().await.unwrap();
            view.update(&action, &ctx);
        }
        let screen = render(&mut view, &ctx, 150, 30);
        assert!(screen.contains("Uploads fail intermittently"), "{screen}");
        assert!(screen.contains("storage migration"), "body: {screen}");
        assert!(screen.contains("@you commented"), "{screen}");
        assert!(
            screen.contains("Details") && screen.contains("assignees"),
            "{screen}"
        );

        // Narrow: no side panel; scrolling stops at the end.
        view.scroll = u16::MAX;
        render(&mut view, &ctx, 80, 30);
        assert!(view.scroll < 50);
    }
}
