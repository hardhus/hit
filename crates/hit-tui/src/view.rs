use std::{sync::Arc, time::Instant};

use hit_core::{Forges, auth::LoginFlow};
use ratatui::{Frame, layout::Rect};
use tokio::sync::mpsc;

use crate::{Action, Event, Theme};

/// Shared, read-mostly context handed to every view.
pub struct Ctx {
    pub theme: Theme,
    pub animations: bool,
    /// Connected forge accounts (the demo forge with `--demo`).
    pub forges: Forges,
    /// Sign-in from inside the TUI, when available.
    pub login: Option<Arc<dyn LoginFlow>>,
    started: Instant,
    tx: mpsc::UnboundedSender<Action>,
}

impl Ctx {
    pub fn new(
        theme: Theme,
        animations: bool,
        forges: Forges,
        login: Option<Arc<dyn LoginFlow>>,
        tx: mpsc::UnboundedSender<Action>,
    ) -> Self {
        Self {
            theme,
            animations,
            forges,
            login,
            started: Instant::now(),
            tx,
        }
    }

    /// Seconds since the app started; drives animations.
    pub fn time(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// Channel for actions produced outside input handling (background tasks).
    pub fn sender(&self) -> mpsc::UnboundedSender<Action> {
        self.tx.clone()
    }
}

/// A screen on the view stack. Kept deliberately small; extend as the UX grows.
pub trait View {
    /// Called once when the view is pushed; a good place to start fetches.
    fn mount(&mut self, ctx: &Ctx) {
        let _ = ctx;
    }

    /// Handles input. Unhandled input returns `None` and falls through to the
    /// app's global keys.
    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let _ = (event, ctx);
        None
    }

    /// Reacts to an action the app did not consume itself.
    fn update(&mut self, action: &Action, ctx: &Ctx) -> Option<Action> {
        let _ = (action, ctx);
        None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx);

    /// While any view animates, the app redraws at the configured frame cap.
    fn is_animating(&self) -> bool {
        false
    }
}
