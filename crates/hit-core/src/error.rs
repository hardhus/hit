use jiff::Timestamp;
use serde::{Deserialize, Serialize};

pub type Result<T, E = ForgeError> = std::result::Result<T, E>;

/// Errors every provider maps its failures into. `Clone` so results can be
/// passed around the UI freely.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ForgeError {
    #[error("not signed in, or the token is no longer valid")]
    Unauthorized,
    #[error("access denied: {message}")]
    Forbidden {
        message: String,
        hint: Option<AccessHint>,
    },
    #[error("not found")]
    NotFound,
    #[error("rate limit exceeded")]
    RateLimited { reset_at: Option<Timestamp> },
    #[error("network error: {0}")]
    Network(String),
    #[error("{0} is not supported by this forge")]
    Unsupported(&'static str),
    #[error("unexpected response from the forge: {0}")]
    InvalidResponse(String),
}

impl ForgeError {
    /// Whether trying again later may succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Network(_) | Self::RateLimited { .. })
    }
}

/// Why access was denied and what the user can do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccessHint {
    /// The organization restricts third-party (OAuth) app access; an owner has
    /// to approve hit at `url`.
    OrgRestricted { org: String, url: String },
    /// The token lacks a scope or permission.
    TokenScope { needed: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_errors() {
        assert!(ForgeError::Network("timeout".into()).is_retryable());
        assert!(ForgeError::RateLimited { reset_at: None }.is_retryable());
        assert!(!ForgeError::Unauthorized.is_retryable());
        assert!(!ForgeError::NotFound.is_retryable());
    }
}
