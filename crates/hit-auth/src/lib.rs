//! Accounts, secret storage and device-flow sign-in.
//!
//! Security rules (see audit D-027):
//! - tokens only ever live in [`Secret`], which never prints its content;
//! - `accounts.toml` holds no secrets, tokens live in the OS keyring;
//! - the plain-file store is used only when explicitly requested;
//! - HTTP is https-only, never follows redirects and never puts a token in a URL.

mod account;
pub mod device;
mod fsutil;
pub mod gh;
mod http;
mod resolve;
mod secret;
mod store;

pub use account::{Account, AccountKey, Accounts, AccountsError, Storage, TokenKind};
pub use http::{HttpError, client};
pub use resolve::{TokenSource, env_token};
pub use secret::{InvalidSecret, Secret};
pub use store::{FileStore, KeyringStore, MemoryStore, StoreError, TokenStore};

/// Service name used for keyring entries.
pub const KEYRING_SERVICE: &str = "hit";
