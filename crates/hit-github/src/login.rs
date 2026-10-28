//! Signing in to GitHub: device flow, personal access tokens and tokens
//! imported from the GitHub CLI all end in [`GithubLogin::complete`], which
//! verifies the token *before* anything is stored.

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use hit_auth::{
    Account, Accounts, Secret, Storage, StoreError, TokenKind, TokenStore,
    device::{self, DeviceAuthorization, DeviceEndpoints, DeviceError, PollOptions},
};
use hit_core::{
    Forge, ForgeError, ForgeKind, Host, Timestamp,
    auth::{DeviceCode, LoginError, LoginFlow, PendingLogin},
};
use reqwest::Client;

use crate::{Api, ApiOptions, FocusOptions, GithubForge, GithubHost, token_kind, verify};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginStorage {
    Keyring,
    /// Plain file; only with `--insecure-storage`.
    InsecureFile,
}

#[derive(Debug, Clone)]
pub struct LoginSettings {
    pub scopes: Vec<String>,
    pub read_only: bool,
    pub allow_mark_read: bool,
    pub storage: LoginStorage,
    /// Action Required categories for the signed-in forge.
    pub focus: FocusOptions,
}

/// Everything needed to sign in to one GitHub host.
#[derive(Clone)]
pub struct GithubLogin {
    http: Client,
    host: GithubHost,
    client_id: Option<String>,
    settings: LoginSettings,
    store: Arc<dyn TokenStore>,
    accounts_path: PathBuf,
}

impl GithubLogin {
    pub fn new(
        http: Client,
        host: GithubHost,
        client_id: Option<String>,
        settings: LoginSettings,
        store: Arc<dyn TokenStore>,
        accounts_path: PathBuf,
    ) -> Self {
        Self {
            http,
            host,
            client_id,
            settings,
            store,
            accounts_path,
        }
    }

    pub fn github_host(&self) -> &GithubHost {
        &self.host
    }

    fn endpoints(&self) -> Result<DeviceEndpoints, LoginError> {
        let client_id = self.client_id.clone().ok_or_else(|| {
            LoginError::Unavailable(format!(
                "no OAuth app is configured for {}; sign in with a personal access token \
                 (`hit auth login --with-token`) or set `hosts.\"{}\".oauth_client_id`",
                self.host.host(),
                self.host.host()
            ))
        })?;
        Ok(DeviceEndpoints {
            code_url: self.host.device_code_url(),
            token_url: self.host.token_url(),
            client_id,
            scopes: self.settings.scopes.clone(),
        })
    }

    /// Step 1 of the device flow.
    pub async fn start_device(&self) -> Result<(DeviceAuthorization, DeviceCode), LoginError> {
        let endpoints = self.endpoints()?;
        let authorization = device::request_code(&self.http, &endpoints)
            .await
            .map_err(device_error)?;
        // Never send the user to a page the server picked on another origin.
        if !self.host.is_own_web_url(&authorization.verification_uri) {
            return Err(LoginError::Forge(ForgeError::InvalidResponse(format!(
                "unexpected verification URL {}",
                authorization.verification_uri
            ))));
        }
        let expires_in = i64::try_from(authorization.expires_in.as_secs()).unwrap_or(900);
        let code = DeviceCode {
            host: self.host.host().clone(),
            user_code: authorization.user_code.clone(),
            verification_uri: authorization.verification_uri.to_string(),
            expires_at: Timestamp::now()
                .checked_add(jiff::SignedDuration::from_secs(expires_in))
                .unwrap_or_else(|_| Timestamp::now()),
        };
        Ok((authorization, code))
    }

    /// Step 2 of the device flow: waits for approval, then completes.
    pub async fn finish_device(
        &self,
        authorization: &DeviceAuthorization,
    ) -> Result<(Account, GithubForge), LoginError> {
        let endpoints = self.endpoints()?;
        let grant = device::poll_token(
            &self.http,
            &endpoints,
            authorization,
            PollOptions::default(),
        )
        .await
        .map_err(device_error)?;
        if grant.refresh_token.is_some() || grant.expires_in.is_some() {
            // Expiring tokens are not configured for hit's app; refreshing is
            // not implemented, so warn instead of failing silently later.
            tracing::warn!(
                "received an expiring token; it will need a new sign-in when it expires"
            );
        }
        self.complete(grant.access_token, TokenKind::OAuth).await
    }

    /// Signs in with an existing token (PAT or imported from gh).
    pub async fn with_token(
        &self,
        token: Secret,
        kind: Option<TokenKind>,
    ) -> Result<(Account, GithubForge), LoginError> {
        let kind = kind.unwrap_or_else(|| token_kind(&token));
        self.complete(token, kind).await
    }

    /// Verifies the token, stores it, then records the account. If recording
    /// fails the stored token is removed again.
    async fn complete(
        &self,
        token: Secret,
        kind: TokenKind,
    ) -> Result<(Account, GithubForge), LoginError> {
        let options = ApiOptions {
            read_only: self.settings.read_only,
            allow_mark_read: self.settings.allow_mark_read,
        };
        let api = Api::new(self.http.clone(), self.host.clone(), token.clone(), options);
        let viewer = verify(&api).await?;

        let account = Account {
            host: self.host.host().clone(),
            login: viewer.login,
            kind: ForgeKind::GitHub,
            storage: match self.settings.storage {
                LoginStorage::Keyring => Storage::Keyring,
                LoginStorage::InsecureFile => Storage::File,
            },
            token_kind: kind,
            scopes: viewer.scopes.unwrap_or_default(),
            read_only: self.settings.read_only,
            added_at: Timestamp::now(),
        };

        self.store
            .set(&account.key(), &token)
            .map_err(storage_error)?;
        let recorded = Accounts::load(&self.accounts_path).and_then(|mut accounts| {
            accounts.upsert(account.clone());
            accounts.save(&self.accounts_path)
        });
        if let Err(e) = recorded {
            if let Err(cleanup) = self.store.delete(&account.key()) {
                tracing::error!(error = %cleanup, "could not remove token after failed sign-in");
            }
            return Err(LoginError::Storage(e.to_string()));
        }

        tracing::info!(account = %account.key(), kind = ?account.token_kind, "signed in");
        let forge = GithubForge::new(account.id(), api, self.settings.focus);
        Ok((account, forge))
    }
}

fn device_error(error: DeviceError) -> LoginError {
    match error {
        DeviceError::Denied => LoginError::Denied,
        DeviceError::Expired => LoginError::Expired,
        DeviceError::Disabled => {
            LoginError::Unavailable("device flow is disabled for hit's OAuth app".to_owned())
        }
        DeviceError::BadClient => {
            LoginError::Unavailable("GitHub rejected hit's OAuth client id".to_owned())
        }
        DeviceError::Network(m) => LoginError::Forge(ForgeError::Network(m)),
        DeviceError::Protocol(m) => LoginError::Forge(ForgeError::InvalidResponse(m)),
    }
}

fn storage_error(error: StoreError) -> LoginError {
    match error {
        StoreError::Unavailable(reason) => LoginError::Storage(format!(
            "the system keyring is not available ({reason}). To keep the token in a \
             private file instead, run `hit auth login --insecure-storage`"
        )),
        other => LoginError::Storage(other.to_string()),
    }
}

#[async_trait]
impl LoginFlow for GithubLogin {
    fn host(&self) -> &Host {
        self.host.host()
    }

    async fn start(&self) -> Result<Box<dyn PendingLogin>, LoginError> {
        let (authorization, code) = self.start_device().await?;
        Ok(Box::new(Pending {
            login: self.clone(),
            authorization,
            code,
        }))
    }
}

struct Pending {
    login: GithubLogin,
    authorization: DeviceAuthorization,
    code: DeviceCode,
}

#[async_trait]
impl PendingLogin for Pending {
    fn code(&self) -> &DeviceCode {
        &self.code
    }

    async fn finish(self: Box<Self>) -> Result<Arc<dyn Forge>, LoginError> {
        let (_, forge) = self.login.finish_device(&self.authorization).await?;
        Ok(Arc::new(forge))
    }
}

#[cfg(test)]
mod tests {
    use hit_auth::{AccountKey, MemoryStore};
    use url::Url;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    fn settings() -> LoginSettings {
        LoginSettings {
            scopes: vec!["repo".into()],
            read_only: true,
            allow_mark_read: false,
            storage: LoginStorage::Keyring,
            focus: FocusOptions::default(),
        }
    }

    async fn login(
        server: &MockServer,
        dir: &std::path::Path,
        store: Arc<MemoryStore>,
    ) -> GithubLogin {
        let base = Url::parse(&server.uri()).unwrap();
        GithubLogin::new(
            hit_auth::client().unwrap(),
            GithubHost::for_tests("github.com", &base),
            Some("client123".into()),
            settings(),
            store,
            dir.join("accounts.toml"),
        )
    }

    async fn mount_user(server: &MockServer, status: u16) {
        Mock::given(method("GET"))
            .and(path("/api/user"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("x-oauth-scopes", "repo")
                    .set_body_json(serde_json::json!({ "login": "octocat" })),
            )
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn token_login_verifies_then_stores() {
        let server = MockServer::start().await;
        mount_user(&server, 200).await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryStore::default());
        let login = login(&server, dir.path(), Arc::clone(&store)).await;

        let (account, forge) = login
            .with_token(Secret::new("ghp_abc").unwrap(), None)
            .await
            .unwrap();
        assert_eq!(account.login, "octocat");
        assert_eq!(account.token_kind, TokenKind::Classic);
        assert!(account.read_only);
        assert!(forge.api().options().read_only);

        let key = AccountKey::new(Host::new("github.com"), "octocat");
        assert_eq!(store.get(&key).unwrap().unwrap().expose(), "ghp_abc");
        let accounts = Accounts::load(&dir.path().join("accounts.toml")).unwrap();
        assert_eq!(accounts.get(&key), Some(&account));
    }

    #[tokio::test]
    async fn invalid_tokens_are_never_stored() {
        let server = MockServer::start().await;
        mount_user(&server, 401).await;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryStore::default());
        let login = login(&server, dir.path(), Arc::clone(&store)).await;

        let err = login
            .with_token(Secret::new("ghp_bad").unwrap(), None)
            .await
            .unwrap_err();
        assert_eq!(err, LoginError::Forge(ForgeError::Unauthorized));
        let key = AccountKey::new(Host::new("github.com"), "octocat");
        assert!(store.get(&key).unwrap().is_none());
        assert!(!dir.path().join("accounts.toml").exists());
    }

    #[tokio::test]
    async fn verification_url_must_be_on_the_host() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "d",
                "user_code": "ABCD-1234",
                "verification_uri": "https://evil.example/login/device",
                "expires_in": 900,
                "interval": 5
            })))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let login = login(&server, dir.path(), Arc::new(MemoryStore::default())).await;
        assert!(login.start_device().await.is_err());
    }

    #[tokio::test]
    async fn missing_client_id_explains_the_alternative() {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let mut login = login(&server, dir.path(), Arc::new(MemoryStore::default())).await;
        login.client_id = None;
        let err = login.start_device().await.unwrap_err();
        assert!(err.to_string().contains("--with-token"), "{err}");
    }
}
