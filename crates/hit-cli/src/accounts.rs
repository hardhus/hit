//! Wiring between config, paths, stored accounts and providers.

use std::{path::PathBuf, sync::Arc};

use hit_auth::{
    Account, Accounts, FileStore, KeyringStore, Secret, Storage, StoreError, TokenSource,
    TokenStore, env_token,
};
use hit_cache::{Cache, Vault};
use hit_config::{Config, Paths};
use hit_core::Timestamp;
use hit_core::{ForgeKind, Forges, auth::LoginFlow};
use hit_github::{
    Api, ApiOptions, FocusOptions, GithubForge, GithubHost, GithubLogin, LoginSettings,
    LoginStorage, local_key, oauth,
};
use jiff::SignedDuration;

pub fn accounts_path(paths: &Paths) -> PathBuf {
    paths.config.join("accounts.toml")
}

pub fn store(storage: Storage, paths: &Paths) -> Arc<dyn TokenStore> {
    match storage {
        Storage::Keyring => Arc::new(KeyringStore::default()),
        Storage::File => Arc::new(FileStore::new(paths.state.join("tokens"))),
    }
}

pub fn env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

pub fn focus_options(config: &Config) -> FocusOptions {
    let focus = &config.focus;
    FocusOptions {
        review_requests: focus.review_requests,
        my_pull_requests: focus.my_pull_requests,
        mentions: focus.mentions,
        mention_days: focus.mention_days,
        assigned: focus.assigned,
    }
}

pub fn api_options(account: &Account, config: &Config) -> ApiOptions {
    ApiOptions {
        read_only: account.read_only || config.auth.read_only,
        allow_mark_read: config.auth.allow_mark_read,
    }
}

/// The token for `account`: the environment first, then its store.
pub fn token(
    account: &Account,
    paths: &Paths,
) -> Result<Option<(Secret, TokenSource)>, StoreError> {
    if let Some(found) = env_token(&account.host, &env) {
        return Ok(Some(found));
    }
    let source = match account.storage {
        Storage::Keyring => TokenSource::Keyring,
        Storage::File => TokenSource::File,
    };
    Ok(store(account.storage, paths)
        .get(&account.key())?
        .map(|secret| (secret, source)))
}

pub fn client_id(host: &GithubHost, config: &Config) -> Option<String> {
    let configured = config
        .hosts
        .get(host.host().as_str())
        .and_then(|h| h.oauth_client_id.as_deref());
    oauth::client_id(host, configured, &env)
}

pub fn github_login(
    host: GithubHost,
    config: &Config,
    paths: &Paths,
    settings: LoginSettings,
) -> Result<GithubLogin, hit_auth::HttpError> {
    let storage = match settings.storage {
        LoginStorage::Keyring => Storage::Keyring,
        LoginStorage::InsecureFile => Storage::File,
    };
    Ok(GithubLogin::new(
        hit_auth::client()?,
        host.clone(),
        client_id(&host, config),
        settings,
        store(storage, paths),
        accounts_path(paths),
    ))
}

/// Sign-in from inside the TUI: github.com, keyring only (insecure storage
/// needs the explicit CLI flag).
pub fn tui_login(config: &Config, paths: &Paths) -> Option<Arc<dyn LoginFlow>> {
    let host = GithubHost::parse("github.com").ok()?;
    client_id(&host, config)?;
    let settings = LoginSettings {
        scopes: oauth::SCOPES.iter().map(|s| (*s).to_owned()).collect(),
        read_only: config.auth.read_only,
        allow_mark_read: config.auth.allow_mark_read,
        storage: LoginStorage::Keyring,
        focus: focus_options(config),
    };
    let login = github_login(host, config, paths, settings).ok()?;
    Some(Arc::new(login))
}

/// Builds a forge for every signed-in account. Problems become warnings
/// shown on the start screen; they never stop hit from starting.
pub fn forges(config: &Config, paths: &Paths) -> (Forges, Vec<String>) {
    let mut forges = Forges::new();
    let mut warnings = Vec::new();

    let accounts = match Accounts::load(&accounts_path(paths)) {
        Ok(accounts) => accounts,
        Err(e) => {
            warnings.push(e.to_string());
            return (forges, warnings);
        }
    };
    let http = match hit_auth::client() {
        Ok(http) => http,
        Err(e) => {
            warnings.push(e.to_string());
            return (forges, warnings);
        }
    };

    for account in accounts.iter().filter(|a| a.kind == ForgeKind::GitHub) {
        let key = account.key();
        let host = match GithubHost::parse(account.host.as_str()) {
            Ok(host) => host,
            Err(e) => {
                warnings.push(format!("{key}: {e}"));
                continue;
            }
        };
        match token(account, paths) {
            Ok(Some((secret, source))) => {
                tracing::info!(account = %key, %source, "account loaded");
                let api = Api::new(http.clone(), host, secret, api_options(account, config));
                forges.add(Arc::new(GithubForge::new(
                    account.id(),
                    api,
                    focus_options(config),
                )));
            }
            Ok(None) => warnings.push(format!(
                "{key}: no stored token; sign in again with `hit auth login`"
            )),
            Err(e) => warnings.push(format!("{key}: {e}")),
        }
    }

    if accounts
        .on_host(&hit_core::Host::new("github.com"))
        .next()
        .is_none()
        && env_token(&hit_core::Host::new("github.com"), &env).is_some()
    {
        warnings.push(
            "GH_TOKEN/GITHUB_TOKEN is set but no github.com account is signed in; \
             run `hit auth login --with-token` to use it"
                .to_owned(),
        );
    }
    (forges, warnings)
}

pub fn cache_path(paths: &Paths) -> PathBuf {
    paths.cache.join("cache.sqlite")
}

/// Opens the encrypted local cache and drops entries older than the
/// configured age. Any problem means "no cache", never a failed start.
pub fn open_cache(config: &Config, paths: &Paths) -> Option<Arc<Cache>> {
    if !config.cache.enabled {
        return None;
    }
    match Cache::open(&cache_path(paths)) {
        Ok(cache) => {
            let days = i64::from(config.cache.max_age_days);
            if let Ok(cutoff) = Timestamp::now().checked_sub(SignedDuration::from_hours(days * 24))
                && let Err(e) = cache.prune(cutoff)
            {
                tracing::warn!(error = %e, "could not prune the cache");
            }
            Some(Arc::new(cache))
        }
        Err(e) => {
            tracing::warn!(error = %e, "cache disabled for this run");
            None
        }
    }
}

/// Removes what is cached for `account`, using its token to find it.
pub fn clear_account_cache(account: &Account, token: &Secret, paths: &Paths) {
    let path = cache_path(paths);
    if !path.exists() {
        return;
    }
    let Ok(host) = GithubHost::parse(account.host.as_str()) else {
        return;
    };
    let vault = Vault::new(&local_key(token, &host), &account.id().to_string());
    match Cache::open(&path).and_then(|cache| cache.clear_account(&vault)) {
        Ok(removed) => tracing::info!(removed, account = %account.key(), "cleared account cache"),
        Err(e) => tracing::warn!(error = %e, "could not clear the account cache"),
    }
}
