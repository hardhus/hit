//! Forge-agnostic core of hit: identifiers, domain models, queries, errors and
//! the capability traits every forge provider implements.
//!
//! Nothing here knows about networking, the TUI or any specific forge.

pub mod auth;
pub mod diff;
mod error;
pub mod focus;
mod forge;
mod forges;
mod id;
pub mod model;
pub mod query;

pub use error::{AccessHint, ForgeError, Result};
pub use forge::{Focus, Forge, Issues, Notifications, PullRequests, Repositories};
pub use forges::Forges;
pub use id::{AccountId, ForgeKind, Host, ItemKind, ItemRef, ParseRepoRefError, RepoRef};

pub use async_trait::async_trait;
pub use jiff::Timestamp;
