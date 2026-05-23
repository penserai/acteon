use serde::{Deserialize, Serialize};

/// Compliance mode that determines the default audit behavior.
///
/// Each mode pre-configures sensible defaults for `ComplianceConfig` fields
/// like `sync_audit_writes`, `immutable_audit`, and `hash_chain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ComplianceMode {
    /// No compliance mode — default behavior.
    #[default]
    None,
    /// `SOC2` compliance: enables synchronous audit writes and hash chaining.
    Soc2,
    /// `HIPAA` compliance: enables synchronous writes, hash chaining, and
    /// immutable audit records.
    Hipaa,
}

impl std::fmt::Display for ComplianceMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Soc2 => f.write_str("soc2"),
            Self::Hipaa => f.write_str("hipaa"),
        }
    }
}

/// Configuration for compliance-aware audit behavior.
///
/// When a `ComplianceMode` is selected, sensible defaults are applied. Fields
/// can be individually overridden after construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ComplianceConfig {
    /// The compliance mode in effect.
    #[serde(default)]
    pub mode: ComplianceMode,

    /// Whether audit writes must be synchronous (blocking the dispatch pipeline
    /// until the write is confirmed by the backend).
    #[serde(default)]
    pub sync_audit_writes: bool,

    /// Whether audit records are immutable (deletes and updates are rejected).
    #[serde(default)]
    pub immutable_audit: bool,

    /// Whether a `SHA-256` hash chain is maintained across audit records within
    /// each `(namespace, tenant)` pair.
    #[serde(default)]
    pub hash_chain: bool,
}

impl Default for ComplianceConfig {
    fn default() -> Self {
        Self::new(ComplianceMode::None)
    }
}

impl ComplianceConfig {
    /// Create a new `ComplianceConfig` with mode-appropriate defaults.
    ///
    /// - `None`: all features disabled
    /// - `Soc2`: sync writes + hash chain
    /// - `Hipaa`: sync writes + hash chain + immutable audit
    #[must_use]
    pub fn new(mode: ComplianceMode) -> Self {
        match mode {
            ComplianceMode::None => Self {
                mode: ComplianceMode::None,
                sync_audit_writes: false,
                immutable_audit: false,
                hash_chain: false,
            },
            ComplianceMode::Soc2 => Self {
                mode: ComplianceMode::Soc2,
                sync_audit_writes: true,
                immutable_audit: false,
                hash_chain: true,
            },
            ComplianceMode::Hipaa => Self {
                mode: ComplianceMode::Hipaa,
                sync_audit_writes: true,
                immutable_audit: true,
                hash_chain: true,
            },
        }
    }

    /// Override the sync audit writes setting.
    #[must_use]
    pub fn with_sync_audit_writes(mut self, enabled: bool) -> Self {
        self.sync_audit_writes = enabled;
        self
    }

    /// Override the immutable audit setting.
    #[must_use]
    pub fn with_immutable_audit(mut self, enabled: bool) -> Self {
        self.immutable_audit = enabled;
        self
    }

    /// Override the hash chain setting.
    #[must_use]
    pub fn with_hash_chain(mut self, enabled: bool) -> Self {
        self.hash_chain = enabled;
        self
    }
}

/// Result of verifying the integrity of an audit hash chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HashChainVerification {
    /// Whether the hash chain is valid (no broken links).
    pub valid: bool,
    /// Total number of records checked during verification.
    pub records_checked: u64,
    /// ID of the record where the chain first broke, if any.
    #[serde(default)]
    pub first_broken_at: Option<String>,
    /// ID of the first record in the verified range.
    #[serde(default)]
    pub first_record_id: Option<String>,
    /// ID of the last record in the verified range.
    #[serde(default)]
    pub last_record_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compliance_mode_default_is_none() {
        assert_eq!(ComplianceMode::default(), ComplianceMode::None);
    }

    #[test]
    fn compliance_mode_display() {
        assert_eq!(format!("{}", ComplianceMode::None), "none");
        assert_eq!(format!("{}", ComplianceMode::Soc2), "soc2");
        assert_eq!(format!("{}", ComplianceMode::Hipaa), "hipaa");
    }

    #[test]
    fn compliance_mode_serde_roundtrip() {
        for mode in [
            ComplianceMode::None,
            ComplianceMode::Soc2,
            ComplianceMode::Hipaa,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            let back: ComplianceMode = serde_json::from_str(&json).unwrap();
            assert_eq!(back, mode);
        }
    }

    #[test]
    fn compliance_config_none_defaults() {
        let config = ComplianceConfig::new(ComplianceMode::None);
        assert_eq!(config.mode, ComplianceMode::None);
        assert!(!config.sync_audit_writes);
        assert!(!config.immutable_audit);
        assert!(!config.hash_chain);
    }

    #[test]
    fn compliance_config_soc2_defaults() {
        let config = ComplianceConfig::new(ComplianceMode::Soc2);
        assert_eq!(config.mode, ComplianceMode::Soc2);
        assert!(config.sync_audit_writes);
        assert!(!config.immutable_audit);
        assert!(config.hash_chain);
    }

    #[test]
    fn compliance_config_hipaa_defaults() {
        let config = ComplianceConfig::new(ComplianceMode::Hipaa);
        assert_eq!(config.mode, ComplianceMode::Hipaa);
        assert!(config.sync_audit_writes);
        assert!(config.immutable_audit);
        assert!(config.hash_chain);
    }

    #[test]
    fn compliance_config_default_is_none() {
        let config = ComplianceConfig::default();
        assert_eq!(config.mode, ComplianceMode::None);
        assert!(!config.sync_audit_writes);
        assert!(!config.immutable_audit);
        assert!(!config.hash_chain);
    }

    #[test]
    fn compliance_config_builder_overrides() {
        let config = ComplianceConfig::new(ComplianceMode::Soc2)
            .with_immutable_audit(true)
            .with_sync_audit_writes(false);
        assert_eq!(config.mode, ComplianceMode::Soc2);
        assert!(!config.sync_audit_writes);
        assert!(config.immutable_audit);
        assert!(config.hash_chain);
    }

    #[test]
    fn compliance_config_serde_roundtrip() {
        let config = ComplianceConfig::new(ComplianceMode::Hipaa);
        let json = serde_json::to_string(&config).unwrap();
        let back: ComplianceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.mode, ComplianceMode::Hipaa);
        assert!(back.sync_audit_writes);
        assert!(back.immutable_audit);
        assert!(back.hash_chain);
    }

    #[test]
    fn compliance_config_deserializes_with_defaults() {
        let json = r#"{"mode": "soc2"}"#;
        let config: ComplianceConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.mode, ComplianceMode::Soc2);
        // Serde defaults (all false), NOT the mode-aware constructor defaults.
        assert!(!config.sync_audit_writes);
        assert!(!config.immutable_audit);
        assert!(!config.hash_chain);
    }

    #[test]
    fn hash_chain_verification_valid() {
        let v = HashChainVerification {
            valid: true,
            records_checked: 100,
            first_broken_at: None,
            first_record_id: Some("rec-001".into()),
            last_record_id: Some("rec-100".into()),
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: HashChainVerification = serde_json::from_str(&json).unwrap();
        assert!(back.valid);
        assert_eq!(back.records_checked, 100);
        assert!(back.first_broken_at.is_none());
        assert_eq!(back.first_record_id.as_deref(), Some("rec-001"));
        assert_eq!(back.last_record_id.as_deref(), Some("rec-100"));
    }

    #[test]
    fn hash_chain_verification_broken() {
        let v = HashChainVerification {
            valid: false,
            records_checked: 50,
            first_broken_at: Some("rec-025".into()),
            first_record_id: Some("rec-001".into()),
            last_record_id: Some("rec-050".into()),
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: HashChainVerification = serde_json::from_str(&json).unwrap();
        assert!(!back.valid);
        assert_eq!(back.first_broken_at.as_deref(), Some("rec-025"));
    }

    #[test]
    fn hash_chain_verification_deserializes_with_defaults() {
        let json = r#"{"valid": true, "records_checked": 0}"#;
        let v: HashChainVerification = serde_json::from_str(json).unwrap();
        assert!(v.valid);
        assert_eq!(v.records_checked, 0);
        assert!(v.first_broken_at.is_none());
        assert!(v.first_record_id.is_none());
        assert!(v.last_record_id.is_none());
    }

    #[test]
    fn compliance_config_with_hash_chain_override() {
        let config = ComplianceConfig::new(ComplianceMode::None).with_hash_chain(true);
        assert_eq!(config.mode, ComplianceMode::None);
        assert!(config.hash_chain);
        assert!(!config.sync_audit_writes);
    }
}
