use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{ItemRef, RepoRef};

/// Forge-specific notification (thread) id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NotificationId(pub String);

impl fmt::Display for NotificationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    PullRequest,
    Issue,
    Release,
    Commit,
    Discussion,
    CheckSuite,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub title: String,
    pub kind: SubjectKind,
    /// Set when the subject is a pull request or issue.
    pub item: Option<ItemRef>,
    pub url: Option<String>,
}

/// Why the user received the notification.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationReason {
    ReviewRequested,
    Mention,
    TeamMention,
    Assign,
    Author,
    Comment,
    StateChange,
    CiActivity,
    Subscribed,
    SecurityAlert,
    Other(String),
}

impl NotificationReason {
    /// Reasons that address the user directly rather than via a subscription.
    pub fn is_participating(&self) -> bool {
        matches!(
            self,
            Self::ReviewRequested | Self::Mention | Self::TeamMention | Self::Assign | Self::Author
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub id: NotificationId,
    pub repo: RepoRef,
    pub subject: Subject,
    pub reason: NotificationReason,
    pub unread: bool,
    pub updated_at: Timestamp,
}
