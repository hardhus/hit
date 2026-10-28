use std::sync::Arc;

use hit_core::{AccountId, Forge, ForgeKind};

use crate::Api;

/// A signed-in GitHub account. Data capabilities arrive in F3; until then it
/// only identifies the account.
#[derive(Debug, Clone)]
pub struct GithubForge {
    account: AccountId,
    api: Arc<Api>,
}

impl GithubForge {
    pub fn new(account: AccountId, api: Api) -> Self {
        Self {
            account,
            api: Arc::new(api),
        }
    }

    pub fn api(&self) -> &Api {
        &self.api
    }
}

impl Forge for GithubForge {
    fn kind(&self) -> ForgeKind {
        ForgeKind::GitHub
    }

    fn account(&self) -> &AccountId {
        &self.account
    }
}
