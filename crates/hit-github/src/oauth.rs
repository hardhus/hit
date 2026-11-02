//! hit's GitHub OAuth app: client id and scopes. The device flow needs no
//! client secret, and hit has none.

use crate::GithubHost;

/// Client id of the `hit` OAuth app on github.com (owned by @hardhus, device
/// flow enabled, non-expiring tokens). Not secret: it identifies the app,
/// nothing more. Forks can override it with `HIT_GITHUB_CLIENT_ID`.
pub const BUILT_IN_CLIENT_ID: &str = "Ov23liJjflVaF9Re0SGU";

/// Everything hit supports: private repositories, organizations and
/// notifications. `repo` also allows writes; hit's read-only mode blocks those
/// locally.
pub const SCOPES: [&str; 3] = ["repo", "read:org", "notifications"];

/// Public repositories only.
pub const MINIMAL_SCOPES: [&str; 3] = ["public_repo", "read:org", "notifications"];

/// The client id to use for `host`: the configured one, then (github.com
/// only) `HIT_GITHUB_CLIENT_ID` at runtime, then the built-in one.
pub fn client_id(
    host: &GithubHost,
    configured: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let candidate = match configured {
        Some(id) => Some(id.to_owned()),
        None if host.is_github_com() => {
            Some(env("HIT_GITHUB_CLIENT_ID").unwrap_or_else(|| BUILT_IN_CLIENT_ID.to_owned()))
        }
        None => None,
    };
    candidate.filter(|id| is_valid_client_id(id))
}

fn is_valid_client_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Required scopes the token lacks. Accounts for implied scopes (`repo`
/// covers `public_repo`; `write:org`/`admin:org` cover `read:org`).
pub fn missing_scopes(granted: &[String], required: &[&str]) -> Vec<String> {
    let has = |scope: &str| granted.iter().any(|g| g == scope);
    required
        .iter()
        .filter(|&&scope| {
            let implied = match scope {
                "public_repo" => has("repo"),
                "read:org" => has("write:org") || has("admin:org"),
                _ => false,
            };
            !has(scope) && !implied
        })
        .map(|s| (*s).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn missing_scopes_respects_implied_ones() {
        assert!(
            missing_scopes(&strings(&["repo", "read:org", "notifications"]), &SCOPES).is_empty()
        );
        assert!(
            missing_scopes(
                &strings(&["repo", "admin:org", "notifications"]),
                &MINIMAL_SCOPES
            )
            .is_empty()
        );
        assert_eq!(
            missing_scopes(&strings(&["public_repo"]), &SCOPES),
            ["repo", "read:org", "notifications"]
        );
    }

    #[test]
    fn client_id_resolution() {
        let gh = GithubHost::parse("github.com").unwrap();
        let ghe = GithubHost::parse("ghe.example.com").unwrap();
        let env = |k: &str| (k == "HIT_GITHUB_CLIENT_ID").then(|| "Ov23envid".to_owned());
        let no_env = |_: &str| None;

        assert_eq!(client_id(&gh, Some("cfg"), &env).as_deref(), Some("cfg"));
        assert_eq!(client_id(&gh, None, &env).as_deref(), Some("Ov23envid"));
        assert_eq!(
            client_id(&gh, None, &no_env).as_deref(),
            Some(BUILT_IN_CLIENT_ID)
        );
        assert!(is_valid_client_id(BUILT_IN_CLIENT_ID));
        // A github.com client id is never used for another host.
        assert_eq!(client_id(&ghe, None, &env), None);
        assert_eq!(
            client_id(&ghe, Some("ghe-app"), &no_env).as_deref(),
            Some("ghe-app")
        );
        assert_eq!(client_id(&gh, Some("bad id&x=1"), &no_env), None);
    }
}
