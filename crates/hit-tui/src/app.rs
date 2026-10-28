use std::{sync::Arc, time::Duration};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hit_config::Config;
use ratatui::{Frame, widgets::Block};
use tokio::{sync::mpsc, time::MissedTickBehavior};

use crate::{
    Action, Ctx, Event, Services, Theme, TuiError, View,
    event::EventSource,
    terminal::{self, Term, TerminalGuard},
    views::{LoginView, Welcome},
};

/// Runs the TUI until the user quits or a shutdown signal arrives.
pub async fn run(
    config: &Config,
    warnings: Vec<String>,
    services: Services,
) -> Result<(), TuiError> {
    let mut app = App::new(config, services);
    app.push(Box::new(Welcome::new(warnings)));
    app.run().await
}

pub struct App {
    views: Vec<Box<dyn View>>,
    ctx: Ctx,
    actions: mpsc::UnboundedReceiver<Action>,
    frame_interval: Duration,
    dirty: bool,
    quit: bool,
}

impl App {
    pub fn new(config: &Config, services: Services) -> Self {
        let (tx, actions) = mpsc::unbounded_channel();
        let theme = Theme::from_config(&config.ui);
        let fps = f64::from(config.ui.max_fps.max(1));
        Self {
            views: Vec::new(),
            ctx: Ctx::new(
                theme,
                config.ui.animations,
                services,
                config.refresh.clone(),
                tx,
            ),
            actions,
            frame_interval: Duration::from_secs_f64(1.0 / fps),
            dirty: true,
            quit: false,
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

    fn draw(&mut self, term: &mut Term) -> Result<(), TuiError> {
        let Self { views, ctx, .. } = self;
        terminal::draw_guarded(|| term.draw(|frame| render(frame, views, ctx)))
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
            Action::Redraw | Action::Data(_) => {}
            Action::OpenLogin => {
                if let Some(flow) = self.ctx.login.clone() {
                    self.push(Box::new(LoginView::new(flow)));
                }
                return;
            }
            Action::AccountAdded(ref handle) => {
                tracing::info!(account = %handle.0.account(), "account added");
                self.ctx.forges.add(Arc::clone(&handle.0));
                // Close the sign-in screen and refresh what is below it.
                self.views.pop();
                if let Some(view) = self.views.last_mut() {
                    view.mount(&self.ctx);
                }
                self.dirty = true;
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

fn render(frame: &mut Frame, views: &mut [Box<dyn View>], ctx: &Ctx) {
    let area = frame.area();
    frame.render_widget(Block::new().style(ctx.theme.base()), area);
    if let Some(view) = views.last_mut() {
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
