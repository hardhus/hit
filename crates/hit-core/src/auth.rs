//! Forge-agnostic sign-in. The TUI drives a login through these traits
//! without knowing the provider.

use std::sync::Arc;

use async_trait::async_trait;
use jiff::Timestamp;

use crate::{Forge, ForgeError, Host};

/// What the user needs to finish a device-flow sign-in in the browser. Holds
/// nothing secret: the device code itself stays inside the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub host: Host,
    /// Short code the user types in the browser, e.g. `ABCD-1234`.
    pub user_code: String,
    /// Validated `https` URL on the forge's host.
    pub verification_uri: String,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LoginError {
    #[error("sign-in was cancelled in the browser")]
    Denied,
    #[error("the code expired before it was entered; start again")]
    Expired,
    #[error("sign-in is not available: {0}")]
    Unavailable(String),
    #[error("could not store the token: {0}")]
    Storage(String),
    #[error(transparent)]
    Forge(#[from] ForgeError),
}

/// Starts device-flow sign-ins for one host.
#[async_trait]
pub trait LoginFlow: Send + Sync {
    fn host(&self) -> &Host;

    async fn start(&self) -> Result<Box<dyn PendingLogin>, LoginError>;
}

/// A sign-in waiting for the user. Dropping it (or the `finish` future)
/// cancels the sign-in.
#[async_trait]
pub trait PendingLogin: Send + Sync {
    fn code(&self) -> &DeviceCode;

    /// Waits for approval, then verifies and stores the account.
    async fn finish(self: Box<Self>) -> Result<Arc<dyn Forge>, LoginError>;
}
