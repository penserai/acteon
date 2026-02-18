use tracing::{debug, info};

use crate::config::AwsBaseConfig;

/// Build an AWS SDK configuration from the given [`AwsBaseConfig`].
///
/// Uses the standard AWS SDK environment credential chain and optionally:
/// - Overrides the endpoint URL for local development (e.g. `LocalStack`)
/// - Assumes an IAM role via STS if `role_arn` is configured
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

    // If a role ARN is specified, assume it via STS. We first load the base
    // config to create an STS client, then use the assumed-role credentials
    // to build the final config.
    if let Some(role_arn) = &config.role_arn {
        info!(role_arn = %role_arn, "assuming IAM role via STS");
        let base_config = loader.load().await;
        let sts_client = aws_sdk_sts::Client::new(&base_config);

        match sts_client
            .assume_role()
            .role_arn(role_arn)
            .role_session_name("acteon-aws-provider")
            .send()
            .await
        {
            Ok(response) => {
                if let Some(creds) = response.credentials() {
                    let static_creds = aws_credential_types::Credentials::from_keys(
                        creds.access_key_id(),
                        creds.secret_access_key(),
                        Some(creds.session_token().to_owned()),
                    );

                    let mut assumed_loader = aws_config::from_env()
                        .region(aws_config::Region::new(config.region.clone()))
                        .credentials_provider(static_creds);

                    if let Some(endpoint) = &config.endpoint_url {
                        assumed_loader = assumed_loader.endpoint_url(endpoint);
                    }

                    info!("STS assume-role succeeded");
                    return assumed_loader.load().await;
                }
                tracing::warn!("STS response had no credentials, falling back to base config");
            }
            Err(e) => {
                tracing::error!(error = %e, "STS assume-role failed, falling back to base config");
            }
        }
        return base_config;
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
}
