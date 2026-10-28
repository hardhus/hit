//! Authenticated REST access. Every request goes through [`Api::send`], which
//! enforces the security rules in one place: the token only goes to this
//! account's API origin, only in the `Authorization` header (marked
//! sensitive), and read-only accounts cannot write.

use hit_auth::Secret;
use hit_core::{AccessHint, ForgeError, Timestamp};
use reqwest::{
    Client, Method, Response, StatusCode,
    header::{self, HeaderMap, HeaderValue},
};
use url::Url;

use crate::GithubHost;

const API_VERSION: &str = "2022-11-28";
const MAX_MESSAGE: usize = 300;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApiOptions {
    /// Block every write.
    pub read_only: bool,
    /// In read-only mode, still allow marking notifications read (the user's
    /// own inbox state). Off unless the user opts in.
    pub allow_mark_read: bool,
}

/// What a request does, for the read-only guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Read,
    MarkRead,
    Write,
}

#[derive(Debug, Clone)]
pub struct Api {
    http: Client,
    host: GithubHost,
    token: Secret,
    options: ApiOptions,
}

impl Api {
    pub fn new(http: Client, host: GithubHost, token: Secret, options: ApiOptions) -> Self {
        Self {
            http,
            host,
            token,
            options,
        }
    }

    pub fn host(&self) -> &GithubHost {
        &self.host
    }

    pub fn options(&self) -> ApiOptions {
        self.options
    }

    pub async fn get(&self, path: &str) -> Result<Response, ForgeError> {
        self.send(Method::GET, path, Op::Read).await
    }

    /// Sends a request to `path`, relative to the REST base.
    pub async fn send(&self, method: Method, path: &str, op: Op) -> Result<Response, ForgeError> {
        guard(&method, op, self.options)?;
        let url = self.url(path)?;
        let mut auth = HeaderValue::from_str(&format!("Bearer {}", self.token.expose()))
            .map_err(|_| ForgeError::Unauthorized)?;
        auth.set_sensitive(true);

        let response = self
            .http
            .request(method, url)
            .header(header::AUTHORIZATION, auth)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .send()
            .await
            .map_err(|e| ForgeError::Network(e.without_url().to_string()))?;
        check(response).await
    }

    /// Joins `path` onto the API base and refuses anything that would leave
    /// the API origin (absolute URLs, `//host`, `..`).
    fn url(&self, path: &str) -> Result<Url, ForgeError> {
        let bad = || ForgeError::InvalidResponse(format!("refusing request path {path:?}"));
        if path.starts_with('/') || path.contains("://") || path.contains('\\') {
            return Err(bad());
        }
        let url = self.host.api().join(path).map_err(|_| bad())?;
        let base = self.host.api();
        let same_origin = url.scheme() == base.scheme()
            && url.host_str() == base.host_str()
            && url.port_or_known_default() == base.port_or_known_default()
            && url.path().starts_with(base.path());
        if same_origin { Ok(url) } else { Err(bad()) }
    }
}

fn guard(method: &Method, op: Op, options: ApiOptions) -> Result<(), ForgeError> {
    let reading = matches!(*method, Method::GET | Method::HEAD) && op == Op::Read;
    if reading || !options.read_only {
        return Ok(());
    }
    if op == Op::MarkRead && options.allow_mark_read {
        return Ok(());
    }
    Err(ForgeError::ReadOnly(format!("{method} request")))
}

/// Blocks GraphQL mutations for read-only accounts. Deliberately
/// conservative: any `mutation` keyword outside comments counts.
pub fn guard_graphql(query: &str, options: ApiOptions) -> Result<(), ForgeError> {
    if !options.read_only {
        return Ok(());
    }
    let has_mutation = query
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .flat_map(|line| line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')))
        .any(|word| word == "mutation");
    if has_mutation {
        Err(ForgeError::ReadOnly("GraphQL mutation".to_owned()))
    } else {
        Ok(())
    }
}

/// Maps unsuccessful responses to [`ForgeError`].
async fn check(response: Response) -> Result<Response, ForgeError> {
    let status = response.status();
    if status.is_success() || status == StatusCode::NOT_MODIFIED {
        return Ok(response);
    }
    let headers = response.headers().clone();
    let message = response
        .text()
        .await
        .ok()
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .and_then(|json| json.get("message")?.as_str().map(str::to_owned))
        .map(|m| m.chars().take(MAX_MESSAGE).collect::<String>())
        .unwrap_or_else(|| status.to_string());
    Err(map_error(status, &headers, message))
}

fn map_error(status: StatusCode, headers: &HeaderMap, message: String) -> ForgeError {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    match status {
        StatusCode::UNAUTHORIZED => ForgeError::Unauthorized,
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
            let exhausted = header("x-ratelimit-remaining") == Some("0");
            if exhausted
                || header("retry-after").is_some()
                || status == StatusCode::TOO_MANY_REQUESTS
            {
                let reset_at = header("x-ratelimit-reset")
                    .and_then(|v| v.parse::<i64>().ok())
                    .and_then(|s| Timestamp::from_second(s).ok());
                return ForgeError::RateLimited { reset_at };
            }
            if let Some(hint) = header("x-github-sso").and_then(parse_sso) {
                return ForgeError::Forbidden {
                    message,
                    hint: Some(hint),
                };
            }
            let hint = header("x-accepted-oauth-scopes")
                .filter(|s| !s.trim().is_empty())
                .map(|needed| AccessHint::TokenScope {
                    needed: needed.trim().to_owned(),
                });
            ForgeError::Forbidden { message, hint }
        }
        StatusCode::NOT_FOUND => ForgeError::NotFound,
        other => ForgeError::InvalidResponse(format!("HTTP {other}: {message}")),
    }
}

/// Parses `X-GitHub-SSO`: `required; url=…` or `partial-results; organizations=1,2`.
pub(crate) fn parse_sso(value: &str) -> Option<AccessHint> {
    let mut parts = value.split(';').map(str::trim);
    match parts.next()? {
        "required" => {
            let url = parts.find_map(|p| p.strip_prefix("url="))?;
            Some(AccessHint::SsoRequired {
                url: url.to_owned(),
            })
        }
        "partial-results" => {
            let ids = parts.find_map(|p| p.strip_prefix("organizations="))?;
            Some(AccessHint::SsoPartial {
                org_ids: ids
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header as has_header, method, path},
    };

    use super::*;

    const RO: ApiOptions = ApiOptions {
        read_only: true,
        allow_mark_read: false,
    };

    async fn api(server: &MockServer, options: ApiOptions) -> Api {
        let base = Url::parse(&server.uri()).unwrap();
        Api::new(
            hit_auth::client().unwrap(),
            GithubHost::for_tests("github.com", &base),
            Secret::new("gho_secret_value").unwrap(),
            options,
        )
    }

    #[test]
    fn read_only_guard() {
        assert!(guard(&Method::GET, Op::Read, RO).is_ok());
        assert!(guard(&Method::POST, Op::Write, RO).is_err());
        assert!(guard(&Method::GET, Op::Write, RO).is_err());
        assert!(guard(&Method::PATCH, Op::MarkRead, RO).is_err());
        let allow = ApiOptions {
            allow_mark_read: true,
            ..RO
        };
        assert!(guard(&Method::PATCH, Op::MarkRead, allow).is_ok());
        assert!(guard(&Method::POST, Op::Write, allow).is_err());
        assert!(guard(&Method::POST, Op::Write, ApiOptions::default()).is_ok());
    }

    #[test]
    fn graphql_mutations_are_blocked_when_read_only() {
        assert!(guard_graphql("query { viewer { login } }", RO).is_ok());
        assert!(guard_graphql("# mutation in a comment\nquery { a }", RO).is_ok());
        assert!(guard_graphql("mutation { addComment(x: 1) { id } }", RO).is_err());
        assert!(guard_graphql("query A { a }\n  mutation B { b }", RO).is_err());
        assert!(guard_graphql("mutation { a }", ApiOptions::default()).is_ok());
    }

    #[test]
    fn sso_header_parsing() {
        assert_eq!(
            parse_sso("required; url=https://github.com/orgs/acme/sso?authorization_request=x"),
            Some(AccessHint::SsoRequired {
                url: "https://github.com/orgs/acme/sso?authorization_request=x".into()
            })
        );
        assert_eq!(
            parse_sso("partial-results; organizations=21955855,20582480"),
            Some(AccessHint::SsoPartial {
                org_ids: vec!["21955855".into(), "20582480".into()]
            })
        );
        assert_eq!(parse_sso("whatever"), None);
    }

    #[tokio::test]
    async fn token_is_sent_only_as_bearer_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/user"))
            .and(has_header("authorization", "Bearer gho_secret_value"))
            .and(has_header("x-github-api-version", API_VERSION))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let api = api(&server, ApiOptions::default()).await;
        api.get("user").await.unwrap();
        let request = &server.received_requests().await.unwrap()[0];
        assert!(!request.url.as_str().contains("gho_"));
        assert!(!format!("{api:?}").contains("gho_secret_value"));
    }

    #[tokio::test]
    async fn refuses_paths_that_leave_the_api() {
        let server = MockServer::start().await;
        let api = api(&server, ApiOptions::default()).await;
        for bad in [
            "https://evil.example/x",
            "//evil.example/x",
            "/user",
            "../outside",
            "..\\outside",
        ] {
            assert!(api.get(bad).await.is_err(), "{bad}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn read_only_blocks_before_sending() {
        let server = MockServer::start().await;
        let api = api(&server, RO).await;
        let err = api.send(Method::POST, "repos/a/b/issues", Op::Write).await;
        assert!(matches!(err, Err(ForgeError::ReadOnly(_))));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn maps_error_responses() {
        let server = MockServer::start().await;
        Mock::given(path("/api/unauth"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        Mock::given(path("/api/limited"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-ratelimit-remaining", "0")
                    .insert_header("x-ratelimit-reset", "1900000000"),
            )
            .mount(&server)
            .await;
        Mock::given(path("/api/sso"))
            .respond_with(
                ResponseTemplate::new(403)
                    .insert_header("x-github-sso", "required; url=https://github.com/orgs/acme/sso")
                    .set_body_json(serde_json::json!({ "message": "Resource protected by organization SAML enforcement." })),
            )
            .mount(&server)
            .await;
        Mock::given(path("/api/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let api = api(&server, ApiOptions::default()).await;

        assert_eq!(
            api.get("unauth").await.unwrap_err(),
            ForgeError::Unauthorized
        );
        assert!(matches!(
            api.get("limited").await.unwrap_err(),
            ForgeError::RateLimited { reset_at: Some(_) }
        ));
        assert!(matches!(
            api.get("sso").await.unwrap_err(),
            ForgeError::Forbidden {
                hint: Some(AccessHint::SsoRequired { .. }),
                ..
            }
        ));
        assert_eq!(api.get("missing").await.unwrap_err(), ForgeError::NotFound);
    }
}
