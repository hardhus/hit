//! Query and pagination types accepted by the capability traits.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::RepoRef;

/// Everything the account can see, or a single repository (repo context).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    #[default]
    All,
    Repo(RepoRef),
}

impl Scope {
    pub fn includes(&self, repo: &RepoRef) -> bool {
        match self {
            Self::All => true,
            Self::Repo(scope) => scope == repo,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StateFilter {
    #[default]
    Open,
    Closed,
    All,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sort {
    /// Most recently updated first.
    #[default]
    Updated,
    /// Newest first.
    Created,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrQuery {
    pub scope: Scope,
    pub state: StateFilter,
    /// Only pull requests by this login.
    pub author: Option<String>,
    /// Free text matched against titles.
    pub search: Option<String>,
    pub sort: Sort,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueQuery {
    pub scope: Scope,
    pub state: StateFilter,
    pub author: Option<String>,
    pub assignee: Option<String>,
    pub search: Option<String>,
    pub sort: Sort,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationQuery {
    pub scope: Scope,
    pub unread_only: bool,
    /// Only notifications that address the user directly.
    pub participating_only: bool,
    pub since: Option<Timestamp>,
}

/// Opaque continuation token; only the provider that issued it understands it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cursor(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRequest {
    pub cursor: Option<Cursor>,
    pub per_page: u16,
}

impl PageRequest {
    pub const DEFAULT_PER_PAGE: u16 = 30;

    pub fn first(per_page: u16) -> Self {
        Self {
            cursor: None,
            per_page: per_page.max(1),
        }
    }

    pub fn after(cursor: Cursor, per_page: u16) -> Self {
        Self {
            cursor: Some(cursor),
            per_page: per_page.max(1),
        }
    }
}

impl Default for PageRequest {
    fn default() -> Self {
        Self::first(Self::DEFAULT_PER_PAGE)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` on the last page.
    pub next: Option<Cursor>,
}

impl<T> Page<T> {
    pub fn last(items: Vec<T>) -> Self {
        Self { items, next: None }
    }
}
