use std::{fmt, sync::Arc};

use hit_core::{Forge, ForgeError, auth::DeviceCode, focus::FocusItem};

use crate::sync::Loaded;

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
    /// Open the sign-in screen.
    OpenLogin,
    /// A sign-in finished; the account is ready to use.
    AccountAdded(ForgeHandle),
}

/// Results of background fetches. Payloads are behind `Arc` so actions stay
/// cheap to clone.
#[derive(Debug, Clone)]
pub enum DataEvent {
    /// A refresh round started.
    Refreshing,
    Focus(Result<Loaded<Arc<Vec<FocusItem>>>, ForgeError>),
    UnreadCount(Result<Loaded<usize>, ForgeError>),
    Login(LoginEvent),
}

#[derive(Debug, Clone)]
pub enum LoginEvent {
    Code {
        code: DeviceCode,
        copied: bool,
        opened: bool,
    },
    Failed(String),
}

#[derive(Clone)]
pub struct ForgeHandle(pub Arc<dyn Forge>);

impl fmt::Debug for ForgeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ForgeHandle({})", self.0.account())
    }
}
