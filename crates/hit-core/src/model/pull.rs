use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{CheckRun, CheckSummary, Comment, Label, Review, ReviewThread, User};
use crate::ItemRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// The forge's overall verdict from reviews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mergeable {
    Mergeable,
    Conflicting,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "merge commit",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
}

/// What the forge says about merging a pull request right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MergeOptions {
    /// Never empty.
    pub allowed: Vec<MergeMethod>,
    pub default: MergeMethod,
    /// Why it cannot be merged.
    pub blocked: Option<String>,
    /// It can be merged, but this is worth knowing first.
    pub warning: Option<String>,
    pub head_ref: String,
    pub base_ref: String,
    /// The branch is in this repository, not in a fork.
    pub can_delete_branch: bool,
}

/// A review's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub reference: ItemRef,
    pub title: String,
    pub author: User,
    pub state: PrState,
    pub draft: Option<bool>,
    pub labels: Vec<Label>,
    pub assignees: Vec<User>,
    pub requested_reviewers: Vec<User>,
    /// The forge's own search says this waits for the viewer's review. Also
    /// true for a request to one of their teams, which `requested_reviewers`
    /// only names as the team.
    #[serde(default)]
    pub review_requested_of_viewer: bool,
    pub review_decision: Option<ReviewDecision>,
    pub checks: Option<CheckSummary>,
    pub mergeable: Option<Mergeable>,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub changed_files: Option<u32>,
    pub comments: u32,
    pub head_ref: String,
    pub base_ref: String,
    pub url: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub closed_at: Option<Timestamp>,
    pub merged_at: Option<Timestamp>,
}

impl PullRequest {
    pub fn is_open(&self) -> bool {
        self.state == PrState::Open
    }

    pub fn is_draft(&self) -> bool {
        self.draft.unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestDetail {
    pub pr: PullRequest,
    /// Markdown description.
    pub body: String,
    pub comments: Vec<Comment>,
    /// Comments before `comments`, which holds the latest ones.
    #[serde(default)]
    pub earlier_comments: u32,
    pub reviews: Vec<Review>,
    #[serde(default)]
    pub review_threads: Vec<ReviewThread>,
    pub checks: Vec<CheckRun>,
}
