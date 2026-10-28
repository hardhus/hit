use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::User;

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
