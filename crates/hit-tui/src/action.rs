use std::{fmt, sync::Arc};

use hit_core::{
    AccountId, Forge, ForgeError,
    auth::DeviceCode,
    focus::FocusItem,
    model::{Notification, NotificationId},
};

use crate::{keymap::Command, sync::Loaded};

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
    /// Show one account only, cycling through them, then all again.
    CycleAccount,
    /// Open a full-screen detail view.
    ShowDetail(Detail),
    /// A short message in the corner.
    Toast(Toast),
    /// Run a command as if its key was pressed (from the palette).
    Run(Command),
}

/// Results of background fetches, tagged with the account they belong to.
/// Payloads are behind `Arc` so actions stay cheap to clone.
#[derive(Debug, Clone)]
pub enum DataEvent {
    /// A refresh round started.
    Refreshing(AccountId),
    Focus {
        account: AccountId,
        result: Result<Loaded<Arc<Vec<FocusItem>>>, ForgeError>,
    },
    UnreadCount {
        account: AccountId,
        result: Result<Loaded<usize>, ForgeError>,
    },
    Notifications {
        account: AccountId,
        unread_only: bool,
        result: Result<Loaded<Arc<Vec<Notification>>>, ForgeError>,
    },
    /// The outcome of marking `ids` read (all of them if the request was
    /// "mark all").
    MarkedRead {
        account: AccountId,
        ids: Vec<NotificationId>,
        result: Result<(), ForgeError>,
    },
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

#[derive(Debug, Clone)]
pub enum Detail {
    Focus {
        account: AccountId,
        item: Box<FocusItem>,
    },
    Notification {
        account: AccountId,
        notification: Box<Notification>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub kind: ToastKind,
    pub message: String,
}

impl Toast {
    fn new(kind: ToastKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self::new(ToastKind::Info, message)
    }

    pub fn success(message: impl Into<String>) -> Self {
        Self::new(ToastKind::Success, message)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(ToastKind::Warning, message)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(ToastKind::Error, message)
    }
}

#[derive(Clone)]
pub struct ForgeHandle(pub Arc<dyn Forge>);

impl fmt::Debug for ForgeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ForgeHandle({})", self.0.account())
    }
}
