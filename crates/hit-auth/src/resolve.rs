//! Where a token comes from. Environment variables win over stored tokens and
//! are never persisted.

use std::fmt;

use hit_core::Host;

use crate::Secret;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Env(&'static str),
    Keyring,
    /// Plain file (`--insecure-storage`).
    File,
}

impl fmt::Display for TokenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(var) => write!(f, "environment ({var})"),
            Self::Keyring => f.write_str("system keyring"),
            Self::File => f.write_str("plain file (insecure storage)"),
        }
    }
}

const GITHUB_COM_VARS: [&str; 2] = ["GH_TOKEN", "GITHUB_TOKEN"];
const ENTERPRISE_VARS: [&str; 2] = ["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];

/// A token from the environment for `host`, if set. `github.com` uses
/// `GH_TOKEN`/`GITHUB_TOKEN`; other hosts the `*_ENTERPRISE_TOKEN` variables,
/// so a github.com token is never sent to another host.
pub fn env_token(
    host: &Host,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<(Secret, TokenSource)> {
    let vars = if host.as_str() == "github.com" {
        GITHUB_COM_VARS
    } else {
        ENTERPRISE_VARS
    };
    vars.into_iter().find_map(|var| {
        let value = env(var)?;
        match Secret::new(value) {
            Ok(secret) => Some((secret, TokenSource::Env(var))),
            Err(e) => {
                tracing::warn!(var, error = %e, "ignoring invalid token in environment");
                None
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn github_com_prefers_gh_token() {
        let env = env(&[("GITHUB_TOKEN", "b"), ("GH_TOKEN", "a")]);
        let (secret, source) = env_token(&Host::new("github.com"), &env).unwrap();
        assert_eq!(
            (secret.expose(), source),
            ("a", TokenSource::Env("GH_TOKEN"))
        );
    }

    #[test]
    fn enterprise_hosts_never_get_github_com_tokens() {
        let env = env(&[("GH_TOKEN", "a")]);
        assert!(env_token(&Host::new("ghe.example.com"), &env).is_none());
        let env = self::env(&[("GH_ENTERPRISE_TOKEN", "e")]);
        let (secret, _) = env_token(&Host::new("ghe.example.com"), &env).unwrap();
        assert_eq!(secret.expose(), "e");
        assert!(env_token(&Host::new("github.com"), &env).is_none());
    }

    #[test]
    fn invalid_env_tokens_are_ignored() {
        let env = env(&[("GH_TOKEN", "  "), ("GITHUB_TOKEN", "ok")]);
        let (secret, _) = env_token(&Host::new("github.com"), &env).unwrap();
        assert_eq!(secret.expose(), "ok");
    }
}
