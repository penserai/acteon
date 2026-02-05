use serde::{Deserialize, Serialize};

use crate::types::{ActionId, Namespace, TenantId};

/// Composite key that uniquely identifies an action within the system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionKey {
    pub namespace: Namespace,
    pub tenant: TenantId,
    pub action_id: ActionId,
    pub discriminator: Option<String>,
}

impl ActionKey {
    /// Create a new action key.
    #[must_use]
    pub fn new(
        namespace: impl Into<Namespace>,
        tenant: impl Into<TenantId>,
        action_id: impl Into<ActionId>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            tenant: tenant.into(),
            action_id: action_id.into(),
            discriminator: None,
        }
    }

    /// Set an optional discriminator for sub-key partitioning.
    #[must_use]
    pub fn with_discriminator(mut self, discriminator: impl Into<String>) -> Self {
        self.discriminator = Some(discriminator.into());
        self
    }

    /// Return the canonical string form: `namespace:tenant:action_id[:discriminator]`
    #[must_use]
    pub fn canonical(&self) -> String {
        match &self.discriminator {
            Some(d) => format!(
                "{}:{}:{}:{}",
                self.namespace, self.tenant, self.action_id, d
            ),
            None => format!("{}:{}:{}", self.namespace, self.tenant, self.action_id),
        }
    }
}

impl std::fmt::Display for ActionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.canonical())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_without_discriminator() {
        let key = ActionKey::new("notif", "tenant-1", "act-42");
        assert_eq!(key.canonical(), "notif:tenant-1:act-42");
    }

    #[test]
    fn canonical_with_discriminator() {
        let key = ActionKey::new("notif", "tenant-1", "act-42").with_discriminator("email");
        assert_eq!(key.canonical(), "notif:tenant-1:act-42:email");
    }

    #[test]
    fn display_matches_canonical() {
        let key = ActionKey::new("ns", "t", "a");
        assert_eq!(key.to_string(), key.canonical());
    }

    #[test]
    fn serde_roundtrip() {
        let key = ActionKey::new("ns", "t", "a").with_discriminator("d");
        let json = serde_json::to_string(&key).unwrap();
        let back: ActionKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, key);
    }
}
