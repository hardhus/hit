//! Who a token belongs to and what it can do.

use hit_auth::{Secret, TokenKind};
use hit_core::ForgeError;
use serde::Deserialize;

use crate::Api;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub login: String,
    pub name: Option<String>,
    /// From `X-OAuth-Scopes`; `None` for tokens without classic scopes
    /// (fine-grained or app tokens).
    pub scopes: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct UserResponse {
    login: String,
    name: Option<String>,
}

/// Calls `GET /user`.
pub async fn verify(api: &Api) -> Result<Viewer, ForgeError> {
    let response = api.get("user").await?;
    let scopes = response
        .headers()
        .get("x-oauth-scopes")
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        });
    let user: UserResponse = response
        .json()
        .await
        .map_err(|e| ForgeError::InvalidResponse(format!("GET /user: {e}")))?;
    if !is_valid_login(&user.login) {
        return Err(ForgeError::InvalidResponse(
            "GET /user: invalid login".to_owned(),
        ));
    }
    Ok(Viewer {
        login: user.login,
        name: user.name,
        scopes,
    })
}

/// GitHub logins: alphanumerics and single hyphens (bots end in `[bot]`).
/// Validated because logins end up in file names and keyring entries.
fn is_valid_login(login: &str) -> bool {
    let base = login.strip_suffix("[bot]").unwrap_or(login);
    (1..=39).contains(&base.len())
        && !base.starts_with('-')
        && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Whether the notifications API works with this token (fine-grained tokens
/// may not be accepted).
pub async fn notifications_access(api: &Api) -> Result<(), ForgeError> {
    api.get("notifications?per_page=1").await.map(|_| ())
}

/// Best-effort token type from GitHub's documented prefixes.
pub fn token_kind(token: &Secret) -> TokenKind {
    let value = token.expose();
    if value.starts_with("gho_") {
        TokenKind::OAuth
    } else if value.starts_with("ghp_") {
        TokenKind::Classic
    } else if value.starts_with("github_pat_") {
        TokenKind::FineGrained
    } else {
        TokenKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use url::Url;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;
    use crate::{ApiOptions, GithubHost};

    async fn api(server: &MockServer) -> Api {
        let base = Url::parse(&server.uri()).unwrap();
        Api::new(
            hit_auth::client().unwrap(),
            GithubHost::for_tests("github.com", &base),
            Secret::new("gho_x").unwrap(),
            ApiOptions::default(),
        )
    }

    #[tokio::test]
    async fn reads_login_and_scopes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/user"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-oauth-scopes", "repo, read:org, notifications")
                    .set_body_json(serde_json::json!({ "login": "octocat", "name": "Mona" })),
            )
            .mount(&server)
            .await;
        let viewer = verify(&api(&server).await).await.unwrap();
        assert_eq!(viewer.login, "octocat");
        assert_eq!(
            viewer.scopes.unwrap(),
            ["repo", "read:org", "notifications"]
        );
    }

    #[tokio::test]
    async fn rejects_weird_logins() {
        let server = MockServer::start().await;
        Mock::given(path("/api/user"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "login": "../../etc" })),
            )
            .mount(&server)
            .await;
        assert!(verify(&api(&server).await).await.is_err());
    }

    #[test]
    fn token_kinds() {
        let kind = |t: &str| token_kind(&Secret::new(t).unwrap());
        assert_eq!(kind("gho_abc"), TokenKind::OAuth);
        assert_eq!(kind("ghp_abc"), TokenKind::Classic);
        assert_eq!(kind("github_pat_abc"), TokenKind::FineGrained);
        assert_eq!(kind("abc"), TokenKind::Unknown);
    }

    #[test]
    fn login_validation() {
        assert!(is_valid_login("octo-cat"));
        assert!(is_valid_login("dependabot[bot]"));
        assert!(!is_valid_login(""));
        assert!(!is_valid_login("-x"));
        assert!(!is_valid_login("a/b"));
    }
}
