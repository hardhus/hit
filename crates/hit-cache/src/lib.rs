//! Local cache: the last known data per account, so hit can draw instantly
//! and stay useful offline.
//!
//! **Encrypted at rest.** Every payload is sealed with XChaCha20-Poly1305
//! under a key derived from the account's own credential (see
//! [`Vault`]); the file holds no plaintext titles, bodies or logins. Rows are
//! bound to their account slot and key, so they cannot be swapped. When the
//! token changes or is removed (sign-out), old rows simply stop decrypting.
//!
//! It is only ever a cache: any problem (corrupt file, schema change, wrong
//! key) degrades to "no cached data", never to an error for the user.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use chacha20poly1305::{
    Key, XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hmac::{Hmac, Mac};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use sha2::Sha256;
use zeroize::Zeroizing;

/// Bump when cached model shapes change; older entries are then ignored.
pub const SCHEMA: i64 = 1;
/// On-disk layout version (`PRAGMA user_version`); a mismatch drops the table.
const LAYOUT: i64 = 2;
const NONCE_LEN: usize = 24;

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cache database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not prepare the cache directory {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not encode cache entry: {0}")]
    Encode(String),
    #[error("could not encrypt cache entry")]
    Encrypt,
}

/// A cached value and when it was fetched.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry<T> {
    pub value: T,
    pub fetched_at: Timestamp,
}

/// One account's encryption context.
///
/// Built from a 32-byte account key (the forge derives it from the token).
/// Two independent sub-keys are derived from it: one encrypts payloads, the
/// other turns the account name into an opaque slot id, so the database
/// never contains logins.
pub struct Vault {
    cipher: XChaCha20Poly1305,
    slot: String,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault").field("slot", &self.slot).finish()
    }
}

type HmacSha256 = Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> Zeroizing<[u8; 32]> {
    // HMAC accepts keys of any length, so this cannot fail.
    let Ok(mut mac) = <HmacSha256 as Mac>::new_from_slice(key) else {
        return Zeroizing::new([0; 32]);
    };
    mac.update(data);
    Zeroizing::new(mac.finalize().into_bytes().into())
}

impl Vault {
    pub fn new(account_key: &[u8; 32], account: &str) -> Self {
        let encryption = hmac(account_key, b"hit/cache/encryption");
        let identity = hmac(account_key, b"hit/cache/slot");
        let slot = hmac(identity.as_slice(), account.as_bytes());
        Self {
            cipher: XChaCha20Poly1305::new(Key::from_slice(encryption.as_slice())),
            slot: slot.iter().take(16).map(|b| format!("{b:02x}")).collect(),
        }
    }

    fn aad(&self, key: &str) -> Vec<u8> {
        format!("{}\0{key}\0{SCHEMA}", self.slot).into_bytes()
    }

    fn seal(&self, key: &str, plaintext: &[u8]) -> Result<Vec<u8>, CacheError> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|_| CacheError::Encrypt)?;
        let aad = self.aad(key);
        let ciphertext = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| CacheError::Encrypt)?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    fn open(&self, key: &str, sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        if sealed.len() < NONCE_LEN {
            return None;
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        let aad = self.aad(key);
        self.cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: &aad,
                },
            )
            .ok()
            .map(Zeroizing::new)
    }
}

pub struct Cache {
    conn: Mutex<Connection>,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache").field("path", &self.path).finish()
    }
}

impl Cache {
    /// Opens (or creates) the cache at `path`. A database that cannot be
    /// opened is deleted and recreated once: it only ever holds copies.
    pub fn open(path: &Path) -> Result<Self, CacheError> {
        if let Some(dir) = path.parent() {
            create_private_dir(dir).map_err(|source| CacheError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        let conn = match Self::connect(path) {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "cache unusable; recreating it");
                remove_files(path);
                Self::connect(path)?
            }
        };
        restrict_file(path);
        Ok(Self {
            conn: Mutex::new(conn),
            path: Some(path.to_path_buf()),
        })
    }

    /// An in-memory cache, for tests.
    pub fn in_memory() -> Result<Self, CacheError> {
        let conn = Connection::open_in_memory()?;
        init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path: None,
        })
    }

    /// Deletes the cache file (and SQLite side files) at `path`.
    pub fn remove(path: &Path) {
        remove_files(path);
    }

    fn connect(path: &Path) -> Result<Connection, CacheError> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        init(&conn)?;
        // Fails on files that are not SQLite databases.
        conn.query_row("SELECT count(*) FROM entries", [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(conn)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The cached value, or `None` if missing, from another schema version,
    /// sealed with another key, or unreadable.
    pub fn get<T: DeserializeOwned>(&self, vault: &Vault, key: &str) -> Option<Entry<T>> {
        let row = self
            .conn()
            .query_row(
                "SELECT payload, fetched_at FROM entries
                 WHERE account = ?1 AND key = ?2 AND schema = ?3",
                params![vault.slot, key, SCHEMA],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional();
        let (sealed, fetched_at) = match row {
            Ok(Some(row)) => row,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(error = %e, key, "cache read failed");
                return None;
            }
        };
        let Some(plaintext) = vault.open(key, &sealed) else {
            // Token changed since this was written; it will be overwritten.
            tracing::debug!(key, "cache entry does not decrypt; ignoring it");
            return None;
        };
        let value = match serde_json::from_slice(&plaintext) {
            Ok(value) => value,
            Err(e) => {
                tracing::warn!(error = %e, key, "cache entry does not decode; ignoring it");
                return None;
            }
        };
        let fetched_at = Timestamp::from_millisecond(fetched_at).ok()?;
        Some(Entry { value, fetched_at })
    }

    pub fn put<T: Serialize>(&self, vault: &Vault, key: &str, value: &T) -> Result<(), CacheError> {
        self.put_at(vault, key, value, Timestamp::now())
    }

    fn put_at<T: Serialize>(
        &self,
        vault: &Vault,
        key: &str,
        value: &T,
        fetched_at: Timestamp,
    ) -> Result<(), CacheError> {
        let plaintext = Zeroizing::new(
            serde_json::to_vec(value).map_err(|e| CacheError::Encode(e.to_string()))?,
        );
        let sealed = vault.seal(key, &plaintext)?;
        self.conn().execute(
            "INSERT INTO entries (account, key, schema, fetched_at, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (account, key) DO UPDATE SET
               schema = excluded.schema,
               fetched_at = excluded.fetched_at,
               payload = excluded.payload",
            params![vault.slot, key, SCHEMA, fetched_at.as_millisecond(), sealed],
        )?;
        Ok(())
    }

    /// Removes everything cached for the vault's account (e.g. on sign-out).
    pub fn clear_account(&self, vault: &Vault) -> Result<usize, CacheError> {
        Ok(self.conn().execute(
            "DELETE FROM entries WHERE account = ?1",
            params![vault.slot],
        )?)
    }

    pub fn clear_all(&self) -> Result<usize, CacheError> {
        Ok(self.conn().execute("DELETE FROM entries", [])?)
    }

    /// Drops entries fetched before `cutoff` and entries of other schema
    /// versions.
    pub fn prune(&self, cutoff: Timestamp) -> Result<usize, CacheError> {
        Ok(self.conn().execute(
            "DELETE FROM entries WHERE fetched_at < ?1 OR schema != ?2",
            params![cutoff.as_millisecond(), SCHEMA],
        )?)
    }
}

/// Creates the table; a database from another layout (e.g. the unencrypted
/// pre-release one) is wiped first.
fn init(conn: &Connection) -> Result<(), CacheError> {
    let layout: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if layout != LAYOUT {
        conn.execute_batch("DROP TABLE IF EXISTS entries;")?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entries (
             account    TEXT    NOT NULL,
             key        TEXT    NOT NULL,
             schema     INTEGER NOT NULL,
             fetched_at INTEGER NOT NULL,
             payload    BLOB    NOT NULL,
             PRIMARY KEY (account, key)
         );",
    )?;
    conn.execute_batch(&format!("PRAGMA user_version = {LAYOUT};"))?;
    Ok(())
}

fn remove_files(path: &Path) {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Owner-only access on Unix; Windows relies on the user profile's ACL.
/// Defense in depth: the payloads are encrypted anyway.
fn restrict_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            tracing::warn!(error = %e, "could not restrict cache file permissions");
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Item {
        title: String,
        count: u32,
    }

    fn item(n: u32) -> Item {
        Item {
            title: format!("secret project {n}"),
            count: n,
        }
    }

    fn vault(key_byte: u8, account: &str) -> Vault {
        Vault::new(&[key_byte; 32], account)
    }

    #[test]
    fn roundtrip_and_overwrite() {
        let cache = Cache::in_memory().unwrap();
        let v = vault(1, "octo@github.com");
        assert!(cache.get::<Item>(&v, "k").is_none());
        cache.put(&v, "k", &item(1)).unwrap();
        cache.put(&v, "k", &item(2)).unwrap();
        let entry = cache.get::<Item>(&v, "k").unwrap();
        assert_eq!(entry.value, item(2));
        assert!(entry.fetched_at <= Timestamp::now());
    }

    #[test]
    fn nothing_readable_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let cache = Cache::open(&path).unwrap();
        let v = vault(1, "octo@github.com");
        cache.put(&v, "focus:all", &item(7)).unwrap();
        drop(cache);
        let bytes = fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        for needle in ["secret project", "octo", "github.com"] {
            assert!(!text.contains(needle), "{needle} found in the cache file");
        }
    }

    #[test]
    fn another_key_cannot_read_and_entries_cannot_be_swapped() {
        let cache = Cache::in_memory().unwrap();
        let v = vault(1, "octo@github.com");
        cache.put(&v, "a", &item(1)).unwrap();
        cache.put(&v, "b", &item(2)).unwrap();

        // New token -> new key: old rows no longer decrypt (or even match).
        assert!(
            cache
                .get::<Item>(&vault(2, "octo@github.com"), "a")
                .is_none()
        );

        // Moving a sealed payload to another key fails authentication.
        cache
            .conn()
            .execute(
                "UPDATE entries SET payload = (SELECT payload FROM entries WHERE key = 'a')
                 WHERE key = 'b'",
                [],
            )
            .unwrap();
        assert!(cache.get::<Item>(&v, "b").is_none());
        assert!(cache.get::<Item>(&v, "a").is_some());
    }

    #[test]
    fn accounts_are_isolated_and_clearable() {
        let cache = Cache::in_memory().unwrap();
        let a = vault(1, "a@github.com");
        let b = vault(2, "b@github.com");
        cache.put(&a, "focus", &item(1)).unwrap();
        cache.put(&b, "focus", &item(2)).unwrap();
        assert_eq!(cache.get::<Item>(&a, "focus").unwrap().value, item(1));
        assert_eq!(cache.clear_account(&a).unwrap(), 1);
        assert!(cache.get::<Item>(&a, "focus").is_none());
        assert!(cache.get::<Item>(&b, "focus").is_some());
        assert_eq!(cache.clear_all().unwrap(), 1);
    }

    #[test]
    fn prune_drops_old_entries() {
        let cache = Cache::in_memory().unwrap();
        let v = vault(1, "a");
        let old = Timestamp::from_second(1_000_000).unwrap();
        cache.put_at(&v, "old", &item(1), old).unwrap();
        cache.put(&v, "new", &item(2)).unwrap();
        let cutoff = Timestamp::from_second(2_000_000).unwrap();
        assert_eq!(cache.prune(cutoff).unwrap(), 1);
        assert!(cache.get::<Item>(&v, "old").is_none());
        assert!(cache.get::<Item>(&v, "new").is_some());
    }

    #[test]
    fn corrupt_files_are_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("cache.sqlite");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"this is not a database, just garbage bytes......").unwrap();
        let v = vault(1, "a");
        let cache = Cache::open(&path).unwrap();
        cache.put(&v, "k", &item(1)).unwrap();
        drop(cache);
        let cache = Cache::open(&path).unwrap();
        assert_eq!(cache.get::<Item>(&v, "k").unwrap().value, item(1));
    }

    #[test]
    fn old_layouts_are_wiped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE entries (account TEXT, key TEXT, schema INTEGER,
                   fetched_at INTEGER, payload TEXT, PRIMARY KEY (account, key));
                 INSERT INTO entries VALUES ('octo', 'k', 1, 0, '{\"plain\":true}');",
            )
            .unwrap();
        }
        let cache = Cache::open(&path).unwrap();
        let rows: i64 = cache
            .conn()
            .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c").join("cache.sqlite");
        let _cache = Cache::open(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let mode = fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }
}
