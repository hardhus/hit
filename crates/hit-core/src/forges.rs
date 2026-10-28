use std::sync::Arc;

use crate::{AccountId, Forge, ForgeKind, Host};

/// Every forge account hit is connected to. The UI only sees providers
/// through this registry.
#[derive(Clone, Default)]
pub struct Forges {
    forges: Vec<Arc<dyn Forge>>,
}

impl Forges {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, forge: Arc<dyn Forge>) {
        self.forges.push(forge);
    }

    pub fn is_empty(&self) -> bool {
        self.forges.is_empty()
    }

    pub fn len(&self) -> usize {
        self.forges.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Forge>> {
        self.forges.iter()
    }

    pub fn by_account(&self, account: &AccountId) -> Option<&Arc<dyn Forge>> {
        self.forges.iter().find(|f| f.account() == account)
    }

    /// The first account on `host`.
    pub fn by_host(&self, host: &Host) -> Option<&Arc<dyn Forge>> {
        self.forges.iter().find(|f| &f.account().host == host)
    }

    /// Whether this is the demo forge only (`hit --demo`).
    pub fn is_demo(&self) -> bool {
        !self.forges.is_empty() && self.forges.iter().all(|f| f.kind() == ForgeKind::Demo)
    }
}

impl std::fmt::Debug for Forges {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.forges.iter().map(|forge| forge.account().to_string()))
            .finish()
    }
}
