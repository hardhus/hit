use std::sync::Arc;

use hit_core::{ForgeError, focus::FocusItem};

/// Something the app should do. Views return actions from input handling, and
/// background tasks send them through [`crate::Ctx::sender`].
#[derive(Debug, Clone)]
pub enum Action {
    Quit,
    /// Close the top view; quits when it is the last one.
    Back,
    /// Force a redraw (e.g. after a background result changed a view).
    Redraw,
    /// A background fetch finished.
    Data(DataEvent),
}

/// Results of background fetches. Payloads are behind `Arc` so actions stay
/// cheap to clone.
#[derive(Debug, Clone)]
pub enum DataEvent {
    Focus(Result<Arc<Vec<FocusItem>>, ForgeError>),
    UnreadCount(Result<usize, ForgeError>),
}
