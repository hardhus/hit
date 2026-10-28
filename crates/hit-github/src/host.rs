//! GitHub hosts: github.com and GitHub Enterprise Server. https only.

use hit_core::Host;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("{0:?} is not a valid host name")]
    Invalid(String),
    #[error("only https is supported, got {0:?}")]
    InsecureScheme(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubHost {
    host: Host,
    web: Url,
    api: Url,
    graphql: Url,
}

impl GithubHost {
    /// Accepts `github.com`, `ghe.example.com[:port]` or an `https://` URL.
    /// Lower-cases, drops paths and rejects anything that is not https.
    pub fn parse(input: &str) -> Result<Self, HostError> {
        let input = input.trim();
        let invalid = || HostError::Invalid(input.to_owned());
        let authority = if input.contains("://") {
            let url = Url::parse(input).map_err(|_| invalid())?;
            if url.scheme() != "https" {
                return Err(HostError::InsecureScheme(url.scheme().to_owned()));
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err(invalid());
            }
            let host = url.host_str().ok_or_else(invalid)?;
            match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            }
        } else {
            input.trim_end_matches('/').to_owned()
        };

        let authority = authority.to_ascii_lowercase();
        let (name, port) = match authority.rsplit_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (authority.as_str(), None),
        };
        let valid_name = !name.is_empty()
            && name.len() <= 253
            && !name.starts_with(['-', '.'])
            && !name.ends_with(['-', '.'])
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
        let valid_port = port.is_none_or(|p| p.parse::<u16>().is_ok_and(|p| p > 0));
        if !valid_name || !valid_port {
            return Err(invalid());
        }

        let canonical = match authority.as_str() {
            "api.github.com" | "www.github.com" => "github.com",
            other => other,
        };
        let parse = |s: String| Url::parse(&s).map_err(|_| invalid());
        if canonical == "github.com" {
            Ok(Self {
                host: Host::new("github.com"),
                web: parse("https://github.com/".to_owned())?,
                api: parse("https://api.github.com/".to_owned())?,
                graphql: parse("https://api.github.com/graphql".to_owned())?,
            })
        } else {
            Ok(Self {
                host: Host::new(canonical),
                web: parse(format!("https://{canonical}/"))?,
                api: parse(format!("https://{canonical}/api/v3/"))?,
                graphql: parse(format!("https://{canonical}/api/graphql"))?,
            })
        }
    }

    pub fn host(&self) -> &Host {
        &self.host
    }

    pub fn is_github_com(&self) -> bool {
        self.host.as_str() == "github.com"
    }

    /// `https://github.com/`
    pub fn web(&self) -> &Url {
        &self.web
    }

    /// REST base, ending in `/`.
    pub fn api(&self) -> &Url {
        &self.api
    }

    pub fn graphql(&self) -> &Url {
        &self.graphql
    }

    pub fn device_code_url(&self) -> Url {
        self.web_path("login/device/code")
    }

    pub fn token_url(&self) -> Url {
        self.web_path("login/oauth/access_token")
    }

    /// Where users review (and revoke) hit's OAuth grant and request
    /// organization access.
    pub fn oauth_app_settings(&self, client_id: &str) -> Url {
        self.web_path(&format!("settings/connections/applications/{client_id}"))
    }

    pub fn token_settings(&self) -> Url {
        self.web_path("settings/tokens")
    }

    fn web_path(&self, path: &str) -> Url {
        self.web.join(path).unwrap_or_else(|_| self.web.clone())
    }

    /// Whether `url` is an https URL on this host's web origin. Used before
    /// opening anything a server sent us in the browser.
    pub fn is_own_web_url(&self, url: &Url) -> bool {
        url.scheme() == "https"
            && url.host_str() == self.web.host_str()
            && url.port_or_known_default() == self.web.port_or_known_default()
    }

    /// Points every endpoint at a mock server.
    #[cfg(feature = "test-http")]
    pub fn for_tests(host: &str, base: &Url) -> Self {
        Self {
            host: Host::new(host),
            web: base.clone(),
            api: base.join("api/").unwrap_or_else(|_| base.clone()),
            graphql: base.join("api/graphql").unwrap_or_else(|_| base.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_com_endpoints() {
        let host = GithubHost::parse("GitHub.com").unwrap();
        assert!(host.is_github_com());
        assert_eq!(host.api().as_str(), "https://api.github.com/");
        assert_eq!(
            host.device_code_url().as_str(),
            "https://github.com/login/device/code"
        );
        assert_eq!(GithubHost::parse("https://api.github.com/x").unwrap(), host);
    }

    #[test]
    fn enterprise_endpoints() {
        let host = GithubHost::parse("https://GHE.example.com:8443/some/path").unwrap();
        assert_eq!(host.host().as_str(), "ghe.example.com:8443");
        assert_eq!(host.api().as_str(), "https://ghe.example.com:8443/api/v3/");
        assert_eq!(
            host.graphql().as_str(),
            "https://ghe.example.com:8443/api/graphql"
        );
    }

    #[test]
    fn rejects_http_and_garbage() {
        assert_eq!(
            GithubHost::parse("http://ghe.example.com"),
            Err(HostError::InsecureScheme("http".into()))
        );
        for bad in [
            "",
            "-evil",
            "a b",
            "host/../x",
            "user@host",
            "https://user:pw@host",
            "host:0",
            "host:99999",
            ".host",
        ] {
            assert!(GithubHost::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn only_own_https_urls_are_trusted() {
        let host = GithubHost::parse("github.com").unwrap();
        let ok = Url::parse("https://github.com/login/device").unwrap();
        assert!(host.is_own_web_url(&ok));
        for bad in [
            "http://github.com/login/device",
            "https://github.com.evil.example/login/device",
            "https://evil.example/?github.com",
            "https://github.com:444/login/device",
        ] {
            assert!(!host.is_own_web_url(&Url::parse(bad).unwrap()), "{bad}");
        }
    }
}
