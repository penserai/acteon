use std::collections::HashMap;

use serde::Deserialize;

/// Configuration for a single provider instance.
///
/// # Example
///
/// ```toml
/// [[providers]]
/// name = "email"
/// type = "webhook"
/// url = "http://localhost:9999/webhook"
///
/// [[providers]]
/// name = "slack"
/// type = "log"
/// ```
#[derive(Debug, Deserialize)]
pub struct ProviderConfig {
    /// Unique name for this provider.
    pub name: String,
    /// Provider type: `"webhook"`, `"log"`, `"twilio"`, `"teams"`, `"discord"`,
    /// `"email"`, `"aws-sns"`, `"aws-lambda"`, `"aws-eventbridge"`, `"aws-sqs"`,
    /// `"aws-s3"`, `"aws-ec2"`, `"aws-autoscaling"`, `"azure-blob"`,
    /// or `"azure-eventhubs"`.
    #[serde(rename = "type")]
    pub provider_type: String,
    /// Target URL (required for `"webhook"` type).
    pub url: Option<String>,
    /// Additional HTTP headers (used by `"webhook"` type).
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Twilio Account SID (required for `"twilio"` type).
    pub account_sid: Option<String>,
    /// Twilio Auth Token (required for `"twilio"` type). Supports `ENC[...]`.
    pub auth_token: Option<String>,
    /// Default sender phone number (used by `"twilio"` type).
    pub from_number: Option<String>,
    /// Webhook URL (used by `"teams"` and `"discord"` types).
    pub webhook_url: Option<String>,
    /// Generic token field (future use).
    pub token: Option<String>,
    /// Default channel or recipient.
    pub default_channel: Option<String>,
    /// SMTP username for email authentication.
    pub username: Option<String>,
    /// SMTP authentication credential. Supports `ENC[...]`.
    #[serde(alias = "smtp_password")]
    pub password: Option<String>,

    // ---- Email provider fields ----
    /// Email backend: `"smtp"` (default) or `"ses"`.
    #[serde(default)]
    pub email_backend: Option<String>,
    /// SMTP server hostname (used by `"email"` type with SMTP backend).
    pub smtp_host: Option<String>,
    /// SMTP server port (used by `"email"` type with SMTP backend).
    pub smtp_port: Option<u16>,
    /// Sender email address (used by `"email"` type).
    pub from_address: Option<String>,
    /// Whether to use TLS for SMTP.
    #[serde(default)]
    pub tls: Option<bool>,

    // ---- AWS provider fields ----
    /// AWS region (used by all `"aws-*"` types and `"email"` with SES backend).
    pub aws_region: Option<String>,
    /// AWS IAM role ARN for STS assume-role (used by `"aws-*"` types).
    pub aws_role_arn: Option<String>,
    /// AWS endpoint URL override for `LocalStack` (used by `"aws-*"` types).
    pub aws_endpoint_url: Option<String>,
    /// SNS topic ARN (used by `"aws-sns"` type).
    pub topic_arn: Option<String>,
    /// Lambda function name or ARN (used by `"aws-lambda"` type).
    pub function_name: Option<String>,
    /// Lambda function qualifier (used by `"aws-lambda"` type).
    pub qualifier: Option<String>,
    /// `EventBridge` event bus name (used by `"aws-eventbridge"` type).
    pub event_bus_name: Option<String>,
    /// SQS queue URL (used by `"aws-sqs"` type).
    pub queue_url: Option<String>,
    /// SES configuration set name (used by `"email"` with SES backend).
    pub ses_configuration_set: Option<String>,
    /// STS session name for assume-role (used by `"aws-*"` types).
    pub aws_session_name: Option<String>,
    /// STS external ID for cross-account trust policies (used by `"aws-*"` types).
    pub aws_external_id: Option<String>,
    /// S3 bucket name (used by `"aws-s3"` type).
    pub bucket_name: Option<String>,
    /// S3 object key prefix (used by `"aws-s3"` type).
    pub object_prefix: Option<String>,
    /// Default security group IDs (used by `"aws-ec2"` type).
    #[serde(default)]
    pub default_security_group_ids: Option<Vec<String>>,
    /// Default subnet ID (used by `"aws-ec2"` type).
    pub default_subnet_id: Option<String>,
    /// Default key-pair name (used by `"aws-ec2"` type).
    pub default_key_name: Option<String>,

    // ---- Azure provider fields ----
    /// Azure AD tenant ID (used by `"azure-*"` types).
    pub azure_tenant_id: Option<String>,
    /// Azure AD client ID (used by `"azure-*"` types).
    pub azure_client_id: Option<String>,
    /// Azure AD client credential (used by `"azure-*"` types). Supports `ENC[...]`.
    pub azure_client_credential: Option<String>,
    /// Azure subscription ID (used by `"azure-*"` types).
    pub azure_subscription_id: Option<String>,
    /// Azure resource group (used by `"azure-*"` types).
    pub azure_resource_group: Option<String>,
    /// Azure region/location (used by `"azure-*"` types).
    pub azure_location: Option<String>,
    /// Azure endpoint URL override for `Azurite` (used by `"azure-*"` types).
    pub azure_endpoint_url: Option<String>,
    /// Azure Storage account name (used by `"azure-blob"` type).
    pub azure_account_name: Option<String>,
    /// Default Azure container name (used by `"azure-blob"` type).
    pub azure_container_name: Option<String>,
    /// Azure blob name prefix (used by `"azure-blob"` type).
    pub azure_blob_prefix: Option<String>,
    /// Azure Event Hubs namespace (used by `"azure-eventhubs"` type).
    pub azure_namespace: Option<String>,
    /// Azure Event Hub name (used by `"azure-eventhubs"` type).
    pub azure_event_hub_name: Option<String>,
}
