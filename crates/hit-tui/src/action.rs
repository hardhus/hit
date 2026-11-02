use std::{fmt, sync::Arc};

use hit_config::Config;
use hit_core::{
    AccountId, Forge, ForgeError, ItemRef,
    auth::DeviceCode,
    focus::FocusItem,
    model::{
        Issue, IssueDetail, MergeOptions, Notification, NotificationId, PullRequest,
        PullRequestDetail, Repo, ReviewVerdict,
    },
    query::{Cursor, Page, Scope},
};

use crate::{
    highlight::HighlightedDiff,
    keymap::Command,
    sync::{ItemOp, Listing, Loaded},
    views::Facts,
};

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
    /// What can be done with this item.
    OpenActions(Target, Facts),
    /// Write a comment or a review.
    Compose(Target, Purpose),
    /// Ask before merging.
    OpenMerge(Target),
    /// Do it now: nothing to ask or to write.
    Perform(Target, ItemOp),
    /// Leave the comment box, keeping `draft` for next time.
    CloseCompose {
        target: Target,
        draft: String,
    },
    /// Hand the terminal to the user's editor with this text.
    EditInEditor(String),
    /// The editor closed.
    EditorDone(Result<String, String>),
    /// A setting changed: apply it now.
    ConfigChanged(ConfigChange),
}

impl Action {
    /// For the log. Never the contents: comments, notification titles and
    /// private repositories' data do not belong in a plain text file.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Quit => "quit",
            Self::Back => "back",
            Self::Redraw => "redraw",
            Self::Data(_) => "data",
            Self::OpenLogin => "open_login",
            Self::AccountAdded(_) => "account_added",
            Self::CycleAccount => "cycle_account",
            Self::SetScope(_) => "set_scope",
            Self::ShowDetail(_) => "show_detail",
            Self::Toast(_) => "toast",
            Self::Run(_) => "run",
            Self::OpenSettings => "open_settings",
            Self::OpenActions(..) => "open_actions",
            Self::Compose(..) => "compose",
            Self::OpenMerge(_) => "open_merge",
            Self::Perform(..) => "perform",
            Self::CloseCompose { .. } => "close_compose",
            Self::EditInEditor(_) => "edit_in_editor",
            Self::EditorDone(_) => "editor_done",
            Self::ConfigChanged(_) => "config_changed",
        }
    }
}

/// What a text box is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Comment,
    Review(ReviewVerdict),
}

/// A pull request or issue, and the account to act with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub account: AccountId,
    pub item: ItemRef,
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
    /// Something was changed on the forge.
    ActionDone {
        target: Target,
        /// What to tell the user if it worked.
        message: String,
        result: Result<(), ForgeError>,
    },
    /// Long enough after a change for the forge's search to have caught up.
    Settled,
    MergeOptions {
        target: Target,
        result: Result<MergeOptions, ForgeError>,
    },
    Focus {
        account: AccountId,
        scope: Scope,
        result: Result<Loaded<Arc<Vec<FocusItem>>>, ForgeError>,
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
        result: Result<Loaded<Arc<Listing<PullRequest>>>, ForgeError>,
    },
    /// Another page of a list; `cursor` is where it was asked from.
    PullsMore {
        account: AccountId,
        scope: Scope,
        view: &'static str,
        cursor: Cursor,
        result: Result<Page<PullRequest>, ForgeError>,
    },
    Issues {
        account: AccountId,
        scope: Scope,
        view: &'static str,
        result: Result<Loaded<Arc<Listing<Issue>>>, ForgeError>,
    },
    IssuesMore {
        account: AccountId,
        scope: Scope,
        view: &'static str,
        cursor: Cursor,
        result: Result<Page<Issue>, ForgeError>,
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
        /// Failing checks are what it is about: open on them.
        checks_first: bool,
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
