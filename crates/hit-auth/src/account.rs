//! Signed-in accounts, persisted in `accounts.toml`. This file never contains
//! secrets; tokens live in a [`crate::TokenStore`].

use std::{fmt, fs, io, path::Path, path::PathBuf};

use hit_core::{AccountId, ForgeKind, Host, Timestamp};
use serde::{Deserialize, Serialize};

use crate::fsutil;

/// Where an account's token is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Storage {
    Keyring,
    /// Plain file, only with `--insecure-storage`.
    File,
}

/// How the token was obtained; decides where users revoke it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    /// OAuth app token from the device flow.
    OAuth,
    /// Classic personal access token.
    Classic,
    FineGrained,
    /// Imported from the GitHub CLI.
    GhCli,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub host: Host,
    pub login: String,
    pub kind: ForgeKind,
    pub storage: Storage,
    pub token_kind: TokenKind,
    /// Scopes reported when the account was added.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// hit never writes with this account (except, if allowed, marking
    /// notifications read).
    #[serde(default)]
    pub read_only: bool,
    pub added_at: Timestamp,
}

impl Account {
    pub fn key(&self) -> AccountKey {
        AccountKey::new(self.host.clone(), self.login.clone())
    }

    pub fn id(&self) -> AccountId {
        AccountId::new(self.host.clone(), self.login.clone())
    }
}

/// Identifies an account in stores: host and login.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountKey {
    pub host: Host,
    pub login: String,
}

impl AccountKey {
    pub fn new(host: Host, login: impl Into<String>) -> Self {
        Self {
            host,
            login: login.into(),
        }
    }
}

impl fmt::Display for AccountKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.host, self.login)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AccountsError {
    #[error("could not read {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Not lenient on purpose: silently dropping accounts would look like a
    /// sign-out and could lead to tokens being orphaned in the keyring.
    #[error("{path} is not valid: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("could not write {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not serialize accounts: {0}")]
    Serialize(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Accounts {
    #[serde(default, rename = "account")]
    accounts: Vec<Account>,
}

impl Accounts {
    /// Loads `path`; a missing file means no accounts.
    pub fn load(path: &Path) -> Result<Self, AccountsError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(AccountsError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|e| AccountsError::Parse {
            path: path.to_path_buf(),
            message: e.message().to_owned(),
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), AccountsError> {
        let body =
            toml::to_string_pretty(self).map_err(|e| AccountsError::Serialize(e.to_string()))?;
        let text = format!(
            "# Accounts signed in to hit. Managed by `hit auth`; holds no credentials.\n\n{body}"
        );
        let write_err = |source| AccountsError::Write {
            path: path.to_path_buf(),
            source,
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(write_err)?;
        }
        fsutil::write_private_atomic(path, text.as_bytes()).map_err(write_err)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Account> {
        self.accounts.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    pub fn on_host<'a>(&'a self, host: &'a Host) -> impl Iterator<Item = &'a Account> + 'a {
        self.accounts.iter().filter(move |a| &a.host == host)
    }

    pub fn get(&self, key: &AccountKey) -> Option<&Account> {
        self.accounts.iter().find(|a| a.key() == *key)
    }

    /// Adds the account or replaces the one with the same host and login.
    pub fn upsert(&mut self, account: Account) {
        match self.accounts.iter_mut().find(|a| a.key() == account.key()) {
            Some(existing) => *existing = account,
            None => self.accounts.push(account),
        }
    }

    pub fn remove(&mut self, key: &AccountKey) -> Option<Account> {
        let index = self.accounts.iter().position(|a| a.key() == *key)?;
        Some(self.accounts.remove(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(login: &str) -> Account {
        Account {
            host: Host::new("github.com"),
            login: login.into(),
            kind: ForgeKind::GitHub,
            storage: Storage::Keyring,
            token_kind: TokenKind::OAuth,
            scopes: vec!["repo".into(), "read:org".into()],
            read_only: false,
            added_at: Timestamp::UNIX_EPOCH,
        }
    }

    #[test]
    fn roundtrip_upsert_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.toml");
        assert!(Accounts::load(&path).unwrap().is_empty());

        let mut accounts = Accounts::default();
        accounts.upsert(account("you"));
        accounts.upsert(account("work"));
        let mut changed = account("you");
        changed.read_only = true;
        accounts.upsert(changed);
        accounts.save(&path).unwrap();

        let loaded = Accounts::load(&path).unwrap();
        assert_eq!(loaded, accounts);
        assert_eq!(loaded.iter().count(), 2);
        let key = AccountKey::new(Host::new("github.com"), "you");
        assert!(loaded.get(&key).unwrap().read_only);

        let mut loaded = loaded;
        assert!(loaded.remove(&key).is_some());
        assert!(loaded.get(&key).is_none());
    }

    #[test]
    fn file_contains_no_secret_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.toml");
        let mut accounts = Accounts::default();
        accounts.upsert(account("you"));
        accounts.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        for word in ["token =", "secret", "password", "gho_", "ghp_"] {
            assert!(!text.contains(word), "{word} in {text}");
        }
    }

    #[test]
    fn broken_file_is_an_error_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.toml");
        fs::write(&path, "[[account]\nhost = ").unwrap();
        assert!(matches!(
            Accounts::load(&path),
            Err(AccountsError::Parse { .. })
        ));
    }
}
