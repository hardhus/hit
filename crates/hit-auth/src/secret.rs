use std::fmt;

use secrecy::{ExposeSecret, SecretString};

/// A token or other credential. Never printed: `Debug` is redacted, there is
/// no `Display` or `Serialize`, and the memory is zeroed on drop.
#[derive(Clone)]
pub struct Secret(SecretString);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSecret {
    #[error("the token is empty")]
    Empty,
    #[error("the token is too long")]
    TooLong,
    /// Whitespace or control characters would allow header injection.
    #[error("the token contains invalid characters")]
    InvalidCharacters,
}

impl Secret {
    const MAX_LEN: usize = 1024;

    /// Validates and wraps a token. Surrounding whitespace is trimmed.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidSecret> {
        let value: String = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(InvalidSecret::Empty);
        }
        if trimmed.len() > Self::MAX_LEN {
            return Err(InvalidSecret::TooLong);
        }
        if !trimmed.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(InvalidSecret::InvalidCharacters);
        }
        Ok(Self(SecretString::from(trimmed.to_owned())))
    }

    /// The raw value. Only call this right where it is sent or stored.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let secret = Secret::new("gho_supersecret").unwrap();
        let debug = format!("{secret:?} {:?}", Some(&secret));
        assert!(!debug.contains("supersecret"), "{debug}");
        assert_eq!(secret.expose(), "gho_supersecret");
    }

    #[test]
    fn trims_and_validates() {
        assert_eq!(Secret::new("  tok \n").unwrap().expose(), "tok");
        assert_eq!(Secret::new(" \n").unwrap_err(), InvalidSecret::Empty);
        assert_eq!(
            Secret::new("a b").unwrap_err(),
            InvalidSecret::InvalidCharacters
        );
        assert_eq!(
            Secret::new("tok\r\nX-Evil: 1").unwrap_err(),
            InvalidSecret::InvalidCharacters
        );
        assert_eq!(
            Secret::new("x".repeat(2000)).unwrap_err(),
            InvalidSecret::TooLong
        );
    }
}
