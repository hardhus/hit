use std::{sync::Arc, time::Duration};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hit_config::Config;
use ratatui::{Frame, widgets::Block};
use tokio::{sync::mpsc, time::MissedTickBehavior};

use crate::{
    Action, Ctx, DataEvent, Event, Services, TuiError, View,
    action::{Detail, Toast},
    browser,
    event::EventSource,
    external_editor,
    keymap::Keymap,
    shell::Shell,
    sync,
    terminal::{self, Term, TerminalGuard},
    ui::toast::Toasts,
    views::{
        ActionMenu, ComposeView, DetailView, IssueView, LoginView, MergeView, PullView,
        SettingsView, Welcome,
    },
};

/// One step of [`screens`] and what the screen showed after it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Screen {
    pub step: String,
    pub text: String,
}

/// Runs the real app with no terminal: presses `steps` in order and returns the
/// screen after each, once nothing has happened for `quiet`. A step is a key
/// (`j`, `enter`, `ctrl+s`), `=text` to type it, or `wait:5` for five seconds.
/// Keys do what they do in hit, writing included; links are only announced.
pub async fn screens(
    config: &Config,
    services: Services,
    steps: &[String],
    width: u16,
    height: u16,
    quiet: Duration,
) -> Result<Vec<Screen>, String> {
    browser::rehearse();
    let mut config = config.clone();
    config.ui.animations = false;
    let (keymap, _) = Keymap::from_config(&config.keys);
    let mut app = App::new(&config, services, keymap);
    if app.ctx.forges.is_empty() {
        app.push(Box::new(Welcome::new(Vec::new())));
    } else {
        app.push(Box::new(Shell::new(Vec::new())));
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
        .map_err(|e| e.to_string())?;
    let mut shots = Vec::with_capacity(steps.len() + 1);
    app.settle(quiet).await;
    shots.push(app.shot("start", &mut terminal)?);
    for step in steps {
        if let Some(seconds) = step.strip_prefix("wait:") {
            let seconds: u64 = seconds
                .parse()
                .map_err(|_| format!("{step}: not a number of seconds"))?;
            app.settle(Duration::from_secs(seconds)).await;
        } else if let Some(text) = step.strip_prefix('=') {
            for c in text.chars() {
                let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
                app.on_event(&Event::Key(key));
            }
        } else {
            let key = crate::keymap::Key::parse(step).map_err(|e| e.to_string())?;
            app.on_event(&Event::Key(key.to_event()));
        }
        if app.editing.take().is_some() {
            return Err(format!("{step}: the editor needs a real terminal"));
        }
        app.settle(quiet).await;
        shots.push(app.shot(step, &mut terminal)?);
        if app.quit {
            break;
        }
    }
    Ok(shots)
}

/// Runs the TUI until the user quits or a shutdown signal arrives.
pub async fn run(
    config: &Config,
    mut warnings: Vec<String>,
    services: Services,
) -> Result<(), TuiError> {
    let (keymap, key_warnings) = Keymap::from_config(&config.keys);
    for warning in &key_warnings {
        tracing::warn!(%warning, "keys");
    }
    warnings.extend(key_warnings);
    let mut app = App::new(config, services, keymap);
    // Signed in: straight to the main screen. Otherwise the empty state.
    if app.ctx.forges.is_empty() {
        app.push(Box::new(Welcome::new(warnings)));
    } else {
        app.push(Box::new(Shell::new(warnings)));
    }
    app.run().await
}

/// However busy a screen stays, a step of [`screens`] ends after this.
const SETTLE_LIMIT: Duration = Duration::from_secs(60);

pub struct App {
    views: Vec<Box<dyn View>>,
    ctx: Ctx,
    actions: mpsc::UnboundedReceiver<Action>,
    frame_interval: Duration,
    dirty: bool,
    quit: bool,
    /// For views that have no toasts of their own (the shell does).
    toasts: Toasts,
    /// Text waiting for the user's editor, between the action and the loop.
    editing: Option<String>,
}

impl App {
    pub fn new(config: &Config, services: Services, keymap: Keymap) -> Self {
        let (tx, actions) = mpsc::unbounded_channel();
        Self {
            views: Vec::new(),
            ctx: Ctx::new(config, services, keymap, tx),
            actions,
            frame_interval: frame_interval(config),
            dirty: true,
            quit: false,
            toasts: Toasts::default(),
            editing: None,
        }
    }

    /// Pushes a view and mounts it. Must be called inside the tokio runtime,
    /// as views may start background fetches when mounted.
    pub fn push(&mut self, mut view: Box<dyn View>) {
        view.mount(&self.ctx);
        self.views.push(view);
        self.dirty = true;
    }

    pub async fn run(mut self) -> Result<(), TuiError> {
        let mut guard = TerminalGuard::enter()?;
        let mut input = EventSource::spawn();
        let result = self.event_loop(guard.terminal(), &mut input).await;
        input.stop().await;
        let restored = guard.restore();
        result.and(restored)
    }

    async fn event_loop(
        &mut self,
        term: &mut Term,
        input: &mut EventSource,
    ) -> Result<(), TuiError> {
        let mut frames = tokio::time::interval(self.frame_interval);
        frames.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let shutdown = shutdown_signal();
        tokio::pin!(shutdown);

        while !self.quit {
            if let Some(text) = self.editing.take() {
                self.run_editor(term, input, text).await?;
            }
            if frames.period() != self.frame_interval {
                frames = tokio::time::interval(self.frame_interval);
                frames.set_missed_tick_behavior(MissedTickBehavior::Skip);
            }
            if self.dirty {
                self.draw(term)?;
                self.dirty = false;
            }

            let animating = self.ctx.animations && self.views.iter().any(|v| v.is_animating());
            tokio::select! {
                event = input.next() => match event {
                    Some(event) => self.on_event(&event),
                    None => return Err(TuiError::InputClosed),
                },
                Some(action) = self.actions.recv() => self.on_action(action),
                () = &mut shutdown => {
                    tracing::info!("shutdown signal received");
                    self.quit = true;
                }
                // A panic on any thread (e.g. a background task) already
                // restored the terminal; stop instead of drawing over it.
                () = terminal::wait_for_panic() => return Err(TuiError::Panicked),
                _ = frames.tick(), if animating => self.dirty = true,
            }
        }
        Ok(())
    }

    /// Gives the terminal to the user's editor and takes it back. Background
    /// tasks keep going; what they send waits in the channel.
    async fn run_editor(
        &mut self,
        term: &mut Term,
        input: &mut EventSource,
        text: String,
    ) -> Result<(), TuiError> {
        input.pause().await;
        terminal::restore_terminal().map_err(TuiError::Restore)?;
        let result = tokio::task::spawn_blocking(move || external_editor::edit(&text))
            .await
            .unwrap_or_else(|e| Err(format!("the editor stopped unexpectedly: {e}")));
        terminal::resume(term).map_err(TuiError::Setup)?;
        input.resume();
        self.on_action(Action::EditorDone(result));
        self.dirty = true;
        Ok(())
    }

    fn draw(&mut self, term: &mut Term) -> Result<(), TuiError> {
        let Self {
            views, ctx, toasts, ..
        } = self;
        toasts.prune();
        terminal::draw_guarded(|| term.draw(|frame| paint(frame, views, ctx, toasts)))
            .ok_or(TuiError::Panicked)?
            .map(|_| ())
            .map_err(TuiError::Draw)
    }

    /// Handles what comes in until nothing has for `quiet`.
    async fn settle(&mut self, quiet: Duration) {
        let limit = tokio::time::Instant::now() + SETTLE_LIMIT;
        while tokio::time::Instant::now() < limit {
            match tokio::time::timeout(quiet, self.actions.recv()).await {
                Ok(Some(action)) => self.on_action(action),
                Ok(None) | Err(_) => break,
            }
        }
    }

    fn shot(
        &mut self,
        step: &str,
        terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
    ) -> Result<Screen, String> {
        let Self {
            views, ctx, toasts, ..
        } = self;
        toasts.prune();
        terminal
            .draw(|frame| paint(frame, views, ctx, toasts))
            .map_err(|e| e.to_string())?;
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        let text = buffer
            .content()
            .chunks(width)
            .map(|row| {
                let line: String = row.iter().map(|cell| cell.symbol()).collect();
                line.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Screen {
            step: step.to_owned(),
            text,
        })
    }

    fn on_event(&mut self, event: &Event) {
        match event {
            Event::Resize(..) | Event::FocusGained => self.dirty = true,
            Event::Key(key) if is_force_quit(key) => self.quit = true,
            _ => {}
        }
        if self.quit {
            return;
        }

        let handled = self
            .views
            .last_mut()
            .and_then(|view| view.handle_event(event, &self.ctx));
        match handled {
            Some(action) => self.on_action(action),
            None => {
                if let Event::Key(key) = event {
                    self.on_global_key(key);
                }
            }
        }
        // Input usually changes what is on screen.
        self.dirty = true;
    }

    fn on_global_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Char('q') if key.modifiers.is_empty() => self.on_action(Action::Quit),
            KeyCode::Esc => self.on_action(Action::Back),
            _ => {}
        }
    }

    fn on_action(&mut self, action: Action) {
        tracing::debug!(action = action.name(), "action");
        match action {
            Action::Quit => self.quit = true,
            Action::Back => {
                self.views.pop();
                if self.views.is_empty() {
                    self.quit = true;
                }
            }
            Action::Redraw => {}
            Action::OpenActions(ref target, facts) => {
                self.push(Box::new(ActionMenu::new(target.clone(), facts)));
                return;
            }
            Action::OpenMerge(ref target) => {
                // In place of the menu that offered it.
                self.views.pop();
                self.push(Box::new(MergeView::new(target.clone())));
                return;
            }
            Action::Perform(ref target, ref op) => {
                self.views.pop();
                if let Some(forge) = self.ctx.forges.by_account(&target.account) {
                    sync::spawn_item_op(
                        Arc::clone(forge),
                        target.clone(),
                        op.clone(),
                        self.ctx.sender(),
                    );
                }
                self.dirty = true;
                return;
            }
            Action::Compose(ref target, purpose) => {
                // In place of the menu that offered it.
                self.views.pop();
                let draft = self
                    .ctx
                    .drafts
                    .get(&target.item)
                    .cloned()
                    .unwrap_or_default();
                self.push(Box::new(ComposeView::new(target.clone(), purpose, &draft)));
                return;
            }
            Action::CloseCompose {
                ref target,
                ref draft,
            } => {
                if draft.trim().is_empty() {
                    self.ctx.drafts.remove(&target.item);
                } else {
                    self.ctx.drafts.insert(target.item.clone(), draft.clone());
                    self.views.pop();
                    self.on_action(Action::Toast(Toast::info("draft kept for this item")));
                    return;
                }
                self.views.pop();
                self.dirty = true;
                return;
            }
            Action::EditInEditor(ref text) => {
                self.editing = Some(text.clone());
                return;
            }
            Action::OpenSettings => {
                self.push(Box::new(SettingsView::new()));
                return;
            }
            Action::ConfigChanged(ref change) => {
                self.ctx.apply_config(&change.config);
                self.frame_interval = frame_interval(&change.config);
                for view in &mut self.views {
                    view.update(&action, &self.ctx);
                }
                self.dirty = true;
                return;
            }
            // Background results and toasts concern every view, not just
            // the one on top (e.g. the shell under a detail view).
            Action::Data(_) | Action::Toast(_) => {
                if let Action::Toast(toast) = &action {
                    self.toasts.push(toast.clone());
                }
                let mut follow_ups = Vec::new();
                if let Action::Data(DataEvent::ActionDone {
                    message,
                    result,
                    target,
                }) = &action
                {
                    match result {
                        Ok(()) => {
                            self.ctx.drafts.remove(&target.item);
                            follow_ups.push(Action::Toast(Toast::success(message.clone())));
                            sync::after_search_catches_up(self.ctx.sender());
                        }
                        // A box or the merge question says it itself.
                        Err(e) if !self.views.last().is_some_and(|v| v.is_overlay()) => {
                            follow_ups.push(Action::Toast(Toast::error(format!(
                                "couldn't change {}: {e}",
                                target.item
                            ))));
                        }
                        Err(_) => {}
                    }
                }
                for view in &mut self.views {
                    follow_ups.extend(view.update(&action, &self.ctx));
                }
                for next in follow_ups {
                    self.on_action(next);
                }
                self.dirty = true;
                return;
            }
            Action::CycleAccount => {
                let accounts: Vec<_> = self
                    .ctx
                    .forges
                    .iter()
                    .map(|f| f.account().clone())
                    .collect();
                self.ctx.account_filter = match &self.ctx.account_filter {
                    None => accounts.first().cloned(),
                    Some(current) => accounts
                        .iter()
                        .position(|a| a == current)
                        .and_then(|i| accounts.get(i + 1))
                        .cloned(),
                };
            }
            Action::SetScope(ref scope) => self.ctx.scope = scope.clone(),
            Action::ShowDetail(ref detail) => {
                let view: Box<dyn View> = match detail {
                    Detail::Notification {
                        account,
                        notification,
                    } => Box::new(DetailView::notification(
                        account.clone(),
                        (**notification).clone(),
                    )),
                    Detail::Issue {
                        account,
                        item,
                        summary,
                    } => Box::new(IssueView::new(
                        account.clone(),
                        item.clone(),
                        summary.as_deref().cloned(),
                    )),
                    Detail::Pull {
                        account,
                        item,
                        summary,
                        checks_first,
                    } => Box::new(
                        PullView::new(account.clone(), item.clone(), summary.as_deref().cloned())
                            .starting_on_checks(*checks_first),
                    ),
                };
                self.push(view);
                return;
            }
            Action::Run(_) | Action::EditorDone(_) => {}
            Action::OpenLogin => {
                if let Some(flow) = self.ctx.login.clone() {
                    self.push(Box::new(LoginView::new(flow)));
                }
                return;
            }
            Action::AccountAdded(ref handle) => {
                tracing::info!(account = %handle.0.account(), "account added");
                let first_account = self.ctx.forges.is_empty();
                self.ctx.add_account(Arc::clone(&handle.0));
                self.views.pop();
                if first_account {
                    // Leave the empty state: from now on the main screen.
                    self.ctx.adopt_home_repo();
                    self.views.clear();
                    self.push(Box::new(Shell::new(Vec::new())));
                } else if let Some(view) = self.views.last_mut() {
                    // Back where the user was, with the new token.
                    view.update(&action, &self.ctx);
                }
                self.on_action(Action::Toast(Toast::success(format!(
                    "signed in as @{}",
                    handle.0.account().login
                ))));
                return;
            }
        }
        if let Some(view) = self.views.last_mut()
            && let Some(next) = view.update(&action, &self.ctx)
        {
            self.on_action(next);
        }
        self.dirty = true;
    }
}

/// Everything on one frame.
fn paint(frame: &mut Frame, views: &mut [Box<dyn View>], ctx: &Ctx, toasts: &Toasts) {
    render(frame, views, ctx);
    // Under a view without toasts, the shell's cannot be seen.
    if !views.last().is_some_and(|view| view.shows_toasts()) {
        let area = frame.area();
        toasts.render(frame, area, &ctx.theme);
    }
    crate::ui::scrub(frame.buffer_mut());
}

fn frame_interval(config: &Config) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(config.ui.max_fps.max(1)))
}

fn render(frame: &mut Frame, views: &mut [Box<dyn View>], ctx: &Ctx) {
    let area = frame.area();
    frame.render_widget(Block::new().style(ctx.theme.base()), area);
    // From the last full screen up, so an overlay has something under it.
    let from = views
        .iter()
        .rposition(|view| !view.is_overlay())
        .unwrap_or(0);
    for view in views.iter_mut().skip(from) {
        view.render(frame, area, ctx);
    }
}

/// Ctrl+C always quits, even if a view (e.g. a text input) would take the key.
/// In raw mode it arrives as a key event, not as SIGINT.
fn is_force_quit(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c'))
}

/// Resolves on Ctrl+C outside raw mode, SIGTERM/SIGHUP on Unix and console
/// close on Windows. Signals that cannot be registered are skipped.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let wait = |kind| async move {
            match signal(kind) {
                Ok(mut s) => {
                    s.recv().await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, ?kind, "could not listen for signal");
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::select! {
            () = ctrl_c() => {}
            () = wait(SignalKind::terminate()) => {}
            () = wait(SignalKind::hangup()) => {}
        }
    }
    #[cfg(windows)]
    {
        let close = async {
            match tokio::signal::windows::ctrl_close() {
                Ok(mut s) => {
                    s.recv().await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "could not listen for console close");
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::select! {
            () = ctrl_c() => {}
            () = close => {}
        }
    }
}

async fn ctrl_c() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::warn!(error = %e, "could not listen for ctrl+c");
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use hit_core::{Forge, Forges, Host, ItemRef, RepoRef};
    use hit_forge_mock::{DEMO_HOST, MockForge, MockOptions};

    use super::*;

    const QUIET: Duration = Duration::from_millis(60);

    fn rocket() -> RepoRef {
        RepoRef::new(Host::new(DEMO_HOST), "acme", "rocket")
    }

    async fn run(forge: &Arc<MockForge>, steps: &[&str]) -> Vec<Screen> {
        let mut forges = Forges::new();
        forges.add(Arc::clone(forge) as Arc<dyn Forge>);
        let steps: Vec<String> = steps.iter().map(|s| (*s).to_owned()).collect();
        let services = Services {
            forges,
            repo: Some(rocket()),
            ..Services::default()
        };
        screens(&Config::default(), services, &steps, 120, 40, QUIET)
            .await
            .unwrap()
    }

    fn demo() -> Arc<MockForge> {
        Arc::new(MockForge::demo(MockOptions::instant()))
    }

    #[tokio::test]
    async fn a_repository_opens_on_its_pull_requests() {
        let shots = run(&demo(), &[]).await;
        assert!(shots[0].text.contains("Needs you"), "{}", shots[0].text);
    }

    #[tokio::test]
    async fn the_whole_comment_flow_works_from_key_presses() {
        let forge = demo();
        // The first pull request in the first group: ask the app which it is.
        let shots = run(&forge, &["x", "c", "=written by keys", "ctrl+s"]).await;
        assert!(shots[1].text.contains("comment"), "{}", shots[1].text);
        assert!(
            shots[3].text.contains("written by keys"),
            "{}",
            shots[3].text
        );
        assert!(shots[4].text.contains("commented on"), "{}", shots[4].text);
        let pulls = forge.pull_requests().unwrap();
        let mut found = false;
        for number in [142, 145, 146, 139, 140] {
            let item = ItemRef::pull_request(rocket(), number);
            if let Ok(detail) = pulls.get(&item).await {
                found |= detail.comments.iter().any(|c| c.body == "written by keys");
            }
        }
        assert!(found);
    }

    #[tokio::test]
    async fn links_are_announced_not_opened() {
        let shots = run(&demo(), &["o"]).await;
        assert!(shots[1].text.contains("would open"), "{}", shots[1].text);
    }

    #[tokio::test]
    async fn the_editor_is_refused_without_a_terminal() {
        let forge = demo();
        let mut forges = Forges::new();
        forges.add(Arc::clone(&forge) as Arc<dyn Forge>);
        let steps = ["x", "c", "ctrl+e"].map(str::to_owned);
        let services = Services {
            forges,
            repo: Some(rocket()),
            ..Services::default()
        };
        let result = screens(&Config::default(), services, &steps, 120, 40, QUIET).await;
        assert!(result.unwrap_err().contains("real terminal"));
    }

    #[tokio::test]
    async fn a_bad_key_says_so() {
        let mut forges = Forges::new();
        forges.add(demo() as Arc<dyn Forge>);
        let steps = ["no-such-key".to_owned()];
        let services = Services {
            forges,
            ..Services::default()
        };
        assert!(
            screens(&Config::default(), services, &steps, 80, 24, QUIET)
                .await
                .is_err()
        );
    }
}
