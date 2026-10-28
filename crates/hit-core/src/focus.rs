//! "Action Required": items that need the user, and why.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    ItemRef,
    model::{CheckState, Issue, Mergeable, PullRequest, ReviewDecision},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusReason {
    /// Someone asked the user to review.
    ReviewRequested,
    /// The user's pull request has requested changes.
    ChangesRequested,
    /// The user's pull request has failing checks.
    ChecksFailing,
    /// The user's pull request is approved and green.
    ReadyToMerge,
    Mentioned,
    Assigned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FocusSubject {
    PullRequest(PullRequest),
    Issue(Issue),
}

impl FocusSubject {
    pub fn reference(&self) -> &ItemRef {
        match self {
            Self::PullRequest(pr) => &pr.reference,
            Self::Issue(issue) => &issue.reference,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Self::PullRequest(pr) => &pr.title,
            Self::Issue(issue) => &issue.title,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusItem {
    pub subject: FocusSubject,
    /// Never empty; sorted, most important first.
    pub reasons: Vec<FocusReason>,
    pub updated_at: Timestamp,
}

impl FocusItem {
    pub fn has(&self, reason: FocusReason) -> bool {
        self.reasons.contains(&reason)
    }
}

/// Focus reasons for a pull request from `me`'s point of view, for providers
/// that cannot ask the forge directly. `mentioned`: `@me` appears in the body
/// or comments.
pub fn pr_reasons(pr: &PullRequest, me: &str, mentioned: bool) -> Vec<FocusReason> {
    if !pr.is_open() {
        return Vec::new();
    }
    let mut reasons = Vec::new();
    let mine = pr.author.login == me;
    if pr.requested_reviewers.iter().any(|u| u.login == me) {
        reasons.push(FocusReason::ReviewRequested);
    }
    if mine && pr.review_decision == Some(ReviewDecision::ChangesRequested) {
        reasons.push(FocusReason::ChangesRequested);
    }
    let checks = pr.checks.map(|c| c.state);
    if mine && checks == Some(CheckState::Failure) {
        reasons.push(FocusReason::ChecksFailing);
    }
    if mine
        && !pr.is_draft()
        && pr.review_decision == Some(ReviewDecision::Approved)
        && matches!(checks, None | Some(CheckState::Success))
        && pr.mergeable != Some(Mergeable::Conflicting)
    {
        reasons.push(FocusReason::ReadyToMerge);
    }
    if mentioned && !mine {
        reasons.push(FocusReason::Mentioned);
    }
    if pr.assignees.iter().any(|u| u.login == me) && !mine {
        reasons.push(FocusReason::Assigned);
    }
    reasons
}

/// Focus reasons for an issue; see [`pr_reasons`].
pub fn issue_reasons(issue: &Issue, me: &str, mentioned: bool) -> Vec<FocusReason> {
    if !issue.is_open() {
        return Vec::new();
    }
    let mut reasons = Vec::new();
    if mentioned && issue.author.login != me {
        reasons.push(FocusReason::Mentioned);
    }
    if issue.assignees.iter().any(|u| u.login == me) {
        reasons.push(FocusReason::Assigned);
    }
    reasons
}
