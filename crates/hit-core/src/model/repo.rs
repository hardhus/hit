use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::RepoRef;

/// A repository's open pull requests and issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCounts {
    pub reference: RepoRef,
    pub open_prs: u32,
    pub open_issues: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub reference: RepoRef,
    pub description: Option<String>,
    pub private: bool,
    pub fork: bool,
    pub archived: bool,
    pub default_branch: Option<String>,
    pub url: String,
    pub pushed_at: Option<Timestamp>,
    /// Open pull requests, when the forge reports them with the repository.
    #[serde(default)]
    pub open_prs: Option<u32>,
    /// Open issues (without pull requests), likewise.
    #[serde(default)]
    pub open_issues: Option<u32>,
}
