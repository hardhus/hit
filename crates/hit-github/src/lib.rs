//! GitHub provider for hit.
//!
//! F2 provides sign-in and token verification; the data capabilities arrive
//! in F3.

mod api;
mod forge;
mod host;
mod login;
pub mod oauth;
mod orgs;
mod verify;

pub use api::{Api, ApiOptions, guard_graphql};
pub use forge::GithubForge;
pub use host::{GithubHost, HostError};
pub use login::{GithubLogin, LoginSettings, LoginStorage};
pub use orgs::{Org, orgs};
pub use verify::{Viewer, notifications_access, token_kind, verify};
