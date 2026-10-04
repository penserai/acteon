use serde::{Deserialize, Serialize};

/// Minimal caller identity for audit threading.
///
/// This type is shared across crates so that the gateway can record
/// who triggered each action without depending on the full auth module.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Caller {
    /// Caller identifier (username or API key name).
    pub id: String,
    /// Stable authenticated actor, when explicitly bound by an auth adapter.
    /// This is audit provenance, not a permit or trusted execution context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<crate::PrincipalIdentity>,
    /// How the caller authenticated (`"jwt"`, `"api_key"`, or `"anonymous"`).
    pub auth_method: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_wire_provenance_remains_unchanged() {
        let wire = serde_json::json!({"id":"legacy-key","auth_method":"api_key"});
        let caller: Caller = serde_json::from_value(wire.clone()).unwrap();
        assert!(caller.principal.is_none());
        assert_eq!(serde_json::to_value(caller).unwrap(), wire);
    }
}
