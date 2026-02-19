use std::collections::HashMap;

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

/// Configuration for the AWS EC2 provider.
#[derive(Clone, Serialize, Deserialize)]
pub struct Ec2Config {
    /// Shared AWS configuration (region, role ARN, endpoint URL).
    #[serde(flatten)]
    pub aws: AwsBaseConfig,

    /// Default security group IDs applied to `run_instances` when not overridden in the payload.
    #[serde(default)]
    pub default_security_group_ids: Option<Vec<String>>,

    /// Default subnet ID applied to `run_instances` when not overridden in the payload.
    #[serde(default)]
    pub default_subnet_id: Option<String>,

    /// Default key-pair name applied to `run_instances` when not overridden in the payload.
    #[serde(default)]
    pub default_key_name: Option<String>,
}

impl std::fmt::Debug for Ec2Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ec2Config")
            .field("aws", &self.aws)
            .field(
                "default_security_group_ids",
                &self.default_security_group_ids,
            )
            .field("default_subnet_id", &self.default_subnet_id)
            .field("default_key_name", &self.default_key_name)
            .finish()
    }
}

impl Ec2Config {
    /// Create a new `Ec2Config` with the given AWS region.
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            aws: AwsBaseConfig::new(region),
            default_security_group_ids: None,
            default_subnet_id: None,
            default_key_name: None,
        }
    }

    /// Set default security group IDs for `run_instances`.
    #[must_use]
    pub fn with_default_security_group_ids(mut self, ids: Vec<String>) -> Self {
        self.default_security_group_ids = Some(ids);
        self
    }

    /// Set the default subnet ID for `run_instances`.
    #[must_use]
    pub fn with_default_subnet_id(mut self, subnet_id: impl Into<String>) -> Self {
        self.default_subnet_id = Some(subnet_id.into());
        self
    }

    /// Set the default key-pair name for `run_instances`.
    #[must_use]
    pub fn with_default_key_name(mut self, key_name: impl Into<String>) -> Self {
        self.default_key_name = Some(key_name.into());
        self
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

/// Payload for the `start_instances` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ec2StartPayload {
    /// EC2 instance IDs to start.
    pub instance_ids: Vec<String>,
}

/// Payload for the `stop_instances` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ec2StopPayload {
    /// EC2 instance IDs to stop.
    pub instance_ids: Vec<String>,

    /// If `true`, hibernate the instances instead of stopping them.
    #[serde(default)]
    pub hibernate: bool,

    /// If `true`, force the instances to stop without a graceful shutdown.
    #[serde(default)]
    pub force: bool,
}

/// Payload for the `reboot_instances` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ec2RebootPayload {
    /// EC2 instance IDs to reboot.
    pub instance_ids: Vec<String>,
}

/// Payload for the `terminate_instances` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ec2TerminatePayload {
    /// EC2 instance IDs to terminate.
    pub instance_ids: Vec<String>,
}

/// Payload for the `run_instances` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ec2RunInstancesPayload {
    /// AMI ID to launch.
    pub image_id: String,

    /// Instance type (e.g. `"t3.micro"`).
    pub instance_type: String,

    /// Minimum number of instances to launch (defaults to 1).
    #[serde(default = "default_one")]
    pub min_count: i32,

    /// Maximum number of instances to launch (defaults to 1).
    #[serde(default = "default_one")]
    pub max_count: i32,

    /// Key-pair name. Overrides config `default_key_name`.
    #[serde(default)]
    pub key_name: Option<String>,

    /// Security group IDs. Overrides config `default_security_group_ids`.
    #[serde(default)]
    pub security_group_ids: Option<Vec<String>>,

    /// Subnet ID. Overrides config `default_subnet_id`.
    #[serde(default)]
    pub subnet_id: Option<String>,

    /// Base64-encoded user data script.
    #[serde(default)]
    pub user_data: Option<String>,

    /// Tags to apply to the launched instances.
    #[serde(default)]
    pub tags: HashMap<String, String>,

    /// IAM instance profile name or ARN.
    #[serde(default)]
    pub iam_instance_profile: Option<String>,
}

fn default_one() -> i32 {
    1
}

/// Payload for the `attach_volume` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ec2AttachVolumePayload {
    /// EBS volume ID.
    pub volume_id: String,

    /// EC2 instance ID.
    pub instance_id: String,

    /// Device name (e.g. `"/dev/sdf"`).
    pub device: String,
}

/// Payload for the `detach_volume` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ec2DetachVolumePayload {
    /// EBS volume ID.
    pub volume_id: String,

    /// Optional instance ID to detach from.
    #[serde(default)]
    pub instance_id: Option<String>,

    /// Device name (optional).
    #[serde(default)]
    pub device: Option<String>,

    /// Force detachment.
    #[serde(default)]
    pub force: bool,
}

/// Payload for the `describe_instances` action type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ec2DescribePayload {
    /// Optional instance IDs to describe. If empty, describes all instances.
    #[serde(default)]
    pub instance_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// AWS EC2 provider for instance lifecycle management.
pub struct Ec2Provider {
    config: Ec2Config,
    client: aws_sdk_ec2::Client,
}

impl std::fmt::Debug for Ec2Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ec2Provider")
            .field("config", &self.config)
            .field("client", &"<Ec2Client>")
            .finish()
    }
}

impl Ec2Provider {
    /// Create a new `Ec2Provider` by building an AWS SDK client.
    pub async fn new(config: Ec2Config) -> Self {
        let sdk_config = build_sdk_config(&config.aws).await;
        let client = aws_sdk_ec2::Client::new(&sdk_config);
        Self { config, client }
    }

    /// Create an `Ec2Provider` with a pre-built client (for testing).
    pub fn with_client(config: Ec2Config, client: aws_sdk_ec2::Client) -> Self {
        Self { config, client }
    }

    /// Resolve key name from payload or config default.
    fn resolve_key_name<'a>(&'a self, payload_key_name: Option<&'a str>) -> Option<&'a str> {
        payload_key_name.or(self.config.default_key_name.as_deref())
    }

    /// Resolve subnet ID from payload or config default.
    fn resolve_subnet_id<'a>(&'a self, payload_subnet_id: Option<&'a str>) -> Option<&'a str> {
        payload_subnet_id.or(self.config.default_subnet_id.as_deref())
    }

    /// Resolve security group IDs from payload or config default.
    fn resolve_security_group_ids<'a>(
        &'a self,
        payload_sg_ids: Option<&'a [String]>,
    ) -> Option<&'a [String]> {
        payload_sg_ids.or(self.config.default_security_group_ids.as_deref())
    }
}

impl Provider for Ec2Provider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "aws-ec2"
    }

    #[instrument(skip(self, action), fields(action_id = %action.id, provider = "aws-ec2"))]
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        match action.action_type.as_str() {
            "start_instances" => self.start_instances(action).await,
            "stop_instances" => self.stop_instances(action).await,
            "reboot_instances" => self.reboot_instances(action).await,
            "terminate_instances" => self.terminate_instances(action).await,
            "hibernate_instances" => self.hibernate_instances(action).await,
            "run_instances" => self.run_instances(action).await,
            "attach_volume" => self.attach_volume(action).await,
            "detach_volume" => self.detach_volume(action).await,
            "describe_instances" => self.describe_instances(action).await,
            other => Err(ProviderError::Configuration(format!(
                "unknown EC2 action type '{other}' (expected 'start_instances', 'stop_instances', \
                 'reboot_instances', 'terminate_instances', 'hibernate_instances', 'run_instances', \
                 'attach_volume', 'detach_volume', or 'describe_instances')"
            ))),
        }
    }

    #[instrument(skip(self), fields(provider = "aws-ec2"))]
    async fn health_check(&self) -> Result<(), ProviderError> {
        debug!("performing EC2 health check via dry-run describe_instances");
        let result = self.client.describe_instances().dry_run(true).send().await;

        match result {
            // A successful response means the API is reachable.
            Ok(_) => {
                info!("EC2 health check passed");
                Ok(())
            }
            Err(e) => {
                let err_str = e.to_string();
                // DryRunOperation means the API call would have succeeded — healthy.
                if err_str.contains("DryRunOperation") {
                    info!("EC2 health check passed (dry-run)");
                    Ok(())
                } else {
                    error!(error = %err_str, "EC2 health check failed");
                    Err(ProviderError::Connection(format!(
                        "EC2 health check failed: {err_str}"
                    )))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Action implementations
// ---------------------------------------------------------------------------

impl Ec2Provider {
    async fn start_instances(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing EC2 start_instances payload");
        let payload: Ec2StartPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        if payload.instance_ids.is_empty() {
            return Err(ProviderError::Configuration(
                "instance_ids must not be empty".to_owned(),
            ));
        }

        debug!(instance_ids = ?payload.instance_ids, "starting EC2 instances");

        let result = self
            .client
            .start_instances()
            .set_instance_ids(Some(payload.instance_ids.clone()))
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "EC2 start_instances failed");
                let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                aws_err
            })?;

        let states: Vec<_> = result
            .starting_instances()
            .iter()
            .map(|s| {
                serde_json::json!({
                    "instance_id": s.instance_id().unwrap_or_default(),
                    "previous_state": s.previous_state().and_then(|st| st.name()).map_or("unknown", aws_sdk_ec2::types::InstanceStateName::as_str),
                    "current_state": s.current_state().and_then(|st| st.name()).map_or("unknown", aws_sdk_ec2::types::InstanceStateName::as_str),
                })
            })
            .collect();

        info!(count = states.len(), "EC2 instances starting");

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "start_instances",
            "instance_state_changes": states,
        })))
    }

    async fn stop_instances(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing EC2 stop_instances payload");
        let payload: Ec2StopPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        if payload.instance_ids.is_empty() {
            return Err(ProviderError::Configuration(
                "instance_ids must not be empty".to_owned(),
            ));
        }

        debug!(instance_ids = ?payload.instance_ids, hibernate = payload.hibernate, force = payload.force, "stopping EC2 instances");

        let result = self
            .client
            .stop_instances()
            .set_instance_ids(Some(payload.instance_ids.clone()))
            .hibernate(payload.hibernate)
            .force(payload.force)
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "EC2 stop_instances failed");
                let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                aws_err
            })?;

        let states: Vec<_> = result
            .stopping_instances()
            .iter()
            .map(|s| {
                serde_json::json!({
                    "instance_id": s.instance_id().unwrap_or_default(),
                    "previous_state": s.previous_state().and_then(|st| st.name()).map_or("unknown", aws_sdk_ec2::types::InstanceStateName::as_str),
                    "current_state": s.current_state().and_then(|st| st.name()).map_or("unknown", aws_sdk_ec2::types::InstanceStateName::as_str),
                })
            })
            .collect();

        info!(
            count = states.len(),
            hibernate = payload.hibernate,
            "EC2 instances stopping"
        );

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "stop_instances",
            "hibernate": payload.hibernate,
            "instance_state_changes": states,
        })))
    }

    async fn hibernate_instances(
        &self,
        action: &Action,
    ) -> Result<ProviderResponse, ProviderError> {
        debug!("hibernate_instances is sugar for stop_instances with hibernate=true");
        // Parse just the instance_ids and re-dispatch as stop with hibernate=true.
        let payload: Ec2StartPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let stop_payload = serde_json::json!({
            "instance_ids": payload.instance_ids,
            "hibernate": true,
        });

        let mut stop_action = action.clone();
        stop_action.payload = stop_payload;
        "stop_instances".clone_into(&mut stop_action.action_type);

        self.stop_instances(&stop_action).await
    }

    async fn reboot_instances(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing EC2 reboot_instances payload");
        let payload: Ec2RebootPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        if payload.instance_ids.is_empty() {
            return Err(ProviderError::Configuration(
                "instance_ids must not be empty".to_owned(),
            ));
        }

        debug!(instance_ids = ?payload.instance_ids, "rebooting EC2 instances");

        self.client
            .reboot_instances()
            .set_instance_ids(Some(payload.instance_ids.clone()))
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "EC2 reboot_instances failed");
                let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                aws_err
            })?;

        info!(
            count = payload.instance_ids.len(),
            "EC2 instances rebooting"
        );

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "reboot_instances",
            "instance_ids": payload.instance_ids,
            "status": "rebooting",
        })))
    }

    async fn terminate_instances(
        &self,
        action: &Action,
    ) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing EC2 terminate_instances payload");
        let payload: Ec2TerminatePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        if payload.instance_ids.is_empty() {
            return Err(ProviderError::Configuration(
                "instance_ids must not be empty".to_owned(),
            ));
        }

        debug!(instance_ids = ?payload.instance_ids, "terminating EC2 instances");

        let result = self
            .client
            .terminate_instances()
            .set_instance_ids(Some(payload.instance_ids.clone()))
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "EC2 terminate_instances failed");
                let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                aws_err
            })?;

        let states: Vec<_> = result
            .terminating_instances()
            .iter()
            .map(|s| {
                serde_json::json!({
                    "instance_id": s.instance_id().unwrap_or_default(),
                    "previous_state": s.previous_state().and_then(|st| st.name()).map_or("unknown", aws_sdk_ec2::types::InstanceStateName::as_str),
                    "current_state": s.current_state().and_then(|st| st.name()).map_or("unknown", aws_sdk_ec2::types::InstanceStateName::as_str),
                })
            })
            .collect();

        info!(count = states.len(), "EC2 instances terminating");

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "terminate_instances",
            "instance_state_changes": states,
        })))
    }

    async fn run_instances(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing EC2 run_instances payload");
        let payload: Ec2RunInstancesPayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        let key_name = self.resolve_key_name(payload.key_name.as_deref());
        let subnet_id = self.resolve_subnet_id(payload.subnet_id.as_deref());
        let sg_ids = self.resolve_security_group_ids(payload.security_group_ids.as_deref());

        debug!(
            image_id = %payload.image_id,
            instance_type = %payload.instance_type,
            min_count = payload.min_count,
            max_count = payload.max_count,
            "launching EC2 instances"
        );

        let mut request = self
            .client
            .run_instances()
            .image_id(&payload.image_id)
            .instance_type(
                payload
                    .instance_type
                    .parse::<aws_sdk_ec2::types::InstanceType>()
                    .map_err(|_| {
                        ProviderError::Configuration(format!(
                            "invalid instance type '{}'",
                            payload.instance_type
                        ))
                    })?,
            )
            .min_count(payload.min_count)
            .max_count(payload.max_count);

        if let Some(kn) = key_name {
            request = request.key_name(kn);
        }

        if let Some(sid) = subnet_id {
            request = request.subnet_id(sid);
        }

        if let Some(ids) = sg_ids {
            for id in ids {
                request = request.security_group_ids(id);
            }
        }

        if let Some(ref ud) = payload.user_data {
            request = request.user_data(ud);
        }

        if let Some(ref profile) = payload.iam_instance_profile {
            let iam_spec = if profile.starts_with("arn:") {
                aws_sdk_ec2::types::IamInstanceProfileSpecification::builder()
                    .arn(profile)
                    .build()
            } else {
                aws_sdk_ec2::types::IamInstanceProfileSpecification::builder()
                    .name(profile)
                    .build()
            };
            request = request.iam_instance_profile(iam_spec);
        }

        if !payload.tags.is_empty() {
            let tags: Vec<aws_sdk_ec2::types::Tag> = payload
                .tags
                .iter()
                .map(|(k, v)| aws_sdk_ec2::types::Tag::builder().key(k).value(v).build())
                .collect();
            let tag_spec = aws_sdk_ec2::types::TagSpecification::builder()
                .resource_type(aws_sdk_ec2::types::ResourceType::Instance)
                .set_tags(Some(tags))
                .build();
            request = request.tag_specifications(tag_spec);
        }

        let result = request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "EC2 run_instances failed");
            let aws_err: ProviderError = classify_sdk_error(&err_str).into();
            aws_err
        })?;

        let instances: Vec<_> = result
            .instances()
            .iter()
            .map(|i| {
                serde_json::json!({
                    "instance_id": i.instance_id().unwrap_or_default(),
                    "instance_type": i.instance_type().map_or("unknown", aws_sdk_ec2::types::InstanceType::as_str),
                    "state": i.state().and_then(|s| s.name()).map_or("unknown", aws_sdk_ec2::types::InstanceStateName::as_str),
                })
            })
            .collect();

        info!(count = instances.len(), "EC2 instances launched");

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "run_instances",
            "reservation_id": result.reservation_id().unwrap_or_default(),
            "instances": instances,
        })))
    }

    async fn attach_volume(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing EC2 attach_volume payload");
        let payload: Ec2AttachVolumePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        debug!(
            volume_id = %payload.volume_id,
            instance_id = %payload.instance_id,
            device = %payload.device,
            "attaching EBS volume"
        );

        let result = self
            .client
            .attach_volume()
            .volume_id(&payload.volume_id)
            .instance_id(&payload.instance_id)
            .device(&payload.device)
            .send()
            .await
            .map_err(|e| {
                let err_str = e.to_string();
                error!(error = %err_str, "EC2 attach_volume failed");
                let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                aws_err
            })?;

        info!(
            volume_id = %payload.volume_id,
            instance_id = %payload.instance_id,
            "EBS volume attached"
        );

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "attach_volume",
            "volume_id": result.volume_id().unwrap_or_default(),
            "instance_id": result.instance_id().unwrap_or_default(),
            "device": result.device().unwrap_or_default(),
            "state": result.state().map_or("unknown", aws_sdk_ec2::types::VolumeAttachmentState::as_str),
        })))
    }

    async fn detach_volume(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing EC2 detach_volume payload");
        let payload: Ec2DetachVolumePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        debug!(volume_id = %payload.volume_id, force = payload.force, "detaching EBS volume");

        let mut request = self.client.detach_volume().volume_id(&payload.volume_id);

        if let Some(ref iid) = payload.instance_id {
            request = request.instance_id(iid);
        }
        if let Some(ref dev) = payload.device {
            request = request.device(dev);
        }
        if payload.force {
            request = request.force(true);
        }

        let result = request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "EC2 detach_volume failed");
            let aws_err: ProviderError = classify_sdk_error(&err_str).into();
            aws_err
        })?;

        info!(volume_id = %payload.volume_id, "EBS volume detached");

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "detach_volume",
            "volume_id": result.volume_id().unwrap_or_default(),
            "instance_id": result.instance_id().unwrap_or_default(),
            "state": result.state().map_or("unknown", aws_sdk_ec2::types::VolumeAttachmentState::as_str),
        })))
    }

    async fn describe_instances(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        debug!("deserializing EC2 describe_instances payload");
        let payload: Ec2DescribePayload = serde_json::from_value(action.payload.clone())
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;

        debug!(instance_ids = ?payload.instance_ids, "describing EC2 instances");

        let mut request = self.client.describe_instances();
        if !payload.instance_ids.is_empty() {
            request = request.set_instance_ids(Some(payload.instance_ids.clone()));
        }

        let result = request.send().await.map_err(|e| {
            let err_str = e.to_string();
            error!(error = %err_str, "EC2 describe_instances failed");
            let aws_err: ProviderError = classify_sdk_error(&err_str).into();
            aws_err
        })?;

        let instances: Vec<_> = result
            .reservations()
            .iter()
            .flat_map(aws_sdk_ec2::types::Reservation::instances)
            .map(|i| {
                serde_json::json!({
                    "instance_id": i.instance_id().unwrap_or_default(),
                    "instance_type": i.instance_type().map_or("unknown", aws_sdk_ec2::types::InstanceType::as_str),
                    "state": i.state().and_then(|s| s.name()).map_or("unknown", aws_sdk_ec2::types::InstanceStateName::as_str),
                    "public_ip": i.public_ip_address().unwrap_or_default(),
                    "private_ip": i.private_ip_address().unwrap_or_default(),
                })
            })
            .collect();

        info!(count = instances.len(), "EC2 instances described");

        Ok(ProviderResponse::success(serde_json::json!({
            "action": "describe_instances",
            "instances": instances,
        })))
    }
}

// ---------------------------------------------------------------------------
// Resource lookup
// ---------------------------------------------------------------------------

/// Payload for `ResourceLookup` `instance` lookups.
#[derive(Debug, Deserialize)]
struct Ec2LookupParams {
    /// EC2 instance IDs to look up.
    #[serde(default)]
    instance_ids: Vec<String>,
}

#[async_trait]
impl ResourceLookup for Ec2Provider {
    async fn lookup(
        &self,
        resource_type: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        match resource_type {
            "instance" => {
                let lookup_params: Ec2LookupParams = serde_json::from_value(params.clone())
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;

                debug!(
                    instance_ids = ?lookup_params.instance_ids,
                    "resource lookup: describing EC2 instances"
                );

                let mut request = self.client.describe_instances();
                if !lookup_params.instance_ids.is_empty() {
                    request = request.set_instance_ids(Some(lookup_params.instance_ids.clone()));
                }

                let result = request.send().await.map_err(|e| {
                    let err_str = e.to_string();
                    error!(error = %err_str, "resource lookup: describe_instances failed");
                    let aws_err: ProviderError = classify_sdk_error(&err_str).into();
                    aws_err
                })?;

                let instances: Vec<_> = result
                    .reservations()
                    .iter()
                    .flat_map(aws_sdk_ec2::types::Reservation::instances)
                    .map(|i| {
                        serde_json::json!({
                            "instance_id": i.instance_id().unwrap_or_default(),
                            "instance_type": i.instance_type().map_or(
                                "unknown",
                                aws_sdk_ec2::types::InstanceType::as_str,
                            ),
                            "state": i.state().and_then(|s| s.name()).map_or(
                                "unknown",
                                aws_sdk_ec2::types::InstanceStateName::as_str,
                            ),
                            "public_ip": i.public_ip_address().unwrap_or_default(),
                            "private_ip": i.private_ip_address().unwrap_or_default(),
                        })
                    })
                    .collect();

                Ok(serde_json::json!({
                    "instances": instances,
                }))
            }
            other => Err(ProviderError::Configuration(format!(
                "unsupported resource type '{other}' for EC2 provider \
                 (supported: 'instance')"
            ))),
        }
    }

    fn supported_resource_types(&self) -> Vec<String> {
        vec!["instance".to_owned()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_sets_region() {
        let config = Ec2Config::new("us-west-2");
        assert_eq!(config.aws.region, "us-west-2");
        assert!(config.default_security_group_ids.is_none());
        assert!(config.default_subnet_id.is_none());
        assert!(config.default_key_name.is_none());
    }

    #[test]
    fn config_builder_chain() {
        let config = Ec2Config::new("eu-west-1")
            .with_default_security_group_ids(vec!["sg-123".to_owned()])
            .with_default_subnet_id("subnet-abc")
            .with_default_key_name("my-key")
            .with_endpoint_url("http://localhost:4566")
            .with_role_arn("arn:aws:iam::123:role/ec2-access")
            .with_session_name("test-session")
            .with_external_id("ext-456");

        assert_eq!(
            config.default_security_group_ids.as_deref(),
            Some(vec!["sg-123".to_owned()].as_slice())
        );
        assert_eq!(config.default_subnet_id.as_deref(), Some("subnet-abc"));
        assert_eq!(config.default_key_name.as_deref(), Some("my-key"));
        assert_eq!(
            config.aws.endpoint_url.as_deref(),
            Some("http://localhost:4566")
        );
        assert!(config.aws.role_arn.is_some());
        assert_eq!(config.aws.session_name.as_deref(), Some("test-session"));
        assert_eq!(config.aws.external_id.as_deref(), Some("ext-456"));
    }

    #[test]
    fn config_debug_format() {
        let config = Ec2Config::new("us-east-1").with_role_arn("arn:aws:iam::123:role/test");
        let debug = format!("{config:?}");
        assert!(debug.contains("Ec2Config"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = Ec2Config::new("ap-southeast-1")
            .with_default_key_name("prod-key")
            .with_default_subnet_id("subnet-xyz");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: Ec2Config = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.aws.region, "ap-southeast-1");
        assert_eq!(deserialized.default_key_name.as_deref(), Some("prod-key"));
        assert_eq!(
            deserialized.default_subnet_id.as_deref(),
            Some("subnet-xyz")
        );
    }

    #[test]
    fn deserialize_start_payload() {
        let json = serde_json::json!({
            "instance_ids": ["i-abc123", "i-def456"]
        });
        let payload: Ec2StartPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.instance_ids.len(), 2);
        assert_eq!(payload.instance_ids[0], "i-abc123");
    }

    #[test]
    fn deserialize_stop_payload() {
        let json = serde_json::json!({
            "instance_ids": ["i-abc123"],
            "hibernate": true,
            "force": false
        });
        let payload: Ec2StopPayload = serde_json::from_value(json).unwrap();
        assert!(payload.hibernate);
        assert!(!payload.force);
    }

    #[test]
    fn deserialize_stop_payload_defaults() {
        let json = serde_json::json!({
            "instance_ids": ["i-abc123"]
        });
        let payload: Ec2StopPayload = serde_json::from_value(json).unwrap();
        assert!(!payload.hibernate);
        assert!(!payload.force);
    }

    #[test]
    fn deserialize_reboot_payload() {
        let json = serde_json::json!({
            "instance_ids": ["i-abc123"]
        });
        let payload: Ec2RebootPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.instance_ids, vec!["i-abc123"]);
    }

    #[test]
    fn deserialize_terminate_payload() {
        let json = serde_json::json!({
            "instance_ids": ["i-abc123", "i-def456"]
        });
        let payload: Ec2TerminatePayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.instance_ids.len(), 2);
    }

    #[test]
    fn deserialize_run_instances_payload() {
        let json = serde_json::json!({
            "image_id": "ami-12345678",
            "instance_type": "t3.micro",
            "min_count": 2,
            "max_count": 5,
            "key_name": "my-key",
            "security_group_ids": ["sg-111", "sg-222"],
            "subnet_id": "subnet-aaa",
            "user_data": "IyEvYmluL2Jhc2g=",
            "tags": {"env": "staging", "team": "platform"},
            "iam_instance_profile": "arn:aws:iam::123:instance-profile/app"
        });
        let payload: Ec2RunInstancesPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.image_id, "ami-12345678");
        assert_eq!(payload.instance_type, "t3.micro");
        assert_eq!(payload.min_count, 2);
        assert_eq!(payload.max_count, 5);
        assert_eq!(payload.key_name.as_deref(), Some("my-key"));
        assert_eq!(payload.security_group_ids.as_ref().unwrap().len(), 2);
        assert_eq!(payload.subnet_id.as_deref(), Some("subnet-aaa"));
        assert!(payload.user_data.is_some());
        assert_eq!(payload.tags.len(), 2);
        assert!(payload.iam_instance_profile.is_some());
    }

    #[test]
    fn deserialize_run_instances_minimal() {
        let json = serde_json::json!({
            "image_id": "ami-12345678",
            "instance_type": "t3.nano"
        });
        let payload: Ec2RunInstancesPayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.min_count, 1);
        assert_eq!(payload.max_count, 1);
        assert!(payload.key_name.is_none());
        assert!(payload.security_group_ids.is_none());
        assert!(payload.tags.is_empty());
    }

    #[test]
    fn deserialize_attach_volume_payload() {
        let json = serde_json::json!({
            "volume_id": "vol-abc123",
            "instance_id": "i-def456",
            "device": "/dev/sdf"
        });
        let payload: Ec2AttachVolumePayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.volume_id, "vol-abc123");
        assert_eq!(payload.instance_id, "i-def456");
        assert_eq!(payload.device, "/dev/sdf");
    }

    #[test]
    fn deserialize_detach_volume_payload() {
        let json = serde_json::json!({
            "volume_id": "vol-abc123",
            "force": true
        });
        let payload: Ec2DetachVolumePayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.volume_id, "vol-abc123");
        assert!(payload.instance_id.is_none());
        assert!(payload.force);
    }

    #[test]
    fn deserialize_describe_payload() {
        let json = serde_json::json!({
            "instance_ids": ["i-abc123"]
        });
        let payload: Ec2DescribePayload = serde_json::from_value(json).unwrap();
        assert_eq!(payload.instance_ids, vec!["i-abc123"]);
    }

    #[test]
    fn deserialize_describe_payload_empty() {
        let json = serde_json::json!({});
        let payload: Ec2DescribePayload = serde_json::from_value(json).unwrap();
        assert!(payload.instance_ids.is_empty());
    }
}
