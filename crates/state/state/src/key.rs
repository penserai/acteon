use serde::{Deserialize, Serialize};

use acteon_core::{ActionKey, Namespace, TenantId};

/// The kind of state being stored.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    Dedup,
    Counter,
    Lock,
    State,
    History,
    RateLimit,
    /// Event lifecycle state (state machine position).
    EventState,
    /// Event timeout tracking.
    EventTimeout,
    /// Event group data.
    Group,
    /// Index of pending groups awaiting flush.
    PendingGroups,
    /// Index of active events for inhibition lookups.
    ActiveEvents,
    /// Approval record awaiting human decision.
    Approval,
    /// Index of pending approvals by action ID.
    PendingApprovals,
    /// Task chain execution state.
    Chain,
    /// Index of pending chain steps awaiting advancement.
    PendingChains,
    Custom(String),
}

impl KeyKind {
    /// Return a string representation of the key kind.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Dedup => "dedup",
            Self::Counter => "counter",
            Self::Lock => "lock",
            Self::State => "state",
            Self::History => "history",
            Self::RateLimit => "rate_limit",
            Self::EventState => "event_state",
            Self::EventTimeout => "event_timeout",
            Self::Group => "group",
            Self::PendingGroups => "pending_groups",
            Self::ActiveEvents => "active_events",
            Self::Approval => "approval",
            Self::PendingApprovals => "pending_approvals",
            Self::Chain => "chain",
            Self::PendingChains => "pending_chains",
            Self::Custom(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for KeyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Key used to address state entries in the store.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StateKey {
    pub namespace: Namespace,
    pub tenant: TenantId,
    pub kind: KeyKind,
    pub id: String,
}

impl StateKey {
    /// Create a new state key.
    #[must_use]
    pub fn new(
        namespace: impl Into<Namespace>,
        tenant: impl Into<TenantId>,
        kind: KeyKind,
        id: impl Into<String>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            tenant: tenant.into(),
            kind,
            id: id.into(),
        }
    }

    /// Build a dedup state key from an `ActionKey`.
    #[must_use]
    pub fn from_action_key(key: &ActionKey) -> Self {
        Self {
            namespace: key.namespace.clone(),
            tenant: key.tenant.clone(),
            kind: KeyKind::Dedup,
            id: key.canonical(),
        }
    }

    /// Return a canonical string representation: `namespace:tenant:kind:id`
    #[must_use]
    pub fn canonical(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.namespace, self.tenant, self.kind, self.id
        )
    }
}

impl std::fmt::Display for StateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.canonical())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_kind_as_str() {
        assert_eq!(KeyKind::Dedup.as_str(), "dedup");
        assert_eq!(KeyKind::Counter.as_str(), "counter");
        assert_eq!(KeyKind::Lock.as_str(), "lock");
        assert_eq!(KeyKind::State.as_str(), "state");
        assert_eq!(KeyKind::History.as_str(), "history");
        assert_eq!(KeyKind::RateLimit.as_str(), "rate_limit");
        assert_eq!(KeyKind::EventState.as_str(), "event_state");
        assert_eq!(KeyKind::EventTimeout.as_str(), "event_timeout");
        assert_eq!(KeyKind::Group.as_str(), "group");
        assert_eq!(KeyKind::PendingGroups.as_str(), "pending_groups");
        assert_eq!(KeyKind::ActiveEvents.as_str(), "active_events");
        assert_eq!(KeyKind::Approval.as_str(), "approval");
        assert_eq!(KeyKind::PendingApprovals.as_str(), "pending_approvals");
        assert_eq!(KeyKind::Chain.as_str(), "chain");
        assert_eq!(KeyKind::PendingChains.as_str(), "pending_chains");
        assert_eq!(KeyKind::Custom("foo".into()).as_str(), "foo");
    }

    #[test]
    fn state_key_canonical() {
        let key = StateKey::new("ns", "t", KeyKind::Dedup, "abc");
        assert_eq!(key.canonical(), "ns:t:dedup:abc");
    }

    #[test]
    fn from_action_key() {
        let ak = ActionKey::new("ns", "t", "act-1");
        let sk = StateKey::from_action_key(&ak);
        assert_eq!(sk.kind, KeyKind::Dedup);
        assert_eq!(sk.id, "ns:t:act-1");
    }
}
