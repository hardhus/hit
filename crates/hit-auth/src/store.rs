//! Token storage: the OS keyring by default, a private file only on explicit
//! request, and memory for tests.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use crate::{AccountKey, KEYRING_SERVICE, Secret, Storage, fsutil};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// No usable OS keyring (e.g. no Secret Service on a headless Linux).
    #[error("the system keyring is not available: {0}")]
    Unavailable(String),
    #[error("token file {0} is readable by other users; fix with `chmod 600` or sign in again")]
    InsecurePermissions(PathBuf),
    #[error("stored token is invalid: {0}")]
    Invalid(String),
    #[error("could not access token file {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub trait TokenStore: Send + Sync {
    fn storage(&self) -> Storage;
    fn get(&self, key: &AccountKey) -> Result<Option<Secret>, StoreError>;
    fn set(&self, key: &AccountKey, token: &Secret) -> Result<(), StoreError>;
    /// Returns whether something was deleted.
    fn delete(&self, key: &AccountKey) -> Result<bool, StoreError>;
}

/// Windows Credential Manager, macOS Keychain or the Secret Service.
#[derive(Debug, Clone)]
pub struct KeyringStore {
    service: String,
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self {
            service: KEYRING_SERVICE.to_owned(),
        }
    }
}

impl KeyringStore {
    fn entry(&self, key: &AccountKey) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(&self.service, &key.to_string()).map_err(keyring_error)
    }

    /// Checks that the keyring can be used at all.
    pub fn probe(&self) -> Result<(), StoreError> {
        let entry = keyring::Entry::new(&self.service, "hit-probe").map_err(keyring_error)?;
        match entry.get_password() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keyring_error(e)),
        }
    }
}

/// Maps keyring errors without ever including a secret (the variants that
/// carry data hold attributes or platform messages, not passwords).
fn keyring_error(error: keyring::Error) -> StoreError {
    match error {
        keyring::Error::BadEncoding(_) => {
            StoreError::Invalid("keyring entry is not valid UTF-8".to_owned())
        }
        other => StoreError::Unavailable(other.to_string()),
    }
}

impl TokenStore for KeyringStore {
    fn storage(&self) -> Storage {
        Storage::Keyring
    }

    fn get(&self, key: &AccountKey) -> Result<Option<Secret>, StoreError> {
        match self.entry(key)?.get_password() {
            Ok(value) => Secret::new(value)
                .map(Some)
                .map_err(|e| StoreError::Invalid(e.to_string())),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keyring_error(e)),
        }
    }

    fn set(&self, key: &AccountKey, token: &Secret) -> Result<(), StoreError> {
        self.entry(key)?
            .set_password(token.expose())
            .map_err(keyring_error)
    }

    fn delete(&self, key: &AccountKey) -> Result<bool, StoreError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(keyring_error(e)),
        }
    }
}

/// Plain token files in a private directory. Only used when the user passes
/// `--insecure-storage`; refuses to read files others can access.
#[derive(Debug, Clone)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, key: &AccountKey) -> PathBuf {
        // Host and login are validated upstream; still never let them form a
        // path that escapes the directory.
        let name: String = key
            .to_string()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{name}.token"))
    }
}

impl TokenStore for FileStore {
    fn storage(&self) -> Storage {
        Storage::File
    }

    fn get(&self, key: &AccountKey) -> Result<Option<Secret>, StoreError> {
        let path = self.path(key);
        let io_err = |source| StoreError::Io {
            path: path.clone(),
            source,
        };
        match fsutil::is_too_open(&path) {
            Ok(true) => return Err(StoreError::InsecurePermissions(path)),
            Ok(false) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err(e)),
        }
        let value = fs::read_to_string(&path).map_err(io_err)?;
        Secret::new(value)
            .map(Some)
            .map_err(|e| StoreError::Invalid(e.to_string()))
    }

    fn set(&self, key: &AccountKey, token: &Secret) -> Result<(), StoreError> {
        let io_err = |path: &Path| {
            let path = path.to_path_buf();
            move |source| StoreError::Io { path, source }
        };
        fsutil::create_private_dir(&self.dir).map_err(io_err(&self.dir))?;
        let path = self.path(key);
        fsutil::write_private_atomic(&path, token.expose().as_bytes()).map_err(io_err(&path))
    }

    fn delete(&self, key: &AccountKey) -> Result<bool, StoreError> {
        let path = self.path(key);
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }
}

/// In-memory store for tests and the demo.
#[derive(Debug, Default)]
pub struct MemoryStore {
    tokens: Mutex<HashMap<String, Secret>>,
}

impl TokenStore for MemoryStore {
    fn storage(&self) -> Storage {
        Storage::Keyring
    }

    fn get(&self, key: &AccountKey) -> Result<Option<Secret>, StoreError> {
        let tokens = self.tokens.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(tokens.get(&key.to_string()).cloned())
    }

    fn set(&self, key: &AccountKey, token: &Secret) -> Result<(), StoreError> {
        let mut tokens = self.tokens.lock().unwrap_or_else(PoisonError::into_inner);
        tokens.insert(key.to_string(), token.clone());
        Ok(())
    }

    fn delete(&self, key: &AccountKey) -> Result<bool, StoreError> {
        let mut tokens = self.tokens.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(tokens.remove(&key.to_string()).is_some())
    }
}

#[cfg(test)]
mod tests {
    use hit_core::Host;

    use super::*;

    fn key(login: &str) -> AccountKey {
        AccountKey::new(Host::new("github.com"), login)
    }

    #[test]
    fn file_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path().join("tokens"));
        assert!(store.get(&key("you")).unwrap().is_none());

        store
            .set(&key("you"), &Secret::new("tok1").unwrap())
            .unwrap();
        store
            .set(&key("you"), &Secret::new("tok2").unwrap())
            .unwrap();
        assert_eq!(store.get(&key("you")).unwrap().unwrap().expose(), "tok2");

        assert!(store.delete(&key("you")).unwrap());
        assert!(!store.delete(&key("you")).unwrap());
        assert!(store.get(&key("you")).unwrap().is_none());
    }

    #[test]
    fn file_names_cannot_escape_the_directory() {
        let store = FileStore::new("/base");
        let path = store.path(&AccountKey::new(Host::new("../../etc"), "../passwd"));
        assert_eq!(path.parent(), Some(Path::new("/base")));
    }

    #[cfg(unix)]
    #[test]
    fn file_store_refuses_files_others_can_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path());
        store
            .set(&key("you"), &Secret::new("tok").unwrap())
            .unwrap();
        let path = store.path(&key("you"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            store.get(&key("you")),
            Err(StoreError::InsecurePermissions(_))
        ));
    }

    #[test]
    fn memory_store_roundtrip() {
        let store = MemoryStore::default();
        store.set(&key("a"), &Secret::new("t").unwrap()).unwrap();
        assert_eq!(store.get(&key("a")).unwrap().unwrap().expose(), "t");
        assert!(store.delete(&key("a")).unwrap());
    }
}
