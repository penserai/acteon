use acteon_core::{Action, ProviderResponse};
use acteon_provider::ProviderError;
use acteon_provider::ResourceLookup;
use acteon_provider::provider::Provider;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument};

use crate::auth::build_sdk_config;
use crate::config::AwsBaseConfig;
use crate::error::classify_sdk_error;

/// Configuration for the AWS Auto Scaling provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct AutoScalingConfig {
    /// Shared AWS configuration (region, role ARN, endpoint URL).
    #[serde(flatten)]
    pub aws: AwsBaseConfig,
}

impl std::fmt::Debug for AutoScalingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoScalingConfig")
            .field("aws", &self.aws)
            .finish()
    }
}

impl AutoScalingConfig {
    /// Create a new `AutoScalingConfig` with the given AWS region.
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            aws: AwsBaseConfig::new(region),
        }
    }

    /// Set the endpoint URL override (for `LocalStack`).
    #[must_use]
    pub fn with_endpoint_url(mut self, endpoint_url: impl Into<String>) -> Self {
        self.aws.endpoint_url = Some(endpoint_url.into());
        self
    }

    /// Set the IAM role ARN to assume.
    #[must_use]
    pub fn with_role_arn(mut self, role_arn: impl Into<String>) -> Self {
        self.aws.role_arn = Some(role_arn.into());
        self
    }

    /// Set the STS session name for assume-role.
    #[must_use]
    pub fn with_session_name(mut self, session_name: impl Into<String>) -> Self {
        self.aws.session_name = Some(session_name.into());
        self
    }

    /// Set the external ID for cross-account trust policies.
    #[must_use]
    pub fn with_external_id(mut self, external_id: impl Into<String>) -> Self {
        self.aws.external_id = Some(external_id.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Payload types
// ---------------------------------------------------------------------------

/// Payload for the `describe_auto_scaling_groups` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsgDescribePayload {
    /// Auto Scaling Group names to describe. If empty, describes all groups.
    #[serde(default)]
    pub auto_scaling_group_names: Vec<String>,
}

/// Payload for the `set_desired_capacity` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsgSetCapacityPayload {
    /// Auto Scaling Group name.
    pub auto_scaling_group_name: String,

    /// Desired capacity to set.
    pub desired_capacity: i32,

    /// Whether to honor the group's cooldown period.
    #[serde(default)]
    pub honor_cooldown: bool,
}

/// Payload for the `update_auto_scaling_group` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsgUpdatePayload {
    /// Auto Scaling Group name.
    pub auto_scaling_group_name: String,

    /// New minimum size.
    #[serde(default)]
    pub min_size: Option<i32>,

    /// New maximum size.
    #[serde(default)]
    pub max_size: Option<i32>,

    /// New desired capacity.
    #[serde(default)]
    pub desired_capacity: Option<i32>,

    /// New default cooldown period in seconds.
    #[serde(default)]
    pub default_cooldown: Option<i32>,

    /// New health check type (`"EC2"` or `"ELB"`).
    #[serde(default)]
    pub health_check_type: Option<String>,

    /// New health check grace period in seconds.
    #[serde(default)]
    pub health_check_grace_period: Option<i32>,
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// AWS Auto Scaling provider for managing Auto Scaling Groups.
pub struct AutoScalingProvider {
    config: AutoScalingConfig,
    client: aws_sdk_autoscaling::Client,
}

impl std::fmt::Debug for AutoScalingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoScalingProvider")
            .field("config", &self.config)
            .field("client", &"<AutoScalingClient>")
            .finish()
    }
}

impl AutoScalingProvider {
    /// Create a new `AutoScalingProvider` by building an AWS SDK client.
    pub async fn new(config: AutoScalingConfig) -> Self {
        let sdk_config = build_sdk_config(&config.aws).await;
        let client = aws_sdk_autoscaling::Client::new(&sdk_config);
        Self { config, client }
    }

    /// Create an `AutoScalingProvider` with a pre-built client (for testing).
    pub fn with_client(config: AutoScalingConfig, client: aws_sdk_autoscaling::Client) -> Self {
        Self { config, client }
    }
}

impl Provider for AutoScalingProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "aws-autoscaling"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "aws-autoscaling"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        match action.action_type.as_str() {
            "describe_auto_scaling_groups" => self.describe_groups(action).await,
            "set_desired_capacity" => self.set_desired_capacity(action).await,
            "update_auto_scaling_group" => self.update_group(action).await,
            other => Err(ProviderError::Configuration(format!(
                "unknown Auto Scaling action type '{other}' (expected \
                 'describe_auto_scaling_groups', 'set_desired_capacity', \
                 or 'update_auto_scaling_group')"
            ))),
        }
    }

    #[instrument(skip(self), fields(provider = "aws-autoscaling"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing Auto Scaling health check");
        self.client
            .describe_auto_scaling_groups()
            .max_records(1)
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "Auto Scaling health check failed");
                ProviderError::Connection(format!("Auto Scaling health check failed: {e}"))
            })?;
        info!("Auto Scaling health check passed");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Action implementations
// ---------------------------------------------------------------------------

impl AutoScalingProvider {
    async fn describe_groups(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Auto Scaling describe_auto_scaling_groups payload");
        let payload: AsgDescribePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        debug!(group_names = ?payload.auto_scaling_group_names, "describing Auto Scaling Groups");

        let mut request = self.client.describe_auto_scaling_groups();
        if !payload.auto_scaling_group_names.is_empty() {
            request = request
                .set_auto_scaling_group_names(Some(payload.auto_scaling_group_names.clone()));
        }

        let result = request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "describe_auto_scaling_groups failed");
            let aws_err: ProviderError = classify_sdk_error(&err_str).into();
            aws_err
        })?;

        let groups: Vec<_> = result
            .auto_scaling_groups()
            .iter()
            .map(|g| {
                serde_json::json!({
                    "auto_scaling_group_name": g.auto_scaling_group_name(),
                    "min_size": g.min_size(),
                    "max_size": g.max_size(),
                    "desired_capacity": g.desired_capacity(),
                    "instance_count": g.instances().len(),
                    "health_check_type": g.health_check_type().unwrap_or_default(),
                })
            })
            .collect();

        info!(count = groups.len(), "Auto Scaling Groups described");

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "describe_auto_scaling_groups",
            "auto_scaling_groups": groups,
        })))
    }

    async fn set_desired_capacity(
        &self,
        action: &Action,
    ) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Auto Scaling set_desired_capacity payload");
        let payload: AsgSetCapacityPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        debug!(
            group = %payload.auto_scaling_group_name,
            desired_capacity = payload.desired_capacity,
            honor_cooldown = payload.honor_cooldown,
            "setting desired capacity"
        );

        self.client
            .set_desired_capacity()
            .auto_scaling_group_name(&payload.auto_scaling_group_name)
            .desired_capacity(payload.desired_capacity)
            .honor_cooldown(payload.honor_cooldown)
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "set_desired_capacity failed");
                let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                aws_err
            })?;

        info!(
            group = %payload.auto_scaling_group_name,
            desired_capacity = payload.desired_capacity,
            "desired capacity set"
        );

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "set_desired_capacity",
            "auto_scaling_group_name": payload.auto_scaling_group_name,
            "desired_capacity": payload.desired_capacity,
            "status": "updated",
        })))
    }

    async fn update_group(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing Auto Scaling update_auto_scaling_group payload");
        let payload: AsgUpdatePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        debug!(group = %payload.auto_scaling_group_name, "updating Auto Scaling Group");

        let mut request = self
            .client
            .update_auto_scaling_group()
            .auto_scaling_group_name(&payload.auto_scaling_group_name);

        if let Some(min) = payload.min_size {
            request = request.min_size(min);
        }
        if let Some(max) = payload.max_size {
            request = request.max_size(max);
        }
        if let Some(desired) = payload.desired_capacity {
            request = request.desired_capacity(desired);
        }
        if let Some(cooldown) = payload.default_cooldown {
            request = request.default_cooldown(cooldown);
        }
        if let Some(ref hct) = payload.health_check_type {
            request = request.health_check_type(hct);
        }
        if let Some(grace) = payload.health_check_grace_period {
            request = request.health_check_grace_period(grace);
        }

        request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "update_auto_scaling_group failed");
            let aws_err: ProviderError = classify_sdk_error(&err_str).into();
            aws_err
        })?;

        info!(group = %payload.auto_scaling_group_name, "Auto Scaling Group updated");

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "update_auto_scaling_group",
            "auto_scaling_group_name": payload.auto_scaling_group_name,
            "status": "updated",
        })))
    }
}

// ---------------------------------------------------------------------------
// Resource lookup
// ---------------------------------------------------------------------------

/// Payload for `ResourceLookup` `auto_scaling_group` lookups.
#[derive(Debug, Deserialize)]
struct AsgLookupParams {
    /// Auto Scaling Group names to look up.
    #[serde(default)]
    auto_scaling_group_names: Vec<String>,
}

#[async_trait]
impl ResourceLookup for AutoScalingProvider {
    async fn lookup(
        &self,
        resource_type: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match resource_type {
            "auto_scaling_group" => {
                let lookup_params: AsgLookupParams = serde_json::from_value(params.clone())
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;

                debug!(
                    group_names = ?lookup_params.auto_scaling_group_names,
                    "resource lookup: describing Auto Scaling Groups"
                );

                let mut request = self.client.describe_auto_scaling_groups();
                if !lookup_params.auto_scaling_group_names.is_empty() {
                    request = request.set_auto_scaling_group_names(Some(
                        lookup_params.auto_scaling_group_names.clone(),
                    ));
                }

                let result = request.send().await.map_err(|e| {
                    let err_str = e.to_string();
                    error!(error = %err_str, "resource lookup: describe_auto_scaling_groups failed");
                    let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                    aws_err
                })?;

                let groups: Vec<_> = result
                    .auto_scaling_groups()
                    .iter()
                    .map(|g| {
                        serde_json::json!({
                            "auto_scaling_group_name": g.auto_scaling_group_name(),
                            "min_size": g.min_size(),
                            "max_size": g.max_size(),
                            "desired_capacity": g.desired_capacity(),
                            "instance_count": g.instances().len(),
                            "health_check_type": g.health_check_type().unwrap_or_default(),
                        })
                    })
                    .collect();

                Ok(serde_json::json!({
                    "auto_scaling_groups": groups,
                }))
            }
            other => Err(ProviderError::Configuration(format!(
                "unsupported resource type '{other}' for Auto Scaling provider \
                 (supported: 'auto_scaling_group')"
            ))),
        }
    }

    fn supported_resource_types(&self) -> Vec<String> {
        vec!["auto_scaling_group".to_owned()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_region() {
        let config = AutoScalingConfig::new("us-west-2");
        assert_eq!(config.aws.region, "us-west-2");
    }

    #[test]
    fn config_builder_chain() {
        let config = AutoScalingConfig::new("eu-west-1")
            .with_endpoint_url("http://localhost:4566")
            .with_role_arn("arn:aws:iam::123:role/asg-access")
            .with_session_name("test-session")
            .with_external_id("ext-789");

        assert_eq!(
            config.aws.endpoint_url.as_deref(),
            Some("http://localhost:4566")
        );
        assert!(config.aws.role_arn.is_some());
        assert_eq!(config.aws.session_name.as_deref(), Some("test-session"));
        assert_eq!(config.aws.external_id.as_deref(), Some("ext-789"));
    }

    #[test]
    fn config_debug_format() {
        let config =
            AutoScalingConfig::new("us-east-1").with_role_arn("arn:aws:iam::123:role/test");
        let debug = format!("{config:?}");
        assert!(debug.contains("AutoScalingConfig"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config =
            AutoScalingConfig::new("ap-southeast-1").with_endpoint_url("http://localhost:4566");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AutoScalingConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.aws.region, "ap-southeast-1");
        assert_eq!(
            deserialized.aws.endpoint_url.as_deref(),
            Some("http://localhost:4566")
        );
    }

    #[test]
    fn deserialize_describe_payload() {
        let json = serde_json::json!({
            "auto_scaling_group_names": ["my-asg-1", "my-asg-2"]
        });
        let payload: AsgDescribePayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.auto_scaling_group_names.len(), 2);
    }

    #[test]
    fn deserialize_describe_payload_empty() {
        let json = serde_json::json!({});
        let payload: AsgDescribePayload = serde_json::from_value(json).unwrap();
        assert!(payload.auto_scaling_group_names.is_empty());
    }

    #[test]
    fn deserialize_set_capacity_payload() {
        let json = serde_json::json!({
            "auto_scaling_group_name": "my-asg",
            "desired_capacity": 5,
            "honor_cooldown": true
        });
        let payload: AsgSetCapacityPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.auto_scaling_group_name, "my-asg");
        assert_eq!(payload.desired_capacity, 5);
        assert!(payload.honor_cooldown);
    }

    #[test]
    fn deserialize_set_capacity_defaults() {
        let json = serde_json::json!({
            "auto_scaling_group_name": "my-asg",
            "desired_capacity": 3
        });
        let payload: AsgSetCapacityPayload = serde_json::from_value(json).unwrap();
        assert!(!payload.honor_cooldown);
    }

    #[test]
    fn deserialize_update_payload() {
        let json = serde_json::json!({
            "auto_scaling_group_name": "my-asg",
            "min_size": 2,
            "max_size": 10,
            "desired_capacity": 5,
            "default_cooldown": 300,
            "health_check_type": "ELB",
            "health_check_grace_period": 120
        });
        let payload: AsgUpdatePayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.auto_scaling_group_name, "my-asg");
        assert_eq!(payload.min_size, Some(2));
        assert_eq!(payload.max_size, Some(10));
        assert_eq!(payload.desired_capacity, Some(5));
        assert_eq!(payload.default_cooldown, Some(300));
        assert_eq!(payload.health_check_type.as_deref(), Some("ELB"));
        assert_eq!(payload.health_check_grace_period, Some(120));
    }

    #[test]
    fn deserialize_update_payload_minimal() {
        let json = serde_json::json!({
            "auto_scaling_group_name": "my-asg"
        });
        let payload: AsgUpdatePayload = serde_json::from_value(json).unwrap();
        assert!(payload.min_size.is_none());
        assert!(payload.max_size.is_none());
        assert!(payload.desired_capacity.is_none());
    }
}
