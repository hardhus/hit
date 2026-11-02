use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::User;

/// Comments on a line of a pull request's diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThread {
    pub path: String,
    /// `None` when the line is no longer in the diff.
    pub line: Option<u32>,
    pub resolved: bool,
    pub outdated: bool,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// Forge-specific comment id.
    pub id: String,
    pub author: User,
    /// Markdown.
    pub body: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}
