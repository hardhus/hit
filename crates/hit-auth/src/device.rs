//! OAuth 2.0 Device Authorization Grant (RFC 8628) client.
//!
//! Forge-agnostic: endpoints are passed in. Polling honors `interval` and
//! `slow_down`, and never outlives `expires_in`.

use std::time::Duration;

use reqwest::{Client, header};
use serde::Deserialize;
use tokio::time::Instant;
use url::Url;

use crate::Secret;

const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Debug, Clone)]
pub struct DeviceEndpoints {
    pub code_url: Url,
    pub token_url: Url,
    pub client_id: String,
    pub scopes: Vec<String>,
}

/// The device authorization. `device_code` can be exchanged for a token, so
/// it is kept as a [`Secret`].
#[derive(Debug, Clone)]
pub struct DeviceAuthorization {
    pub device_code: Secret,
    pub user_code: String,
    pub verification_uri: Url,
    pub expires_in: Duration,
    pub interval: Duration,
}

/// The granted token. No `Debug` derive on purpose beyond the redacted secrets.
#[derive(Debug, Clone)]
pub struct TokenGrant {
    pub access_token: Secret,
    pub scopes: Vec<String>,
    pub refresh_token: Option<Secret>,
    pub expires_in: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeviceError {
    #[error("the user denied access")]
    Denied,
    #[error("the device code expired")]
    Expired,
    #[error("device flow is not enabled for this app")]
    Disabled,
    #[error("the app's client id was rejected")]
    BadClient,
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    Protocol(String),
}

/// Polling knobs; the defaults follow RFC 8628.
#[derive(Debug, Clone, Copy)]
pub struct PollOptions {
    /// Added to the interval on `slow_down`.
    pub slow_down_step: Duration,
    /// Lower bound for the interval the server asks for.
    pub min_interval: Duration,
}

impl Default for PollOptions {
    fn default() -> Self {
        Self {
            slow_down_step: Duration::from_secs(5),
            min_interval: Duration::from_secs(1),
        }
    }
}

#[derive(Deserialize)]
struct CodeResponse {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
    error: Option<String>,
}

/// Fields are read once and moved into [`Secret`]s; the struct itself is
/// never logged.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    scope: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
    interval: Option<u64>,
}

fn network(e: &reqwest::Error) -> DeviceError {
    // reqwest errors carry the URL, never request bodies or headers.
    DeviceError::Network(e.to_string())
}

fn known_error(code: &str) -> Option<DeviceError> {
    match code {
        "access_denied" => Some(DeviceError::Denied),
        "expired_token" => Some(DeviceError::Expired),
        "device_flow_disabled" => Some(DeviceError::Disabled),
        "incorrect_client_credentials" | "invalid_client" | "unauthorized_client" => {
            Some(DeviceError::BadClient)
        }
        _ => None,
    }
}

/// Step 1: asks for a device and user code.
pub async fn request_code(
    http: &Client,
    endpoints: &DeviceEndpoints,
) -> Result<DeviceAuthorization, DeviceError> {
    let scope = endpoints.scopes.join(" ");
    let response = http
        .post(endpoints.code_url.clone())
        .header(header::ACCEPT, "application/json")
        .form(&[
            ("client_id", endpoints.client_id.as_str()),
            ("scope", scope.as_str()),
        ])
        .send()
        .await
        .map_err(|e| network(&e))?;
    let status = response.status();
    let body: CodeResponse = response
        .json()
        .await
        .map_err(|_| DeviceError::Protocol(format!("code endpoint answered {status}")))?;

    if let Some(code) = body.error.as_deref() {
        return Err(known_error(code)
            .unwrap_or_else(|| DeviceError::Protocol(format!("code endpoint error {code:?}"))));
    }
    let missing = |field: &str| DeviceError::Protocol(format!("code response lacks {field}"));
    let device_code = body.device_code.ok_or_else(|| missing("device_code"))?;
    let user_code = body.user_code.ok_or_else(|| missing("user_code"))?;
    let verification_uri = body
        .verification_uri
        .ok_or_else(|| missing("verification_uri"))?;
    let verification_uri = Url::parse(&verification_uri)
        .map_err(|_| DeviceError::Protocol("verification_uri is not a URL".to_owned()))?;

    Ok(DeviceAuthorization {
        device_code: Secret::new(device_code)
            .map_err(|_| DeviceError::Protocol("invalid device_code".to_owned()))?,
        user_code,
        verification_uri,
        expires_in: Duration::from_secs(body.expires_in.unwrap_or(900).min(3600)),
        interval: Duration::from_secs(body.interval.unwrap_or(5)),
    })
}

/// Step 2: polls until the user approves, denies, or the code expires.
/// Cancel by dropping the future.
pub async fn poll_token(
    http: &Client,
    endpoints: &DeviceEndpoints,
    authorization: &DeviceAuthorization,
    options: PollOptions,
) -> Result<TokenGrant, DeviceError> {
    let deadline = Instant::now() + authorization.expires_in;
    let mut interval = authorization.interval.max(options.min_interval);

    loop {
        if Instant::now() + interval >= deadline {
            return Err(DeviceError::Expired);
        }
        tokio::time::sleep(interval).await;

        let response = http
            .post(endpoints.token_url.clone())
            .header(header::ACCEPT, "application/json")
            .form(&[
                ("client_id", endpoints.client_id.as_str()),
                ("device_code", authorization.device_code.expose()),
                ("grant_type", GRANT_TYPE),
            ])
            .send()
            .await
            .map_err(|e| network(&e))?;
        let status = response.status();
        let body: TokenResponse = response
            .json()
            .await
            .map_err(|_| DeviceError::Protocol(format!("token endpoint answered {status}")))?;

        match body.error.as_deref() {
            None => {}
            Some("authorization_pending") => continue,
            Some("slow_down") => {
                let asked = Duration::from_secs(body.interval.unwrap_or(0));
                interval = asked.max(interval + options.slow_down_step);
                continue;
            }
            Some(code) => {
                return Err(known_error(code).unwrap_or_else(|| {
                    DeviceError::Protocol(format!("token endpoint error {code:?}"))
                }));
            }
        }

        let access_token = body
            .access_token
            .ok_or_else(|| DeviceError::Protocol("token response lacks access_token".to_owned()))?;
        return Ok(TokenGrant {
            access_token: Secret::new(access_token)
                .map_err(|_| DeviceError::Protocol("invalid access_token".to_owned()))?,
            scopes: body
                .scope
                .unwrap_or_default()
                .split([',', ' '])
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            refresh_token: body.refresh_token.and_then(|t| Secret::new(t).ok()),
            expires_in: body.expires_in.map(Duration::from_secs),
        });
    }
}

#[cfg(test)]
mod tests {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string_contains, method, path},
    };

    use super::*;

    fn endpoints(server: &MockServer) -> DeviceEndpoints {
        let base = Url::parse(&server.uri()).unwrap();
        DeviceEndpoints {
            code_url: base.join("login/device/code").unwrap(),
            token_url: base.join("login/oauth/access_token").unwrap(),
            client_id: "client123".into(),
            scopes: vec!["repo".into(), "read:org".into()],
        }
    }

    fn fast() -> PollOptions {
        PollOptions {
            slow_down_step: Duration::from_millis(20),
            min_interval: Duration::from_millis(1),
        }
    }

    fn authorization(expires_in: Duration) -> DeviceAuthorization {
        DeviceAuthorization {
            device_code: Secret::new("dev-code").unwrap(),
            user_code: "ABCD-1234".into(),
            verification_uri: Url::parse("https://github.com/login/device").unwrap(),
            expires_in,
            interval: Duration::from_millis(1),
        }
    }

    async fn token_reply(server: &MockServer, body: serde_json::Value, times: u64) {
        Mock::given(method("POST"))
            .and(path("/login/oauth/access_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .up_to_n_times(times)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn requests_a_code_with_scopes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/device/code"))
            .and(body_string_contains("client_id=client123"))
            .and(body_string_contains("scope=repo+read%3Aorg"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "dev-code",
                "user_code": "ABCD-1234",
                "verification_uri": "https://github.com/login/device",
                "expires_in": 900,
                "interval": 5
            })))
            .mount(&server)
            .await;

        let http = crate::client().unwrap();
        let auth = request_code(&http, &endpoints(&server)).await.unwrap();
        assert_eq!(auth.user_code, "ABCD-1234");
        assert_eq!(auth.interval, Duration::from_secs(5));
        assert_eq!(auth.device_code.expose(), "dev-code");
        assert!(!format!("{auth:?}").contains("dev-code"));
    }

    #[tokio::test]
    async fn disabled_device_flow_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/device/code"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({ "error": "device_flow_disabled" })),
            )
            .mount(&server)
            .await;
        let http = crate::client().unwrap();
        let err = request_code(&http, &endpoints(&server)).await.unwrap_err();
        assert_eq!(err, DeviceError::Disabled);
    }

    #[tokio::test]
    async fn polls_through_pending_and_slow_down() {
        let server = MockServer::start().await;
        // wiremock uses the first mounted mock that still matches.
        token_reply(
            &server,
            serde_json::json!({ "error": "authorization_pending" }),
            2,
        )
        .await;
        token_reply(
            &server,
            serde_json::json!({ "error": "slow_down", "interval": 0 }),
            1,
        )
        .await;
        token_reply(
            &server,
            serde_json::json!({ "access_token": "gho_token", "scope": "repo,read:org", "token_type": "bearer" }),
            1,
        )
        .await;

        let http = crate::client().unwrap();
        let start = Instant::now();
        let grant = poll_token(
            &http,
            &endpoints(&server),
            &authorization(Duration::from_secs(30)),
            fast(),
        )
        .await
        .unwrap();
        assert_eq!(grant.access_token.expose(), "gho_token");
        assert_eq!(grant.scopes, ["repo", "read:org"]);
        // The slow_down step was applied before the final poll.
        assert!(start.elapsed() >= Duration::from_millis(20));
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4);
        let body = String::from_utf8_lossy(&requests[0].body).into_owned();
        assert!(body.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"));
    }

    #[tokio::test]
    async fn denial_and_expiry_end_polling() {
        let server = MockServer::start().await;
        token_reply(&server, serde_json::json!({ "error": "access_denied" }), 1).await;
        let http = crate::client().unwrap();
        let err = poll_token(
            &http,
            &endpoints(&server),
            &authorization(Duration::from_secs(30)),
            fast(),
        )
        .await
        .unwrap_err();
        assert_eq!(err, DeviceError::Denied);

        let server = MockServer::start().await;
        token_reply(
            &server,
            serde_json::json!({ "error": "authorization_pending" }),
            1000,
        )
        .await;
        let err = poll_token(
            &http,
            &endpoints(&server),
            &authorization(Duration::from_millis(50)),
            fast(),
        )
        .await
        .unwrap_err();
        assert_eq!(err, DeviceError::Expired);
    }
}
