use acteon_state::StateKey;

/// Render a [`StateKey`] into a Redis key string with the given prefix.
///
/// The format is `prefix:namespace:tenant:kind:id`.
pub fn render_key(prefix: &str, key: &StateKey) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        prefix, key.namespace, key.tenant, key.kind, key.id
    )
}

#[cfg(test)]
mod tests {
    use acteon_state::KeyKind;

    use super::*;

    #[test]
    fn renders_standard_key() {
        let key = StateKey::new("notif", "tenant-1", KeyKind::Dedup, "abc-123");
        let rendered = render_key("acteon", &key);
        assert_eq!(rendered, "acteon:notif:tenant-1:dedup:abc-123");
    }

    #[test]
    fn renders_custom_kind() {
        let key = StateKey::new("ns", "t", KeyKind::Custom("my_kind".into()), "id-1");
        let rendered = render_key("pfx", &key);
        assert_eq!(rendered, "pfx:ns:t:my_kind:id-1");
    }

    #[test]
    fn renders_all_builtin_kinds() {
        let kinds = [
            (KeyKind::Dedup, "dedup"),
            (KeyKind::Counter, "counter"),
            (KeyKind::Lock, "lock"),
            (KeyKind::State, "state"),
            (KeyKind::History, "history"),
        ];
        for (kind, expected_segment) in kinds {
            let key = StateKey::new("ns", "t", kind, "id");
            let rendered = render_key("p", &key);
            assert_eq!(rendered, format!("p:ns:t:{expected_segment}:id"));
        }
    }
}
