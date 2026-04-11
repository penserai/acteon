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
    /// `"email"`, `"opsgenie"`, `"victorops"`, `"pushover"`, `"telegram"`,
    /// `"aws-sns"`, `"aws-lambda"`, `"aws-eventbridge"`, `"aws-sqs"`,
    /// `"aws-s3"`, `"aws-ec2"`, `"aws-autoscaling"`, `"azure-blob"`,
    /// `"azure-eventhubs"`, `"gcp-pubsub"`, or `"gcp-storage"`.
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

    // ---- GCP provider fields ----
    /// GCP project ID (used by all `"gcp-*"` types).
    pub gcp_project_id: Option<String>,
    /// Path to GCP service account JSON key file (used by `"gcp-*"` types).
    pub gcp_credentials_path: Option<String>,
    /// Inline GCP service account JSON key (used by `"gcp-*"` types). Supports `ENC[...]`.
    pub gcp_credentials_json: Option<String>,
    /// GCP endpoint URL override for emulators (used by `"gcp-*"` types).
    pub gcp_endpoint_url: Option<String>,
    /// Default `Pub/Sub` topic (used by `"gcp-pubsub"` type).
    pub gcp_topic: Option<String>,
    /// Default Cloud Storage bucket name (used by `"gcp-storage"` type).
    pub gcp_bucket: Option<String>,
    /// Cloud Storage object name prefix (used by `"gcp-storage"` type).
    pub gcp_object_prefix: Option<String>,

    // ---- Nested per-provider config sub-structs ----
    //
    // New providers should put their settings inside a nested
    // struct rather than adding more flat `foo_*` fields at the
    // top level — the top-level field count is already a
    // maintenance burden and will not scale to 30+ providers.
    // Existing flat provider fields are left in place so this
    // refactor does not break existing TOML configs; a follow-up
    // PR can migrate them.
    /// Nested configuration block for the `"opsgenie"` provider type.
    ///
    /// Example TOML:
    /// ```toml
    /// [[providers]]
    /// name = "opsgenie-prod"
    /// type = "opsgenie"
    /// opsgenie.api_key = "ENC[...]"
    /// opsgenie.region = "us"
    /// opsgenie.default_team = "platform-oncall"
    /// ```
    #[serde(default)]
    pub opsgenie: OpsGenieProviderConfig,

    /// Nested configuration block for the `"victorops"` provider type.
    ///
    /// Example TOML:
    /// ```toml
    /// [[providers]]
    /// name = "victorops-prod"
    /// type = "victorops"
    /// victorops.api_key = "ENC[...]"
    /// victorops.default_route = "team-ops"
    /// victorops.routes = { team-ops = "ENC[...]", team-infra = "ENC[...]" }
    /// ```
    #[serde(default)]
    pub victorops: VictorOpsProviderConfig,

    /// Nested configuration block for the `"pushover"` provider type.
    ///
    /// Example TOML:
    /// ```toml
    /// [[providers]]
    /// name = "pushover-ops"
    /// type = "pushover"
    /// pushover.app_token = "ENC[...]"
    /// pushover.default_recipient = "ops-oncall"
    /// pushover.recipients = { ops-oncall = "ENC[...]", dev = "ENC[...]" }
    /// ```
    #[serde(default)]
    pub pushover: PushoverProviderConfig,

    /// Nested configuration block for the `"telegram"` provider type.
    ///
    /// Example TOML:
    /// ```toml
    /// [[providers]]
    /// name = "telegram-ops"
    /// type = "telegram"
    /// telegram.bot_token = "ENC[...]"
    /// telegram.default_chat = "ops-channel"
    /// telegram.default_parse_mode = "HTML"
    /// telegram.chats = { ops-channel = "-1001234567890", devs = "@devchannel" }
    /// ```
    #[serde(default)]
    pub telegram: TelegramProviderConfig,
}

/// Nested configuration block for the `OpsGenie` provider.
///
/// All fields are optional at the TOML layer. The provider's own
/// validation (in `main.rs`) rejects configurations that omit
/// required fields like `api_key`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct OpsGenieProviderConfig {
    /// `OpsGenie` API integration key. Supports `ENC[...]`.
    pub api_key: Option<String>,
    /// `OpsGenie` region: `"us"` (default) or `"eu"`.
    pub region: Option<String>,
    /// Default team responder used when a payload omits one.
    pub default_team: Option<String>,
    /// Default alert priority (`P1`..=`P5`).
    pub default_priority: Option<String>,
    /// Default alert source label.
    pub default_source: Option<String>,
    /// Override base URL for the `OpsGenie` API (testing only).
    pub api_base_url: Option<String>,
    /// Whether to automatically prefix user-supplied aliases with
    /// `{namespace}:{tenant}:` before sending them to `OpsGenie`.
    /// Defaults to `true` — leave it on unless each Acteon
    /// namespace/tenant has its own dedicated `OpsGenie` integration
    /// key.
    pub scope_aliases: Option<bool>,
    /// Maximum length (in bytes) for the `message` field before
    /// client-side truncation. Defaults to 130 (the current
    /// `OpsGenie` API cap).
    pub message_max_length: Option<usize>,
}

/// Nested configuration block for the `VictorOps` (Splunk On-Call) provider.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct VictorOpsProviderConfig {
    /// Organization-level REST integration key. Supports `ENC[...]`.
    pub api_key: Option<String>,
    /// Map of logical route name → per-route routing key. Values
    /// support `ENC[...]`.
    pub routes: HashMap<String, String>,
    /// Name of the default route used when the payload omits
    /// `routing_key`.
    pub default_route: Option<String>,
    /// Override base URL for the `VictorOps` REST endpoint
    /// integration (testing only).
    pub api_base_url: Option<String>,
    /// Value reported in the alert body's `monitoring_tool` field.
    /// Defaults to `"acteon"` at the provider layer.
    pub monitoring_tool: Option<String>,
    /// Whether to auto-prefix `entity_id` with
    /// `{namespace}:{tenant}:` for multi-tenant isolation. Defaults
    /// to `true`.
    pub scope_entity_ids: Option<bool>,
}

/// Nested configuration block for the `Pushover` provider.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PushoverProviderConfig {
    /// Pushover application token (the `T...` key). Supports `ENC[...]`.
    pub app_token: Option<String>,
    /// Map of logical recipient name → Pushover user or group key
    /// (`U...` / `G...`). Values support `ENC[...]`.
    pub recipients: HashMap<String, String>,
    /// Name of the default recipient used when the dispatch payload
    /// omits `user_key`.
    pub default_recipient: Option<String>,
    /// Override base URL for the Pushover Messages API (testing only).
    pub api_base_url: Option<String>,
}

/// Nested configuration block for the `Telegram` Bot provider.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TelegramProviderConfig {
    /// Telegram bot token (`{bot_id}:{auth-string}`). Supports `ENC[...]`.
    pub bot_token: Option<String>,
    /// Map of logical chat name → Telegram `chat_id`. Chat IDs
    /// can be numeric (`-1001234567890`) or string
    /// `@channelusername` handles. Chat IDs are **not** secrets.
    pub chats: HashMap<String, String>,
    /// Name of the default chat used when the dispatch payload
    /// omits `chat`.
    pub default_chat: Option<String>,
    /// Default `parse_mode` applied to outgoing messages when the
    /// payload omits it. `"HTML"`, `"Markdown"`, or `"MarkdownV2"`.
    pub default_parse_mode: Option<String>,
    /// Client-side `text` truncation cap, in **UTF-16 code units**
    /// — matches the units Telegram's API uses for its 4096 cap.
    /// One BMP character costs 1 unit; one non-BMP character
    /// (most emoji, some CJK supplementary ideographs) costs 2.
    pub text_max_utf16_units: Option<usize>,
    /// Override base URL for the Telegram Bot API (testing only).
    pub api_base_url: Option<String>,
}
