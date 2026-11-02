//! Organizations, for `hit auth status`. GitHub has no API that reveals an
//! OAuth app access restriction, so status lists the orgs and points users at
//! the page where they can check and request access.

use hit_core::{AccessHint, ForgeError};
use serde::Deserialize;

use crate::{Api, api::parse_sso};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Org {
    pub login: String,
}

/// The user's organizations, plus an SSO hint when some were left out.
pub async fn orgs(api: &Api) -> Result<(Vec<Org>, Option<AccessHint>), ForgeError> {
    let response = api.get("user/orgs?per_page=100").await?;
    let hint = response
        .headers()
        .get("x-github-sso")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_sso);
    let orgs = response
        .json()
        .await
        .map_err(|e| ForgeError::InvalidResponse(format!("GET /user/orgs: {e}")))?;
    Ok((orgs, hint))
}
