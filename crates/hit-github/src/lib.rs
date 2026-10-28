//! GitHub provider for hit: sign-in, token verification and the read
//! capabilities (REST + hand-written GraphQL).

mod api;
mod forge;
mod host;
mod login;
mod nodes;
pub mod oauth;
mod orgs;
mod rest;
mod verify;

pub use api::{Api, ApiOptions, guard_graphql};
pub use forge::{FocusOptions, GithubForge};
pub use host::{GithubHost, HostError};
pub use login::{GithubLogin, LoginSettings, LoginStorage};
pub use orgs::{Org, orgs};
pub use verify::{Viewer, notifications_access, token_kind, verify};
