use std::{fmt, sync::Arc};

use hit_config::Config;
use hit_core::{
    AccountId, Forge, ForgeError, ItemRef,
    auth::DeviceCode,
    focus::FocusItem,
    model::{
        Issue, IssueDetail, Notification, NotificationId, PullRequest, PullRequestDetail, Repo,
    },
    query::Scope,
};

use crate::{highlight::HighlightedDiff, keymap::Command, sync::Loaded};

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
    /// Narrow everything to one repository, or show all of them again.
    SetScope(Scope),
    /// Open a full-screen detail view.
    ShowDetail(Detail),
    /// A short message in the corner.
    Toast(Toast),
    /// Run a command as if its key was pressed (from the palette).
    Run(Command),
    /// Open the settings screen.
    OpenSettings,
    /// A setting changed: apply it now.
    ConfigChanged(ConfigChange),
}

#[derive(Debug, Clone)]
pub struct ConfigChange {
    pub config: Box<Config>,
    /// Refresh loops have to start over to pick it up.
    pub reload: bool,
}

/// Results of background fetches, tagged with the account they belong to.
/// Payloads are behind `Arc` so actions stay cheap to clone.
#[derive(Debug, Clone)]
pub enum DataEvent {
    /// A refresh round started.
    Refreshing(AccountId),
    Focus {
        account: AccountId,
        scope: Scope,
        result: Result<Loaded<Arc<Vec<FocusItem>>>, ForgeError>,
    },
    UnreadCount {
        account: AccountId,
        scope: Scope,
        result: Result<Loaded<usize>, ForgeError>,
    },
    Notifications {
        account: AccountId,
        scope: Scope,
        unread_only: bool,
        result: Result<Loaded<Arc<Vec<Notification>>>, ForgeError>,
    },
    Pulls {
        account: AccountId,
        scope: Scope,
        view: &'static str,
        result: Result<Loaded<Arc<Vec<PullRequest>>>, ForgeError>,
    },
    Issues {
        account: AccountId,
        scope: Scope,
        view: &'static str,
        result: Result<Loaded<Arc<Vec<Issue>>>, ForgeError>,
    },
    Repos {
        account: AccountId,
        result: Result<Loaded<Arc<Vec<Repo>>>, ForgeError>,
    },
    IssueDetail {
        item: ItemRef,
        result: Result<Loaded<Arc<IssueDetail>>, ForgeError>,
    },
    PullDetail {
        item: ItemRef,
        result: Result<Loaded<Arc<PullRequestDetail>>, ForgeError>,
    },
    PullDiff {
        item: ItemRef,
        result: Result<Arc<HighlightedDiff>, ForgeError>,
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
    Notification {
        account: AccountId,
        notification: Box<Notification>,
    },
    /// A pull request; `summary` (if known) shows while the detail loads.
    Pull {
        account: AccountId,
        item: ItemRef,
        summary: Option<Box<PullRequest>>,
    },
    /// An issue; `summary` (if known) shows while the detail loads.
    Issue {
        account: AccountId,
        item: ItemRef,
        summary: Option<Box<Issue>>,
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
