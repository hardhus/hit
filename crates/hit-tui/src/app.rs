use std::{sync::Arc, time::Duration};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hit_config::Config;
use ratatui::{Frame, widgets::Block};
use tokio::{sync::mpsc, time::MissedTickBehavior};

use crate::{
    Action, Ctx, DataEvent, Event, Services, TuiError, View,
    action::{Detail, Toast},
    event::EventSource,
    external_editor,
    keymap::Keymap,
    shell::Shell,
    terminal::{self, Term, TerminalGuard},
    ui::toast::Toasts,
    views::{
        ActionMenu, ComposeView, DetailView, IssueView, LoginView, PullView, SettingsView, Welcome,
    },
};

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
        terminal::draw_guarded(|| {
            term.draw(|frame| {
                render(frame, views, ctx);
                // Under a view without toasts, the shell's cannot be seen.
                if !views.last().is_some_and(|view| view.shows_toasts()) {
                    let area = frame.area();
                    toasts.render(frame, area, &ctx.theme);
                }
                crate::ui::scrub(frame.buffer_mut());
            })
        })
        .ok_or(TuiError::Panicked)?
        .map(|_| ())
        .map_err(TuiError::Draw)
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
        tracing::debug!(?action, "action");
        match action {
            Action::Quit => self.quit = true,
            Action::Back => {
                self.views.pop();
                if self.views.is_empty() {
                    self.quit = true;
                }
            }
            Action::Redraw => {}
            Action::OpenActions(ref target) => {
                self.push(Box::new(ActionMenu::new(target.clone())));
                return;
            }
            Action::Compose(ref target) => {
                // In place of the menu that offered it.
                self.views.pop();
                let draft = self
                    .ctx
                    .drafts
                    .get(&target.item)
                    .cloned()
                    .unwrap_or_default();
                self.push(Box::new(ComposeView::new(target.clone(), &draft)));
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
                    self.toasts.push(Toast::info("draft kept for this item"));
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
                if let Action::Data(DataEvent::ActionDone {
                    what,
                    result: Ok(()),
                    target,
                }) = &action
                {
                    self.ctx.drafts.remove(&target.item);
                    self.toasts
                        .push(Toast::success(format!("{what} on {}", target.item)));
                }
                let mut follow_ups = Vec::new();
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
                self.ctx.forges.upsert(Arc::clone(&handle.0));
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
