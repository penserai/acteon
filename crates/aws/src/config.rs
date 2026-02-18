use serde::{Deserialize, Serialize};

/// Shared base configuration for all AWS providers.
///
/// Contains common settings like region, optional STS assume-role ARN for
/// cross-account access, and an endpoint URL override for local development
/// (e.g. `LocalStack`).
#[derive(Clone, Serialize, Deserialize)]
pub struct AwsBaseConfig {
    /// AWS region (e.g. `"us-east-1"`).
    pub region: String,

    /// Optional IAM role ARN to assume via STS for cross-account access.
    pub role_arn: Option<String>,

    /// Optional endpoint URL override for local development (e.g. `LocalStack`).
    pub endpoint_url: Option<String>,
}

impl std::fmt::Debug for AwsBaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsBaseConfig")
            .field("region", &self.region)
            .field("role_arn", &self.role_arn.as_ref().map(|_| "[REDACTED]"))
            .field("endpoint_url", &self.endpoint_url)
            .finish()
    }
}

impl AwsBaseConfig {
    /// Create a new `AwsBaseConfig` with the given region.
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            region: region.into(),
            role_arn: None,
            endpoint_url: None,
        }
    }

    /// Set an IAM role ARN to assume via STS.
    #[must_use]
    pub fn with_role_arn(mut self, role_arn: impl Into<String>) -> Self {
        self.role_arn = Some(role_arn.into());
        self
    }

    /// Set an endpoint URL override for local development.
    #[must_use]
    pub fn with_endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        self.endpoint_url = Some(endpoint_url.into());
        self
    }
}

impl Default for AwsBaseConfig {
    fn default() -> Self {
        Self {
            region: "us-east-1".to_owned(),
            role_arn: None,
            endpoint_url: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_config_sets_region() {
        let config = AwsBaseConfig::new("eu-west-1");
        assert_eq!(config.region, "eu-west-1");
        assert!(config.role_arn.is_none());
        assert!(config.endpoint_url.is_none());
    }

    #[test]
    fn with_role_arn_sets_value() {
        let config =
            AwsBaseConfig::new("us-east-1").with_role_arn("arn:aws:iam::123456789012:role/test");
        assert_eq!(
            config.role_arn.as_deref(),
            Some("arn:aws:iam::123456789012:role/test")
        );
    }

    #[test]
    fn with_endpoint_url_sets_value() {
        let config = AwsBaseConfig::new("us-east-1").with_endpoint_url("http://localhost:4566");
        assert_eq!(
            config.endpoint_url.as_deref(),
            Some("http://localhost:4566")
        );
    }

    #[test]
    fn default_config() {
        let config = AwsBaseConfig::default();
        assert_eq!(config.region, "us-east-1");
        assert!(config.role_arn.is_none());
        assert!(config.endpoint_url.is_none());
    }

    #[test]
    fn debug_redacts_role_arn() {
        let config =
            AwsBaseConfig::new("us-east-1").with_role_arn("arn:aws:iam::123456789012:role/test");
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("123456789012"));
    }

    #[test]
    fn serde_roundtrip() {
        let config = AwsBaseConfig::new("ap-southeast-1")
            .with_role_arn("arn:aws:iam::111111111111:role/cross")
            .with_endpoint_url("http://localhost:4566");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AwsBaseConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.region, "ap-southeast-1");
        assert_eq!(
            deserialized.role_arn.as_deref(),
            Some("arn:aws:iam::111111111111:role/cross")
        );
        assert_eq!(
            deserialized.endpoint_url.as_deref(),
            Some("http://localhost:4566")
        );
    }
}
