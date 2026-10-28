use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::RepoRef;

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
}
