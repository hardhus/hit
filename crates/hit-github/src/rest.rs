//! REST response shapes: notifications and repositories.

use hit_core::{
    ForgeError, Host, ItemRef, RepoRef, Timestamp,
    model::{Notification, NotificationId, NotificationReason, Repo, Subject, SubjectKind},
};
use serde::Deserialize;

use crate::GithubHost;

/// A repository or owner name, safe to put in a URL path. GitHub names use
/// `[A-Za-z0-9._-]`; anything else could change the request path.
pub(crate) fn segment(value: &str) -> Result<&str, ForgeError> {
    let valid = !value.is_empty()
        && value.len() <= 100
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(value)
    } else {
        Err(ForgeError::InvalidResponse(format!(
            "invalid repository path segment {value:?}"
        )))
    }
}

/// `owner/name` for a request path, validated.
pub(crate) fn repo_path(repo: &RepoRef) -> Result<String, ForgeError> {
    Ok(format!(
        "{}/{}",
        segment(&repo.owner)?,
        segment(&repo.name)?
    ))
}

#[derive(Debug, Deserialize)]
pub(crate) struct OwnerJson {
    login: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RepoJson {
    name: String,
    owner: OwnerJson,
    description: Option<String>,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    fork: bool,
    #[serde(default)]
    archived: bool,
    default_branch: Option<String>,
    html_url: String,
    pushed_at: Option<Timestamp>,
}

impl RepoJson {
    fn reference(&self, host: &Host) -> RepoRef {
        RepoRef::new(host.clone(), self.owner.login.clone(), self.name.clone())
    }

    pub fn into_model(self, host: &Host) -> Repo {
        Repo {
            reference: self.reference(host),
            description: self.description.filter(|d| !d.is_empty()),
            private: self.private,
            fork: self.fork,
            archived: self.archived,
            default_branch: self.default_branch,
            url: self.html_url,
            pushed_at: self.pushed_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubjectJson {
    title: String,
    url: Option<String>,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NotificationJson {
    id: String,
    unread: bool,
    reason: String,
    updated_at: Timestamp,
    subject: SubjectJson,
    repository: RepoJson,
}

impl NotificationJson {
    pub fn into_model(self, host: &GithubHost) -> Notification {
        let repo = self.repository.reference(host.host());
        let number = self
            .subject
            .url
            .as_deref()
            .and_then(|url| url.rsplit('/').next())
            .and_then(|n| n.parse::<u64>().ok());
        let (kind, item, web) = match (self.subject.kind.as_str(), number) {
            ("PullRequest", Some(n)) => (
                SubjectKind::PullRequest,
                Some(ItemRef::pull_request(repo.clone(), n)),
                format!("{}{repo}/pull/{n}", host.web()),
            ),
            ("Issue", Some(n)) => (
                SubjectKind::Issue,
                Some(ItemRef::issue(repo.clone(), n)),
                format!("{}{repo}/issues/{n}", host.web()),
            ),
            (kind, _) => (
                match kind {
                    "Release" => SubjectKind::Release,
                    "Commit" => SubjectKind::Commit,
                    "Discussion" => SubjectKind::Discussion,
                    "CheckSuite" => SubjectKind::CheckSuite,
                    other => SubjectKind::Other(other.to_owned()),
                },
                None,
                self.repository.html_url.clone(),
            ),
        };
        Notification {
            id: NotificationId(self.id),
            repo,
            subject: Subject {
                title: self.subject.title,
                kind,
                item,
                url: Some(web),
            },
            reason: reason(&self.reason),
            unread: self.unread,
            updated_at: self.updated_at,
        }
    }
}

fn reason(reason: &str) -> NotificationReason {
    use NotificationReason as R;
    match reason {
        "review_requested" => R::ReviewRequested,
        "mention" => R::Mention,
        "team_mention" => R::TeamMention,
        "assign" => R::Assign,
        "author" => R::Author,
        "comment" => R::Comment,
        "state_change" => R::StateChange,
        "ci_activity" => R::CiActivity,
        "subscribed" => R::Subscribed,
        "security_alert" => R::SecurityAlert,
        other => R::Other(other.to_owned()),
    }
}

/// Notification thread ids are numeric.
pub(crate) fn thread_id(id: &NotificationId) -> Result<&str, ForgeError> {
    if !id.0.is_empty() && id.0.len() <= 32 && id.0.bytes().all(|b| b.is_ascii_digit()) {
        Ok(&id.0)
    } else {
        Err(ForgeError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_cannot_change_the_path() {
        for ok in ["acme", "rocket.rs", "my_repo-2"] {
            assert!(segment(ok).is_ok(), "{ok}");
        }
        for bad in ["", ".", "..", "a/b", "a?x=1", "a#b", "a b", "%2e%2e", "ä"] {
            assert!(segment(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn thread_ids_are_numeric() {
        assert!(thread_id(&NotificationId("123".into())).is_ok());
        assert!(thread_id(&NotificationId("../user".into())).is_err());
        assert!(thread_id(&NotificationId(String::new())).is_err());
    }
}
