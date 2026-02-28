use serde::{Deserialize, Serialize};

/// Shared base configuration for all GCP providers.
///
/// Contains common settings like project ID, service account credentials path,
/// and an optional endpoint URL override for local development (e.g. emulators).
#[derive(Clone, Serialize, Deserialize)]
pub struct GcpBaseConfig {
    /// GCP project ID.
    pub project_id: String,

    /// Path to a service account JSON key file.
    /// If not set, Application Default Credentials (ADC) are used.
    #[serde(default)]
    pub credentials_path: Option<String>,

    /// Inline service account JSON key.
    /// Supports `ENC[...]` values.
    #[serde(default)]
    pub credentials_json: Option<String>,

    /// Optional endpoint URL override for local development
    /// (e.g. `Pub/Sub` emulator, `fake-gcs-server`).
    #[serde(default)]
    pub endpoint_url: Option<String>,
}

impl std::fmt::Debug for GcpBaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcpBaseConfig")
            .field("project_id", &self.project_id)
            .field(
                "credentials_path",
                &self.credentials_path.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "credentials_json",
                &self.credentials_json.as_ref().map(|_| "[REDACTED]"),
            )
            .field("endpoint_url", &self.endpoint_url)
            .finish()
    }
}

impl GcpBaseConfig {
    /// Create a new `GcpBaseConfig` with the given project ID.
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            credentials_path: None,
            credentials_json: None,
            endpoint_url: None,
        }
    }

    /// Set the path to a service account JSON key file.
    #[must_use]
    pub fn with_credentials_path(mut self, path: impl Into<String>) -> Self {
        self.credentials_path = Some(path.into());
        self
    }

    /// Set the inline service account JSON key.
    #[must_use]
    pub fn with_credentials_json(mut self, json: impl Into<String>) -> Self {
        self.credentials_json = Some(json.into());
        self
    }

    /// Set the endpoint URL override for local development.
    #[must_use]
    pub fn with_endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        self.endpoint_url = Some(endpoint_url.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_config_sets_project_id() {
        let config = GcpBaseConfig::new("my-project");
        assert_eq!(config.project_id, "my-project");
        assert!(config.credentials_path.is_none());
        assert!(config.endpoint_url.is_none());
    }

    #[test]
    fn builder_chain() {
        let config = GcpBaseConfig::new("test-project")
            .with_credentials_path("/path/to/sa.json")
            .with_endpoint_url("http://localhost:8085");
        assert_eq!(config.project_id, "test-project");
        assert_eq!(config.credentials_path.as_deref(), Some("/path/to/sa.json"));
        assert_eq!(
            config.endpoint_url.as_deref(),
            Some("http://localhost:8085")
        );
    }

    #[test]
    fn debug_redacts_credentials_path() {
        let config =
            GcpBaseConfig::new("my-project").with_credentials_path("/home/user/sa-key.json");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("sa-key.json"));
        assert!(debug.contains("my-project"));
    }

    #[test]
    fn serde_roundtrip() {
        let config = GcpBaseConfig::new("round-trip").with_endpoint_url("http://localhost:8085");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: GcpBaseConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.project_id, "round-trip");
        assert_eq!(
            deserialized.endpoint_url.as_deref(),
            Some("http://localhost:8085")
        );
        assert!(deserialized.credentials_path.is_none());
    }
}
