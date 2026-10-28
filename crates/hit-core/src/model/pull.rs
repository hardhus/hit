use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{CheckRun, CheckSummary, Comment, Label, Review, User};
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
    pub reviews: Vec<Review>,
    pub checks: Vec<CheckRun>,
}
