use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Comment, Label, User};
use crate::ItemRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    Completed,
    NotPlanned,
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    Open,
    Closed(Option<CloseReason>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub reference: ItemRef,
    pub title: String,
    pub author: User,
    pub state: IssueState,
    pub labels: Vec<Label>,
    pub assignees: Vec<User>,
    pub comments: u32,
    pub url: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub closed_at: Option<Timestamp>,
}

impl Issue {
    pub fn is_open(&self) -> bool {
        self.state == IssueState::Open
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetail {
    pub issue: Issue,
    /// Markdown description.
    pub body: String,
    pub comments: Vec<Comment>,
}
