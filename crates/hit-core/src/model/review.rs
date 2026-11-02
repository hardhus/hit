use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::User;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub author: User,
    pub state: ReviewState,
    /// Markdown; may be empty.
    pub body: String,
    pub submitted_at: Option<Timestamp>,
}
