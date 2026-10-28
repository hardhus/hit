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
    AccountId, ForgeKind, ItemRef, RepoRef, Result,
    diff::Diff,
    focus::FocusItem,
    model::{
        Issue, IssueDetail, Notification, NotificationId, PullRequest, PullRequestDetail, Repo,
    },
    query::{IssueQuery, NotificationQuery, Page, PageRequest, PrQuery, Scope},
};

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

    /// A key only this account's credentials can produce, for encrypting
    /// what hit stores locally about it. `None` disables local caching.
    fn local_key(&self) -> Option<Zeroizing<[u8; 32]>> {
        None
    }
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
}
