use std::time::Duration;

/// Configuration for the etcd state store and distributed lock backends.
#[derive(Debug, Clone)]
pub struct EtcdConfig {
    /// etcd endpoint URLs (e.g. `["http://localhost:2379"]`).
    pub endpoints: Vec<String>,

    /// Key prefix applied to every etcd key to avoid collisions.
    pub prefix: String,

    /// Timeout for establishing a connection to etcd.
    pub connect_timeout: Duration,
}

impl Default for EtcdConfig {
    fn default() -> Self {
        Self {
            endpoints: vec![String::from("http://localhost:2379")],
            prefix: String::from("acteon"),
            connect_timeout: Duration::from_secs(5),
        }
    }
}

impl EtcdConfig {
    /// Build the full etcd key for a state entry.
    ///
    /// Format: `{prefix}/{namespace}/{tenant}/{kind}/{id}`
    pub(crate) fn render_key(&self, key: &acteon_state::key::StateKey) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            self.prefix, key.namespace, key.tenant, key.kind, key.id
        )
    }

    /// Build the full etcd key for a lock entry.
    ///
    /// Format: `{prefix}/_locks/{name}`
    pub(crate) fn lock_key(&self, name: &str) -> String {
        format!("{}/_locks/{}", self.prefix, name)
    }
}

#[cfg(test)]
mod tests {
    use acteon_state::key::{KeyKind, StateKey};

    use super::*;

    #[test]
    fn default_values() {
        let cfg = EtcdConfig::default();
        assert_eq!(cfg.endpoints, vec!["http://localhost:2379"]);
        assert_eq!(cfg.prefix, "acteon");
        assert_eq!(cfg.connect_timeout, Duration::from_secs(5));
    }

    #[test]
    fn render_key_format() {
        let cfg = EtcdConfig::default();
        let key = StateKey::new("notif", "tenant-1", KeyKind::Dedup, "abc-123");
        assert_eq!(cfg.render_key(&key), "acteon/notif/tenant-1/dedup/abc-123");
    }

    #[test]
    fn render_key_custom_kind() {
        let cfg = EtcdConfig {
            prefix: String::from("pfx"),
            ..EtcdConfig::default()
        };
        let key = StateKey::new("ns", "t", KeyKind::Custom("my_kind".into()), "id-1");
        assert_eq!(cfg.render_key(&key), "pfx/ns/t/my_kind/id-1");
    }

    #[test]
    fn lock_key_format() {
        let cfg = EtcdConfig::default();
        assert_eq!(cfg.lock_key("my-lock"), "acteon/_locks/my-lock");
    }

    #[test]
    fn render_all_builtin_kinds() {
        let cfg = EtcdConfig {
            prefix: String::from("p"),
            ..EtcdConfig::default()
        };
        let kinds = [
            (KeyKind::Dedup, "dedup"),
            (KeyKind::Counter, "counter"),
            (KeyKind::Lock, "lock"),
            (KeyKind::State, "state"),
            (KeyKind::History, "history"),
        ];
        for (kind, expected_segment) in kinds {
            let key = StateKey::new("ns", "t", kind, "id");
            assert_eq!(
                cfg.render_key(&key),
                format!("p/ns/t/{expected_segment}/id")
            );
        }
    }
}
