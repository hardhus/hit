//! Repositories: the way into a repository's scope. The one hit was opened
//! on comes first, then the most recently pushed.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use crossterm::event::KeyCode;
use hit_core::{AccountId, ForgeError, model::Repo, query::Scope};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use tokio::task::JoinHandle;

use super::{Tab, TabArea, focus::spinner};
use crate::{
    Action, Ctx, DataEvent, Event,
    action::Toast,
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

#[derive(Default)]
struct Data {
    items: Option<Loaded<Arc<Vec<Repo>>>>,
    error: Option<ForgeError>,
}

#[derive(Default)]
pub struct ReposTab {
    accounts: HashMap<AccountId, Data>,
    tasks: Vec<JoinHandle<()>>,
    filter: String,
    editing: bool,
    list: ListState,
    cursor: Option<Tween>,
    appeared: Option<Moment>,
    list_height: u16,
}

impl Drop for ReposTab {
    fn drop(&mut self) {
        self.stop();
    }
}

impl ReposTab {
    pub fn new() -> Self {
        Self::default()
    }

    fn stop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    fn shown_data<'a>(&'a self, ctx: &'a Ctx) -> impl Iterator<Item = &'a Data> + 'a {
        self.accounts
            .iter()
            .filter(|(account, _)| ctx.shows(account))
            .map(|(_, data)| data)
    }

    /// Shown repositories, each once even if several accounts can see it.
    fn visible<'a>(&'a self, ctx: &'a Ctx) -> Vec<&'a Repo> {
        let needle = self.filter.to_lowercase();
        let mut seen = HashSet::new();
        let mut repos: Vec<&Repo> = self
            .shown_data(ctx)
            .filter_map(|data| data.items.as_ref())
            .flat_map(|loaded| loaded.value.iter())
            .filter(|repo| seen.insert(&repo.reference))
            .filter(|repo| {
                needle.is_empty()
                    || repo.reference.to_string().to_lowercase().contains(&needle)
                    || repo
                        .description
                        .as_ref()
                        .is_some_and(|d| d.to_lowercase().contains(&needle))
            })
            .collect();
        repos.sort_by_key(|repo| {
            (
                ctx.home_repo.as_ref() != Some(&repo.reference),
                repo.archived,
                Reverse(repo.pushed_at),
            )
        });
        repos
    }

    fn row(repo: &Repo, ctx: &Ctx) -> Row {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let sep = || Span::styled(" · ", Style::new().fg(t.border));
        let color = chips::repo_color(&repo.reference, t);
        let current = ctx.scope.repo() == Some(&repo.reference);
        let home = ctx.home_repo.as_ref() == Some(&repo.reference);

        let mut top = vec![Span::styled(if home { "⌂ " } else { "▣ " }, bold(color))];
        top.push(Span::styled(repo.reference.to_string(), bold(color)));
        for (flag, text) in [
            (repo.private, "private"),
            (repo.fork, "fork"),
            (repo.archived, "archived"),
            (current, "showing"),
        ] {
            if flag {
                top.push(Span::styled(format!("  {text}"), muted));
            }
        }

        let mut bottom = Vec::new();
        if let (Some(prs), Some(issues)) = (repo.open_prs, repo.open_issues) {
            bottom.push(Span::styled(
                format!("◈ {prs}"),
                bold(if prs > 0 { t.success } else { t.muted }),
            ));
            bottom.push(Span::raw("  "));
            bottom.push(Span::styled(
                format!("○ {issues}"),
                bold(if issues > 0 { t.info } else { t.muted }),
            ));
            bottom.push(sep());
        }
        if let Some(pushed) = repo.pushed_at {
            bottom.push(Span::styled(format!("pushed {}", sync::ago(pushed)), muted));
        }
        if let Some(description) = &repo.description {
            if !bottom.is_empty() {
                bottom.push(sep());
            }
            bottom.push(Span::styled(description.clone(), muted));
        }
        Row::item(Line::from(top), Line::from(bottom), color)
    }

    fn rows(&self, ctx: &Ctx) -> Vec<Row> {
        self.visible(ctx)
            .into_iter()
            .map(|repo| Self::row(repo, ctx))
            .collect()
    }

    fn selected_repo(&self, ctx: &Ctx) -> Option<Repo> {
        self.visible(ctx)
            .get(self.list.selected)
            .map(|repo| (*repo).clone())
    }

    fn render_empty(&self, frame: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = &ctx.theme;
        let data: Vec<&Data> = self.shown_data(ctx).collect();
        let loading =
            data.is_empty() || data.iter().any(|d| d.items.is_none() && d.error.is_none());
        let lines = if loading {
            vec![Line::from(vec![
                Span::styled(spinner(ctx), bold(t.accent)),
                Span::styled(" loading repositories…", Style::new().fg(t.muted)),
            ])]
        } else if let Some(e) = data
            .iter()
            .find_map(|d| d.error.as_ref())
            .filter(|_| data.iter().all(|d| d.items.is_none()))
        {
            vec![
                Line::from(Span::styled("couldn't load repositories", bold(t.danger))),
                Line::from(Span::styled(e.to_string(), Style::new().fg(t.muted))),
            ]
        } else if !self.filter.is_empty() {
            vec![Line::from(Span::styled(
                "nothing matches · esc clears the filter",
                Style::new().fg(t.muted),
            ))]
        } else {
            vec![Line::from(Span::styled("no repositories here", bold(t.fg)))]
        };
        let [middle] = Layout::vertical([Constraint::Length(lines.len() as u16)])
            .flex(Flex::Center)
            .areas(area);
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), middle);
    }

    fn preview_lines(repo: &Repo, ctx: &Ctx) -> Vec<Line<'static>> {
        let t = &ctx.theme;
        let muted = Style::new().fg(t.muted);
        let field = |name: &str, value: String| {
            Line::from(vec![
                Span::styled(format!("{name:<10}"), muted),
                Span::styled(value, Style::new().fg(t.fg)),
            ])
        };
        let mut lines = vec![Line::from(Span::styled(
            repo.reference.to_string(),
            bold(chips::repo_color(&repo.reference, t)),
        ))];
        if let Some(description) = &repo.description {
            lines.push(Line::from(Span::styled(
                description.clone(),
                Style::new().fg(t.fg),
            )));
        }
        lines.push(Line::raw(""));
        if let Some(prs) = repo.open_prs {
            lines.push(field("pulls", format!("{prs} open")));
        }
        if let Some(issues) = repo.open_issues {
            lines.push(field("issues", format!("{issues} open")));
        }
        lines.push(field(
            "access",
            if repo.private { "private" } else { "public" }.to_owned(),
        ));
        if let Some(branch) = &repo.default_branch {
            lines.push(field("branch", branch.clone()));
        }
        if let Some(pushed) = repo.pushed_at {
            lines.push(field("pushed", sync::ago(pushed)));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "↵ show only this repository",
            Style::new().fg(t.muted),
        ));
        lines
    }
}

impl Tab for ReposTab {
    fn title(&self) -> &'static str {
        "Repos"
    }

    fn count(&self, ctx: &Ctx) -> Option<usize> {
        Some(self.visible(ctx).len()).filter(|n| *n > 0)
    }

    fn mount(&mut self, ctx: &Ctx) {
        self.stop();
        for forge in ctx.forges.iter() {
            self.accounts.entry(forge.account().clone()).or_default();
            self.tasks.push(sync::spawn_repos(
                Arc::clone(forge),
                ctx.cache.clone(),
                ctx.home_repo.clone(),
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
                let repo = self.selected_repo(ctx)?;
                return Some(Action::SetScope(Scope::Repo(repo.reference)));
            }
            Command::Browser => {
                let repo = self.selected_repo(ctx)?;
                return Some(Action::Toast(match browser::open(&repo.url) {
                    Ok(()) => Toast::success(format!("opened {}", repo.reference)),
                    Err(toast) => toast,
                }));
            }
            Command::Filter => self.editing = true,
            Command::Back if !self.filter.is_empty() => {
                self.filter.clear();
                self.list.top(&self.rows(ctx));
            }
            Command::Refresh => {
                self.mount(ctx);
                return Some(Action::Toast(Toast::info("refreshing…")));
            }
            _ => return None,
        }
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

    fn update(&mut self, action: &Action, _ctx: &Ctx) {
        if let Action::Data(DataEvent::Repos { account, result }) = action {
            let data = self.accounts.entry(account.clone()).or_default();
            match result {
                Ok(loaded) => {
                    if loaded.fresh {
                        data.error = None;
                    }
                    data.items = Some(loaded.clone());
                }
                Err(e) => data.error = Some(e.clone()),
            }
        }
    }

    fn render(&mut self, frame: &mut Frame, area: TabArea, ctx: &Ctx) {
        let t = &ctx.theme;
        let [bar, list_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area.list);
        let rows = self.rows(ctx);
        let mut spans = vec![Span::styled(" your repositories", bold(t.accent_alt))];
        if self.editing || !self.filter.is_empty() {
            spans.push(Span::raw("   "));
            spans.push(Span::styled("/ ", bold(t.accent_alt)));
            spans.push(Span::styled(self.filter.clone(), Style::new().fg(t.fg)));
            if self.editing {
                spans.push(Span::styled("▏", bold(t.accent_alt)));
            }
        }
        let bar = Rect { height: 1, ..bar };
        frame.render_widget(Paragraph::new(Line::from(spans)), bar);
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("{} shown ", rows.len()),
                Style::new().fg(t.muted),
            ))
            .alignment(Alignment::Right),
            bar,
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
            if let Some(repo) = self.selected_repo(ctx) {
                frame.render_widget(
                    Paragraph::new(Self::preview_lines(&repo, ctx)).wrap(Wrap { trim: false }),
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
            .filter_map(|d| d.items.as_ref())
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
            if let Some(e) = self.shown_data(ctx).find_map(|d| d.error.as_ref()) {
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
            (&[Command::Open], "enter"),
            (&[Command::Filter], "filter"),
            (&[Command::Browser], "browser"),
        ]
    }

    fn is_animating(&self) -> bool {
        let entering = self.appeared.is_some_and(|at| at.within(ENTRANCE));
        let gliding = self.cursor.is_some_and(|c| c.is_running());
        let loading = self
            .accounts
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
    use hit_core::{Forges, Host, RepoRef};
    use hit_forge_mock::{MockForge, MockOptions};

    use super::*;
    use crate::Services;

    async fn loaded(
        repo: Option<RepoRef>,
    ) -> (ReposTab, Ctx, tokio::sync::mpsc::UnboundedReceiver<Action>) {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        let (ctx, mut rx) = Ctx::for_tests(Services {
            forges,
            repo,
            ..Services::default()
        });
        let mut tab = ReposTab::new();
        tab.mount(&ctx);
        while tab.visible(&ctx).is_empty() {
            let action = rx.recv().await.unwrap();
            tab.update(&action, &ctx);
        }
        (tab, ctx, rx)
    }

    #[tokio::test]
    async fn recently_pushed_first_with_open_counts() {
        let (tab, ctx, _rx) = loaded(None).await;
        let repos = tab.visible(&ctx);
        assert!(repos.is_sorted_by_key(|r| Reverse(r.pushed_at)));
        assert!(
            repos
                .iter()
                .all(|r| r.open_prs.is_some() && r.open_issues.is_some())
        );
    }

    #[tokio::test]
    async fn the_home_repository_comes_first() {
        let home = RepoRef::new(Host::new(hit_forge_mock::DEMO_HOST), "oss", "termpaint");
        let (tab, ctx, _rx) = loaded(Some(home.clone())).await;
        assert_eq!(tab.visible(&ctx)[0].reference, home);
    }

    #[tokio::test]
    async fn enter_scopes_to_the_selected_repository() {
        let (mut tab, ctx, _rx) = loaded(None).await;
        tab.filter = "termpaint".to_owned();
        let Some(Action::SetScope(Scope::Repo(repo))) = tab.command(Command::Open, &ctx) else {
            panic!("expected a scope change");
        };
        assert_eq!(repo.to_string(), "oss/termpaint");
    }
}
