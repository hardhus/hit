//! Capability traits.
//!
//! A provider implements [`Forge`] plus whichever capability traits its forge
//! supports, and exposes them through the accessor methods. `None` means "not
//! supported", and the UI hides the matching features.
//!
//! Read and write capabilities are separate traits; write traits (comments,
//! reviews, merging) are added later without touching these.

use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::{
    AccountId, ForgeKind, ItemRef, RepoRef, Result, Timestamp,
    diff::Diff,
    focus::FocusItem,
    model::{
        CloseReason, Issue, IssueDetail, MergeMethod, MergeOptions, Notification, NotificationId,
        PullRequest, PullRequestDetail, Repo, RepoCounts, ReviewVerdict,
    },
    query::{IssueQuery, NotificationQuery, Page, PageRequest, PrQuery, Scope},
};

/// How much of a request budget is left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    pub remaining: u32,
    pub limit: u32,
    pub reset_at: Option<Timestamp>,
}

impl RateLimit {
    pub fn is_exhausted(&self) -> bool {
        self.remaining == 0
    }
}

/// What the forge last said about this account.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Health {
    /// The token was rejected: the account needs a new sign-in.
    pub token_rejected: bool,
    /// Set only when running low, so there is something worth saying.
    pub rate_limit: Option<RateLimit>,
}

pub trait Forge: Send + Sync {
    fn kind(&self) -> ForgeKind;

    /// The account this forge instance acts as.
    fn account(&self) -> &AccountId;

    fn pull_requests(&self) -> Option<&dyn PullRequests> {
        None
    }

    fn issues(&self) -> Option<&dyn Issues> {
        None
    }

    fn notifications(&self) -> Option<&dyn Notifications> {
        None
    }

    fn focus(&self) -> Option<&dyn Focus> {
        None
    }

    fn repositories(&self) -> Option<&dyn Repositories> {
        None
    }

    /// Changing things on the forge. Read-only accounts refuse these before
    /// anything is sent.
    fn actions(&self) -> Option<&dyn ItemActions> {
        None
    }

    fn health(&self) -> Health {
        Health::default()
    }

    /// Whether this account refuses to write.
    fn is_read_only(&self) -> bool {
        false
    }

    /// Refuses writes from now on. There is deliberately no way back while
    /// hit runs: leaving read-only mode takes a restart.
    fn lock_read_only(&self) {}

    /// A key only this account's credentials can produce, for encrypting
    /// what hit stores locally about it. `None` disables local caching.
    fn local_key(&self) -> Option<Zeroizing<[u8; 32]>> {
        None
    }
}

#[async_trait]
pub trait ItemActions: Send + Sync {
    /// Adds a comment to a pull request or an issue.
    async fn comment(&self, item: &ItemRef, body: &str) -> Result<()>;
    /// Submits a review. `body` may be empty when approving.
    async fn review(&self, item: &ItemRef, verdict: ReviewVerdict, body: &str) -> Result<()>;
    async fn merge_options(&self, item: &ItemRef) -> Result<MergeOptions>;
    async fn merge(&self, item: &ItemRef, method: MergeMethod) -> Result<()>;
    /// Deletes a merged pull request's branch from its repository.
    async fn delete_branch(&self, item: &ItemRef, branch: &str) -> Result<()>;
    /// Closes or reopens. `reason` only means something for closing an issue.
    async fn set_open(&self, item: &ItemRef, open: bool, reason: Option<CloseReason>)
    -> Result<()>;
    /// Runs a pull request's failed checks again; how many runs that was.
    async fn rerun_failed(&self, item: &ItemRef) -> Result<usize>;
}

#[async_trait]
pub trait PullRequests: Send + Sync {
    async fn list(&self, query: &PrQuery, page: &PageRequest) -> Result<Page<PullRequest>>;
    async fn get(&self, pr: &ItemRef) -> Result<PullRequestDetail>;
    async fn diff(&self, pr: &ItemRef) -> Result<Diff>;
}

#[async_trait]
pub trait Issues: Send + Sync {
    async fn list(&self, query: &IssueQuery, page: &PageRequest) -> Result<Page<Issue>>;
    async fn get(&self, issue: &ItemRef) -> Result<IssueDetail>;
}

#[async_trait]
pub trait Notifications: Send + Sync {
    async fn list(
        &self,
        query: &NotificationQuery,
        page: &PageRequest,
    ) -> Result<Page<Notification>>;
    async fn mark_read(&self, id: &NotificationId) -> Result<()>;
    async fn mark_all_read(&self, scope: &Scope) -> Result<()>;

    /// How often the forge wants notifications to be polled, if it says.
    fn poll_interval(&self) -> Option<std::time::Duration> {
        None
    }
}

/// "Action Required". A capability of its own because forges answer it
/// differently (GitHub search queries, GitLab to-do list).
#[async_trait]
pub trait Focus: Send + Sync {
    /// Items that need the user, most recently updated first.
    async fn items(&self, scope: &Scope) -> Result<Vec<FocusItem>>;
}

#[async_trait]
pub trait Repositories: Send + Sync {
    async fn get(&self, repo: &RepoRef) -> Result<Repo>;
    /// Repositories the user owns or contributes to.
    async fn list_mine(&self, page: &PageRequest) -> Result<Page<Repo>>;
    /// Open counts for [`Self::list_mine`], most recently pushed first, for
    /// forges where they cost more than the list. Others report them with
    /// the repository and leave this empty.
    async fn open_counts(&self, _page: &PageRequest) -> Result<Page<RepoCounts>> {
        Ok(Page::last(Vec::new()))
    }
}
