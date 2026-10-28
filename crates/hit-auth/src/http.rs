use std::time::Duration;

use reqwest::{Client, redirect};

#[derive(Debug, thiserror::Error)]
#[error("could not set up the HTTP client: {0}")]
pub struct HttpError(String);

/// The HTTP client used for everything that carries a token: https only, no
/// redirects (a redirect could forward credentials elsewhere), bounded
/// timeouts and an identifying user agent. TLS uses rustls with the platform
/// certificate verifier, so corporate CAs (GHE) work.
pub fn client() -> Result<Client, HttpError> {
    let builder = Client::builder()
        .redirect(redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("hit/", env!("CARGO_PKG_VERSION")));
    #[cfg(not(feature = "test-http"))]
    let builder = builder.https_only(true);
    builder.build().map_err(|e| HttpError(e.to_string()))
}
