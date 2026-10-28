use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct User {
    pub login: String,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_bot: bool,
}

impl User {
    pub fn new(login: impl Into<String>) -> Self {
        Self {
            login: login.into(),
            name: None,
            avatar_url: None,
            is_bot: false,
        }
    }
}
