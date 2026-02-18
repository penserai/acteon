use tracing::{debug, info};

use crate::config::AwsBaseConfig;

/// Build an AWS SDK configuration from the given [`AwsBaseConfig`].
///
/// Uses the standard AWS SDK environment credential chain and optionally:
/// - Overrides the endpoint URL for local development (e.g. `LocalStack`)
/// - Assumes an IAM role via STS if `role_arn` is configured, with automatic
///   credential refresh before expiry
///
/// # Examples
///
/// ```no_run
/// use acteon_aws::config::AwsBaseConfig;
/// use acteon_aws::auth::build_sdk_config;
///
/// # async fn example() {
/// let config = AwsBaseConfig::new("us-east-1")
///     .with_endpoint_url("http://localhost:4566");
/// let sdk_config = build_sdk_config(&config).await;
/// # }
/// ```
pub async fn build_sdk_config(config: &AwsBaseConfig) -> aws_config::SdkConfig {
    let mut loader = aws_config::from_env().region(aws_config::Region::new(config.region.clone()));

    if let Some(endpoint) = &config.endpoint_url {
        debug!(endpoint = %endpoint, "using custom AWS endpoint");
        loader = loader.endpoint_url(endpoint);
    }

    // If a role ARN is specified, assume it via STS with automatic credential
    // refresh. The SDK's `AssumeRoleProvider` handles refreshing credentials
    // before they expire, eliminating the previous issue where static
    // credentials would expire after ~1 hour.
    if let Some(role_arn) = &config.role_arn {
        let session_name = config
            .session_name
            .as_deref()
            .unwrap_or("acteon-aws-provider");

        info!(role_arn = %role_arn, session_name = %session_name, "assuming IAM role via STS (auto-refresh)");

        // Load the base config first so the assume-role provider inherits
        // the endpoint override and base credentials for STS calls.
        let base_config = loader.load().await;

        let mut provider_builder = aws_config::sts::AssumeRoleProvider::builder(role_arn)
            .session_name(session_name)
            .region(aws_config::Region::new(config.region.clone()));

        if let Some(ref external_id) = config.external_id {
            provider_builder = provider_builder.external_id(external_id);
        }

        let assume_role_provider = provider_builder.configure(&base_config).build().await;

        // Build the final config with the auto-refreshing credential provider.
        let mut final_loader = aws_config::from_env()
            .region(aws_config::Region::new(config.region.clone()))
            .credentials_provider(assume_role_provider);

        if let Some(endpoint) = &config.endpoint_url {
            final_loader = final_loader.endpoint_url(endpoint);
        }

        info!("STS assume-role configured (credentials will auto-refresh)");
        return final_loader.load().await;
    }

    loader.load().await
}

#[cfg(all(test, feature = "integration"))]
mod integration_tests {
    use super::*;

    // These tests require a TLS root certificate store and are only run in
    // integration test mode. The AWS SDK panics on `load()` if no system
    // root certificates are available.

    #[tokio::test]
    async fn build_sdk_config_sets_region() {
        let config = AwsBaseConfig::new("ap-northeast-1");
        let sdk_config = build_sdk_config(&config).await;
        assert_eq!(
            sdk_config.region().map(|r| r.as_ref()),
            Some("ap-northeast-1")
        );
    }

    #[tokio::test]
    async fn build_sdk_config_with_endpoint() {
        let config = AwsBaseConfig::new("us-west-2").with_endpoint_url("http://localhost:4566");
        let sdk_config = build_sdk_config(&config).await;
        assert_eq!(sdk_config.region().map(|r| r.as_ref()), Some("us-west-2"));
    }

    #[tokio::test]
    async fn build_sdk_config_with_session_name() {
        let config = AwsBaseConfig::new("us-east-1").with_session_name("my-custom-session");
        let sdk_config = build_sdk_config(&config).await;
        assert_eq!(sdk_config.region().map(|r| r.as_ref()), Some("us-east-1"));
    }
}
