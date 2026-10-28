//! Identifiers shared by every forge.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ForgeKind {
    GitHub,
    GitLab,
    Gitea,
    /// The built-in demo forge (`hit --demo`).
    Demo,
}

impl ForgeKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Gitea => "Gitea",
            Self::Demo => "Demo",
        }
    }
}

impl fmt::Display for ForgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A forge host such as `github.com` or `gitlab.example.com`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Host(String);

impl Host {
    pub fn new(host: impl Into<String>) -> Self {
        Self(host.into().to_ascii_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One signed-in account on one host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId {
    pub host: Host,
    pub login: String,
}

impl AccountId {
    pub fn new(host: Host, login: impl Into<String>) -> Self {
        Self {
            host,
            login: login.into(),
        }
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.login, self.host)
    }
}

/// A repository. `owner` may contain `/` for nested groups (GitLab).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RepoRef {
    pub host: Host,
    pub owner: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid repository path {0:?}: expected owner/name")]
pub struct ParseRepoRefError(pub String);

impl RepoRef {
    pub fn new(host: Host, owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            host,
            owner: owner.into(),
            name: name.into(),
        }
    }

    /// Parses `owner/name` or `group/subgroup/name`; the last segment is the
    /// repository name.
    pub fn parse(host: Host, path: &str) -> Result<Self, ParseRepoRefError> {
        let path = path.trim().trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let invalid = || ParseRepoRefError(path.to_owned());
        let (owner, name) = path.rsplit_once('/').ok_or_else(invalid)?;
        if owner.is_empty() || name.is_empty() || owner.split('/').any(str::is_empty) {
            return Err(invalid());
        }
        Ok(Self::new(host, owner, name))
    }

    /// `owner/name`, as shown to users.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

impl fmt::Display for RepoRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A pull request (GitHub, Gitea) or merge request (GitLab).
    PullRequest,
    Issue,
}

/// A pull request or issue, addressed by repository and number.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemRef {
    pub repo: RepoRef,
    pub kind: ItemKind,
    pub number: u64,
}

impl ItemRef {
    pub fn pull_request(repo: RepoRef, number: u64) -> Self {
        Self {
            repo,
            kind: ItemKind::PullRequest,
            number,
        }
    }

    pub fn issue(repo: RepoRef, number: u64) -> Self {
        Self {
            repo,
            kind: ItemKind::Issue,
            number,
        }
    }
}

impl fmt::Display for ItemRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.repo, self.number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        Host::new("GitHub.com")
    }

    #[test]
    fn host_is_lowercased() {
        assert_eq!(host().as_str(), "github.com");
    }

    #[test]
    fn parses_owner_and_name() {
        let repo = RepoRef::parse(host(), "hardhus/hit").unwrap();
        assert_eq!(
            (repo.owner.as_str(), repo.name.as_str()),
            ("hardhus", "hit")
        );
        assert_eq!(repo.to_string(), "hardhus/hit");
    }

    #[test]
    fn parses_nested_groups_and_git_suffix() {
        let repo = RepoRef::parse(host(), "/group/sub/project.git").unwrap();
        assert_eq!(repo.owner, "group/sub");
        assert_eq!(repo.name, "project");
    }

    #[test]
    fn rejects_invalid_paths() {
        for path in ["", "hit", "/hit", "owner/", "a//b"] {
            assert!(RepoRef::parse(host(), path).is_err(), "{path:?}");
        }
    }

    #[test]
    fn item_display() {
        let repo = RepoRef::new(host(), "o", "r");
        assert_eq!(ItemRef::pull_request(repo, 42).to_string(), "o/r#42");
    }
}
