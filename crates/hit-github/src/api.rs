//! Authenticated REST access. Every request goes through [`Api::send`], which
//! enforces the security rules in one place: the token only goes to this
//! account's API origin, only in the `Authorization` header (marked
//! sensitive), and read-only accounts cannot write.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use hit_auth::Secret;
use hit_core::{AccessHint, ForgeError, Health, RateLimit, Timestamp};
use jiff::SignedDuration;
use reqwest::{
    Client, Method, RequestBuilder, Response, StatusCode,
    header::{self, HeaderMap, HeaderValue},
};
use serde::{Deserialize, de::DeserializeOwned};
use url::Url;

use crate::GithubHost;

const API_VERSION: &str = "2022-11-28";
const MAX_MESSAGE: usize = 300;
const JSON: &str = "application/vnd.github+json";

#[derive(Deserialize)]
struct GraphqlReply<T> {
    data: Option<T>,
    errors: Option<Vec<GraphqlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    message: String,
    #[serde(rename = "type")]
    kind: Option<String>,
}

impl GraphqlError {
    fn into_forge_error(self) -> ForgeError {
        let message: String = self.message.chars().take(MAX_MESSAGE).collect();
        match self.kind.as_deref() {
            Some("NOT_FOUND") => ForgeError::NotFound,
            Some("FORBIDDEN") => ForgeError::Forbidden {
                message,
                hint: None,
            },
            Some("INSUFFICIENT_SCOPES") => ForgeError::Forbidden {
                message,
                hint: Some(AccessHint::TokenScope {
                    needed: "see the error message".to_owned(),
                }),
            },
            Some("RATE_LIMITED") => ForgeError::RateLimited { reset_at: None },
            _ => ForgeError::InvalidResponse(format!("graphql: {message}")),
        }
    }
}

async fn decode<T: DeserializeOwned>(response: Response, what: &str) -> Result<T, ForgeError> {
    response
        .json()
        .await
        .map_err(|e| ForgeError::InvalidResponse(format!("{what}: {}", e.without_url())))
}

/// The `page` of the `rel="next"` link in a `Link` header.
pub(crate) fn next_page(headers: &HeaderMap) -> Option<u32> {
    let link = headers.get(header::LINK)?.to_str().ok()?;
    link.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        if !params.contains("rel=\"next\"") {
            return None;
        }
        let target = target.trim().trim_start_matches('<').trim_end_matches('>');
        let url = Url::parse(target).ok()?;
        url.query_pairs()
            .find(|(k, _)| k == "page")
            .and_then(|(_, v)| v.parse().ok())
    })
}

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
    /// Read-only was turned on while hit runs; shared by clones.
    locked: Arc<AtomicBool>,
    standing: Arc<Mutex<Standing>>,
    /// Validators and bodies of recent conditional `GET`s, by URL.
    conditional: Arc<Mutex<HashMap<String, Stored>>>,
}

/// A rejected token is not tried again for this long. It stops pointless
/// requests, and the occasional try notices a token that works again.
const REJECTED_RECHECK: Duration = Duration::from_secs(10 * 60);
/// When a secondary limit does not say how long to wait.
const SECONDARY_LIMIT_WAIT: i64 = 60;

/// GitHub counts REST and GraphQL requests separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pool {
    Rest,
    Graphql,
}

/// What the responses so far say about this account.
#[derive(Debug, Default)]
struct Standing {
    rejected_at: Option<Instant>,
    rest: Option<RateLimit>,
    graphql: Option<RateLimit>,
    /// A secondary limit: nothing is sent until then.
    blocked_until: Option<Timestamp>,
}

impl Standing {
    fn pool(&self, pool: Pool) -> Option<RateLimit> {
        match pool {
            Pool::Rest => self.rest,
            Pool::Graphql => self.graphql,
        }
    }

    /// Why nothing should be sent right now.
    fn refusal(&self, pool: Pool, now: Timestamp) -> Option<ForgeError> {
        if self
            .rejected_at
            .is_some_and(|at| at.elapsed() < REJECTED_RECHECK)
        {
            return Some(ForgeError::Unauthorized);
        }
        if let Some(until) = self.blocked_until.filter(|until| *until > now) {
            return Some(ForgeError::RateLimited {
                reset_at: Some(until),
            });
        }
        self.pool(pool)
            .filter(RateLimit::is_exhausted)
            .and_then(|limit| limit.reset_at)
            .filter(|reset| *reset > now)
            .map(|reset| ForgeError::RateLimited {
                reset_at: Some(reset),
            })
    }

    fn observe(&mut self, pool: Pool, status: StatusCode, headers: &HeaderMap) {
        if status == StatusCode::UNAUTHORIZED {
            self.rejected_at = Some(Instant::now());
        } else if status.is_success() || status == StatusCode::NOT_MODIFIED {
            self.rejected_at = None;
        }
        let number = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
        };
        let (Some(remaining), Some(limit)) =
            (number("x-ratelimit-remaining"), number("x-ratelimit-limit"))
        else {
            return;
        };
        let rate = RateLimit {
            remaining: u32::try_from(remaining).unwrap_or(0),
            limit: u32::try_from(limit).unwrap_or(0),
            reset_at: number("x-ratelimit-reset").and_then(|s| Timestamp::from_second(s).ok()),
        };
        match pool {
            Pool::Rest => self.rest = Some(rate),
            Pool::Graphql => self.graphql = Some(rate),
        }
    }

    /// The tightest budget, once under a tenth of its size.
    fn low_rate_limit(&self, now: Timestamp) -> Option<RateLimit> {
        if let Some(until) = self.blocked_until.filter(|until| *until > now) {
            return Some(RateLimit {
                remaining: 0,
                limit: 0,
                reset_at: Some(until),
            });
        }
        [self.rest, self.graphql]
            .into_iter()
            .flatten()
            // Reset already: the numbers are old news.
            .filter(|rate| rate.reset_at.is_none_or(|reset| reset > now))
            .filter(|rate| u64::from(rate.remaining) * 10 < u64::from(rate.limit))
            .min_by_key(|rate| rate.remaining)
    }
}

/// Upper bound on remembered conditional responses.
const MAX_CONDITIONAL: usize = 64;

#[derive(Debug, Clone)]
struct Stored {
    etag: Option<HeaderValue>,
    last_modified: Option<HeaderValue>,
    body: Vec<u8>,
    next_page: Option<u32>,
    poll_interval: Option<Duration>,
}

/// The result of a conditional `GET`.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub body: Vec<u8>,
    pub next_page: Option<u32>,
    /// `X-Poll-Interval`: how often the server wants to be asked.
    pub poll_interval: Option<Duration>,
    /// The server answered `304`; `body` is the stored copy.
    pub not_modified: bool,
}

impl Fetched {
    pub fn json<T: DeserializeOwned>(&self, what: &str) -> Result<T, ForgeError> {
        serde_json::from_slice(&self.body)
            .map_err(|e| ForgeError::InvalidResponse(format!("{what}: {e}")))
    }
}

fn poll_interval(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get("x-poll-interval")?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()
        .map(|s| Duration::from_secs(s.clamp(1, 3600)))
}

impl Api {
    pub fn new(http: Client, host: GithubHost, token: Secret, options: ApiOptions) -> Self {
        Self {
            http,
            host,
            token,
            options,
            locked: Arc::default(),
            standing: Arc::default(),
            conditional: Arc::default(),
        }
    }

    pub fn host(&self) -> &GithubHost {
        &self.host
    }

    pub fn options(&self) -> ApiOptions {
        ApiOptions {
            read_only: self.options.read_only || self.locked.load(Ordering::Relaxed),
            ..self.options
        }
    }

    pub fn health(&self) -> Health {
        let standing = self.lock_standing();
        Health {
            token_rejected: standing.rejected_at.is_some(),
            rate_limit: standing.low_rate_limit(Timestamp::now()),
        }
    }

    fn lock_standing(&self) -> MutexGuard<'_, Standing> {
        self.standing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends `request` unless the forge has already said not to, and learns
    /// from the answer. Every request goes through here.
    async fn dispatch(&self, pool: Pool, request: RequestBuilder) -> Result<Response, ForgeError> {
        if let Some(refusal) = self.lock_standing().refusal(pool, Timestamp::now()) {
            return Err(refusal);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ForgeError::Network(e.without_url().to_string()))?;
        self.lock_standing()
            .observe(pool, response.status(), response.headers());
        Ok(response)
    }

    /// Remembers a limit the forge only reported as an error.
    fn note_limit(&self, error: ForgeError) -> ForgeError {
        let ForgeError::RateLimited { reset_at } = error else {
            return error;
        };
        let until = reset_at.or_else(|| {
            Timestamp::now()
                .checked_add(SignedDuration::from_secs(SECONDARY_LIMIT_WAIT))
                .ok()
        });
        let mut standing = self.lock_standing();
        standing.blocked_until = until;
        ForgeError::RateLimited { reset_at: until }
    }

    /// Refuses writes from now on; see [`hit_core::Forge::lock_read_only`].
    pub fn lock_read_only(&self) {
        self.locked.store(true, Ordering::Relaxed);
    }

    /// Key for encrypting this account's local cache, derived from the token
    /// (never the token itself). Changes when the token changes.
    pub(crate) fn local_key(&self) -> zeroize::Zeroizing<[u8; 32]> {
        local_key(&self.token, &self.host)
    }
}

/// The local-cache key for an account on `host`, derived from its token.
pub fn local_key(token: &Secret, host: &GithubHost) -> zeroize::Zeroizing<[u8; 32]> {
    zeroize::Zeroizing::new(token.derive_key(&format!("hit/local-cache/v1/{}", host.host())))
}

impl Api {
    pub async fn get(&self, path: &str) -> Result<Response, ForgeError> {
        self.send(Method::GET, path, Op::Read).await
    }

    /// `GET` and decode JSON.
    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, ForgeError> {
        decode(self.get(path).await?, path).await
    }

    /// Sends a request to `path`, relative to the REST base.
    pub async fn send(&self, method: Method, path: &str, op: Op) -> Result<Response, ForgeError> {
        guard(&method, op, self.options())?;
        let url = self.url(path)?;
        self.execute(method, url, JSON, None).await
    }

    /// Like [`Self::send`] with a JSON body.
    pub async fn send_json(
        &self,
        method: Method,
        path: &str,
        op: Op,
        body: &serde_json::Value,
    ) -> Result<Response, ForgeError> {
        guard(&method, op, self.options())?;
        let url = self.url(path)?;
        self.execute(method, url, JSON, Some(body)).await
    }

    /// `GET` with a custom media type (e.g. a diff), capped at `max_bytes`.
    pub async fn get_text(
        &self,
        path: &str,
        accept: &'static str,
        max_bytes: usize,
    ) -> Result<String, ForgeError> {
        let url = self.url(path)?;
        let response = self.execute(Method::GET, url, accept, None).await?;
        if response
            .content_length()
            .is_some_and(|len| len > max_bytes as u64)
        {
            return Err(ForgeError::InvalidResponse(format!(
                "{path}: response is larger than {max_bytes} bytes"
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| ForgeError::Network(e.without_url().to_string()))?;
        if bytes.len() > max_bytes {
            return Err(ForgeError::InvalidResponse(format!(
                "{path}: response is larger than {max_bytes} bytes"
            )));
        }
        String::from_utf8(bytes.to_vec())
            .map_err(|_| ForgeError::InvalidResponse(format!("{path}: response is not UTF-8")))
    }

    /// Runs a GraphQL query. Mutations are refused for read-only accounts;
    /// queries are reads even though they are sent with `POST`.
    pub async fn graphql<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T, ForgeError> {
        guard_graphql(query, self.options())?;
        let url = self.host.graphql().clone();
        let body = serde_json::json!({ "query": query, "variables": variables });
        let response = self.execute(Method::POST, url, JSON, Some(&body)).await?;
        let reply: GraphqlReply<T> = decode(response, "graphql").await?;
        match (reply.data, reply.errors.unwrap_or_default()) {
            (Some(data), errors) => {
                for error in &errors {
                    // Partial results, e.g. orgs that need SSO; keep the data.
                    tracing::warn!(message = %error.message, kind = ?error.kind, "graphql partial error");
                }
                Ok(data)
            }
            (None, errors) => Err(self.note_limit(
                errors
                    .into_iter()
                    .next()
                    .map(GraphqlError::into_forge_error)
                    .unwrap_or_else(|| {
                        ForgeError::InvalidResponse("graphql: no data and no errors".to_owned())
                    }),
            )),
        }
    }

    /// `GET` that revalidates with `If-None-Match` / `If-Modified-Since`.
    /// A `304` (which GitHub does not count against the rate limit) returns
    /// the body stored from the previous response.
    pub async fn get_conditional(&self, path: &str) -> Result<Fetched, ForgeError> {
        let url = self.url(path)?;
        let key = url.to_string();
        let stored = self.lock_conditional().get(&key).cloned();

        let mut request = self.request(Method::GET, url, JSON)?;
        if let Some(stored) = &stored {
            if let Some(etag) = &stored.etag {
                request = request.header(header::IF_NONE_MATCH, etag.clone());
            }
            if let Some(modified) = &stored.last_modified {
                request = request.header(header::IF_MODIFIED_SINCE, modified.clone());
            }
        }
        let response = self.dispatch(Pool::Rest, request).await?;

        if response.status() == StatusCode::NOT_MODIFIED
            && let Some(stored) = stored
        {
            return Ok(Fetched {
                // GitHub may raise the poll interval under load; prefer the new one.
                poll_interval: poll_interval(response.headers()).or(stored.poll_interval),
                body: stored.body,
                next_page: stored.next_page,
                not_modified: true,
            });
        }

        let response = check(response).await.map_err(|e| self.note_limit(e))?;
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .map_err(|e| ForgeError::Network(e.without_url().to_string()))?
            .to_vec();
        let fetched = Fetched {
            body,
            next_page: next_page(&headers),
            poll_interval: poll_interval(&headers),
            not_modified: false,
        };
        let etag = headers.get(header::ETAG).cloned();
        let last_modified = headers.get(header::LAST_MODIFIED).cloned();
        if etag.is_some() || last_modified.is_some() {
            let mut map = self.lock_conditional();
            if map.len() >= MAX_CONDITIONAL && !map.contains_key(&key) {
                map.clear();
            }
            map.insert(
                key,
                Stored {
                    etag,
                    last_modified,
                    body: fetched.body.clone(),
                    next_page: fetched.next_page,
                    poll_interval: fetched.poll_interval,
                },
            );
        }
        Ok(fetched)
    }

    fn lock_conditional(&self) -> MutexGuard<'_, HashMap<String, Stored>> {
        self.conditional
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn request(
        &self,
        method: Method,
        url: Url,
        accept: &'static str,
    ) -> Result<RequestBuilder, ForgeError> {
        let mut auth = HeaderValue::from_str(&format!("Bearer {}", self.token.expose()))
            .map_err(|_| ForgeError::Unauthorized)?;
        auth.set_sensitive(true);
        Ok(self
            .http
            .request(method, url)
            .header(header::AUTHORIZATION, auth)
            .header(header::ACCEPT, accept)
            .header("X-GitHub-Api-Version", API_VERSION))
    }

    async fn execute(
        &self,
        method: Method,
        url: Url,
        accept: &'static str,
        body: Option<&serde_json::Value>,
    ) -> Result<Response, ForgeError> {
        let pool = if url == *self.host.graphql() {
            Pool::Graphql
        } else {
            Pool::Rest
        };
        let mut request = self.request(method, url, accept)?;
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = self.dispatch(pool, request).await?;
        check(response).await.map_err(|e| self.note_limit(e))
    }

    /// Joins `path` onto the API base and refuses anything that would leave
    /// the API origin (absolute URLs, `//host`, `..`).
    fn url(&self, path: &str) -> Result<Url, ForgeError> {
        let bad = || ForgeError::InvalidResponse(format!("refusing request path {path:?}"));
        if path.starts_with('/') || path.contains("://") || path.contains('\\') {
            return Err(bad());
        }
        // `..` stays inside the API origin but reaches another endpoint.
        let segments = path.split(['?', '#']).next().unwrap_or_default().split('/');
        if segments
            .into_iter()
            .any(|segment| matches!(segment, "" | "." | ".."))
        {
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
        .and_then(|json| error_message(&json))
        .map(|m| m.chars().take(MAX_MESSAGE).collect::<String>())
        .unwrap_or_else(|| status.to_string());
    Err(map_error(status, &headers, message))
}

/// GitHub's reason: `message`, and for 422s the more useful first detail.
fn error_message(json: &serde_json::Value) -> Option<String> {
    let message = json.get("message")?.as_str()?;
    let detail = json
        .get("errors")
        .and_then(|errors| errors.get(0))
        .and_then(|first| first.get("message").or(Some(first)))
        .and_then(serde_json::Value::as_str);
    Some(match detail {
        Some(detail) => format!("{message}: {detail}"),
        None => message.to_owned(),
    })
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
                let wait = header("retry-after")
                    .and_then(|v| v.parse::<i64>().ok())
                    .and_then(|s| {
                        Timestamp::now()
                            .checked_add(SignedDuration::from_secs(s))
                            .ok()
                    });
                let reset = header("x-ratelimit-reset")
                    .and_then(|v| v.parse::<i64>().ok())
                    .and_then(|s| Timestamp::from_second(s).ok());
                return ForgeError::RateLimited {
                    reset_at: wait.or(reset),
                };
            }
            // GitHub's abuse protection answers a plain 403 sometimes.
            if message.to_lowercase().contains("secondary rate limit") {
                return ForgeError::RateLimited { reset_at: None };
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
        StatusCode::METHOD_NOT_ALLOWED
        | StatusCode::CONFLICT
        | StatusCode::UNPROCESSABLE_ENTITY => ForgeError::Rejected(message),
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => {
            ForgeError::Unavailable(status.to_string())
        }
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

    fn limited(remaining: &str, reset: i64) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({}))
            .insert_header("x-ratelimit-limit", "5000")
            .insert_header("x-ratelimit-remaining", remaining)
            .insert_header("x-ratelimit-reset", reset.to_string().as_str())
    }

    fn in_an_hour() -> i64 {
        Timestamp::now().as_second() + 3600
    }

    #[tokio::test]
    async fn a_path_cannot_reach_another_endpoint() {
        let server = MockServer::start().await;
        let api = api(&server, ApiOptions::default()).await;
        for path in [
            "repos/a/b/git/refs/heads/../../x",
            "repos/a/../../user",
            "..",
            "./user",
            "repos//a",
            "repos/a/",
        ] {
            assert!(
                matches!(
                    api.send(Method::GET, path, Op::Read).await,
                    Err(ForgeError::InvalidResponse(_))
                ),
                "{path}"
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());

        // A query may hold anything.
        assert!(api.url("search?q=a/../b").is_ok());
        assert!(api.url("repos/a.b/c..d/pulls").is_ok());
    }

    #[tokio::test]
    async fn a_rejected_token_stops_the_requests() {
        let server = MockServer::start().await;
        Mock::given(path("/api/user"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let api = api(&server, ApiOptions::default()).await;
        assert!(!api.health().token_rejected);

        assert_eq!(api.get("user").await.unwrap_err(), ForgeError::Unauthorized);
        assert!(api.health().token_rejected);
        for _ in 0..3 {
            assert_eq!(api.get("user").await.unwrap_err(), ForgeError::Unauthorized);
        }
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "only the first one reached GitHub"
        );
    }

    #[tokio::test]
    async fn a_rejected_token_is_tried_again_later_and_can_recover() {
        let server = MockServer::start().await;
        Mock::given(path("/api/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        let api = api(&server, ApiOptions::default()).await;
        api.lock_standing().rejected_at = Instant::now().checked_sub(REJECTED_RECHECK);

        api.get("user").await.unwrap();

        assert!(!api.health().token_rejected);
    }

    #[tokio::test]
    async fn an_exhausted_budget_waits_for_its_reset() {
        let server = MockServer::start().await;
        Mock::given(path("/api/rest"))
            .respond_with(limited("0", in_an_hour()))
            .mount(&server)
            .await;
        Mock::given(path("/api/graphql"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "data": {} }))
                    .insert_header("x-ratelimit-limit", "5000")
                    .insert_header("x-ratelimit-remaining", "4000"),
            )
            .mount(&server)
            .await;
        let api = api(&server, ApiOptions::default()).await;

        api.get("rest").await.unwrap();
        assert!(matches!(
            api.get("rest").await.unwrap_err(),
            ForgeError::RateLimited { reset_at: Some(_) }
        ));
        let requests = server.received_requests().await.unwrap().len();
        assert_eq!(requests, 1, "the second one was not sent");
        assert!(api.health().rate_limit.unwrap().is_exhausted());

        // GraphQL has its own budget.
        api.graphql::<serde_json::Value>("query { a }", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_budget_that_has_reset_is_used_again() {
        let server = MockServer::start().await;
        Mock::given(path("/api/rest"))
            .respond_with(limited("0", Timestamp::now().as_second() - 10))
            .mount(&server)
            .await;
        let api = api(&server, ApiOptions::default()).await;
        api.get("rest").await.unwrap();
        api.get("rest").await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        assert_eq!(api.health().rate_limit, None);
    }

    #[tokio::test]
    async fn the_limit_is_only_mentioned_when_low() {
        let server = MockServer::start().await;
        Mock::given(path("/api/plenty"))
            .respond_with(limited("501", in_an_hour()))
            .mount(&server)
            .await;
        Mock::given(path("/api/low"))
            .respond_with(limited("499", in_an_hour()))
            .mount(&server)
            .await;
        let api = api(&server, ApiOptions::default()).await;
        api.get("plenty").await.unwrap();
        assert_eq!(api.health().rate_limit, None);
        api.get("low").await.unwrap();
        assert_eq!(api.health().rate_limit.unwrap().remaining, 499);
    }

    #[tokio::test]
    async fn a_secondary_limit_pauses_everything() {
        let server = MockServer::start().await;
        Mock::given(path("/api/busy"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "message": "You have exceeded a secondary rate limit."
            })))
            .mount(&server)
            .await;
        let api = api(&server, ApiOptions::default()).await;
        assert!(matches!(
            api.get("busy").await.unwrap_err(),
            ForgeError::RateLimited { reset_at: Some(_) }
        ));
        assert!(matches!(
            api.graphql::<serde_json::Value>("query { a }", serde_json::json!({}))
                .await
                .unwrap_err(),
            ForgeError::RateLimited { .. }
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn turning_read_only_on_works_right_away() {
        let server = MockServer::start().await;
        let api = api(&server, ApiOptions::default()).await;
        let clone = api.clone();
        assert!(!api.options().read_only);

        clone.lock_read_only();

        assert!(api.options().read_only);
        let err = api.send(Method::POST, "repos/a/b/issues", Op::Write).await;
        assert!(matches!(err, Err(ForgeError::ReadOnly(_))));
        let err = api
            .graphql::<serde_json::Value>("mutation { x }", serde_json::json!({}))
            .await;
        assert!(matches!(err, Err(ForgeError::ReadOnly(_))));
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
        // A new Api for each: errors are remembered, see `Standing`.
        let fresh = || api(&server, ApiOptions::default());

        assert_eq!(
            fresh().await.get("unauth").await.unwrap_err(),
            ForgeError::Unauthorized
        );
        assert!(matches!(
            fresh().await.get("limited").await.unwrap_err(),
            ForgeError::RateLimited { reset_at: Some(_) }
        ));
        assert!(matches!(
            fresh().await.get("sso").await.unwrap_err(),
            ForgeError::Forbidden {
                hint: Some(AccessHint::SsoRequired { .. }),
                ..
            }
        ));
        assert_eq!(
            fresh().await.get("missing").await.unwrap_err(),
            ForgeError::NotFound
        );
    }
}
