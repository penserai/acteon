# AWS Providers — Architecture

## Overview

The `acteon-aws` crate provides native AWS service integrations as Acteon providers.
Each provider implements the `Provider` trait from `acteon-provider`, allowing AWS
services to participate in the full dispatch pipeline: rule evaluation, circuit
breaking, health checks, metrics, and audit.

## Crate Structure

```
crates/aws/
├── Cargo.toml          # Feature-gated deps: sns, lambda, eventbridge, sqs, s3, ses
├── src/
│   ├── lib.rs          # Module declarations + re-exports
│   ├── auth.rs         # Shared AWS authentication (STS auto-refresh)
│   ├── config.rs       # AwsBaseConfig (region, role_arn, endpoint_url, ...)
│   ├── error.rs        # AWS SDK error → ProviderError classification
│   ├── sns.rs          # SnsConfig, SnsProvider
│   ├── lambda.rs       # LambdaConfig, LambdaProvider
│   ├── eventbridge.rs  # EventBridgeConfig, EventBridgeProvider
│   ├── sqs.rs          # SqsConfig, SqsProvider
│   ├── s3.rs           # S3Config, S3Provider
│   └── ses.rs          # SesConfig, SesClient (used by email provider)
```

Each provider module is feature-gated:

| Feature | Module | AWS SDK Dependency |
|---------|--------|-------------------|
| `sns` | `sns.rs` | `aws-sdk-sns` |
| `lambda` | `lambda.rs` | `aws-sdk-lambda` |
| `eventbridge` | `eventbridge.rs` | `aws-sdk-eventbridge` |
| `sqs` | `sqs.rs` | `aws-sdk-sqs` |
| `s3` | `s3.rs` | `aws-sdk-s3` |
| `ses` | `ses.rs` | `aws-sdk-sesv2` |
| `full` | All of the above | All |

## Authentication Layer

### Credential Resolution

`auth.rs` exports a single function:

```rust
pub async fn build_sdk_config(config: &AwsBaseConfig) -> SdkConfig
```

The credential resolution flow:

```
AwsBaseConfig
    │
    ├── region → aws_config::Region
    ├── endpoint_url → loader.endpoint_url()
    │
    └── role_arn?
         │
         ├── None → default credential chain
         │          (env vars → ~/.aws/credentials → EC2/ECS role)
         │
         └── Some(arn) → AssumeRoleProvider
                          ├── session_name (default: "acteon-aws-provider")
                          ├── external_id (optional, for cross-account)
                          └── configure(&base_config) → inherits endpoint overrides
                              → auto-refresh before expiry
```

### STS Auto-Refresh (Critical Fix)

The previous implementation manually called STS `AssumeRole` once, extracted
temporary credentials, and froze them as `Credentials::from_keys()`. These static
credentials expired after ~1 hour, causing `ExpiredTokenException` in long-running
instances.

The current implementation uses `AssumeRoleProvider` from `aws-config`, which:

1. Calls STS `AssumeRole` on first use
2. Caches the temporary credentials
3. Automatically refreshes them before expiry (typically at ~75% of TTL)
4. Is thread-safe and can be shared across multiple AWS clients

This is critical for production deployments where Acteon runs as a long-lived service.

### AwsBaseConfig

Shared configuration for all AWS providers:

```rust
pub struct AwsBaseConfig {
    pub region: String,              // AWS region (required)
    pub role_arn: Option<String>,    // IAM role to assume
    pub endpoint_url: Option<String>, // Override (LocalStack)
    pub session_name: Option<String>, // STS session name
    pub external_id: Option<String>,  // Cross-account external ID
}
```

Each provider config (e.g., `SnsConfig`, `S3Config`) contains an `AwsBaseConfig`
via `#[serde(flatten)]`, inheriting all base fields plus adding provider-specific
fields (topic ARN, bucket name, etc.).

## Error Classification

`error.rs` maps AWS SDK error strings to `ProviderError` variants:

| AWS Error Pattern | Classification | Retryable |
|-------------------|---------------|-----------|
| `"dispatch failure"`, `"connection"` | `AwsErrorKind::Connection` → `ProviderError::Connection` | Yes |
| `"throttl"`, `"too many"`, `"rate"` | `AwsErrorKind::Throttled` → `ProviderError::RateLimited` | Yes |
| `"timeout"`, `"timed out"` | `AwsErrorKind::Timeout` → `ProviderError::Timeout` | Yes |
| `"credential"`, `"expired"` | `AwsErrorKind::Credential` → `ProviderError::Configuration` | No |
| `"invalid"`, `"malformed"` | `AwsErrorKind::InvalidPayload` → `ProviderError::Serialization` | No |
| Everything else | `AwsErrorKind::ServiceError` → `ProviderError::ExecutionFailed` | Depends |

This classification determines whether the circuit breaker counts a failure toward
its trip threshold and whether the executor retries the request.

## Provider Pattern

All AWS providers follow the same implementation pattern:

```rust
pub struct XxxProvider {
    config: XxxConfig,
    client: aws_sdk_xxx::Client,
}

impl XxxProvider {
    pub async fn new(config: XxxConfig) -> Self {
        let sdk_config = build_sdk_config(&config.aws).await;
        let client = aws_sdk_xxx::Client::new(&sdk_config);
        Self { config, client }
    }

    pub fn with_client(config: XxxConfig, client: aws_sdk_xxx::Client) -> Self {
        Self { config, client }
    }
}

impl Provider for XxxProvider {
    fn name(&self) -> &str { "aws-xxx" }

    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> {
        // 1. Deserialize action.payload into typed struct
        // 2. Resolve defaults from config (topic_arn, bucket, etc.)
        // 3. Build AWS SDK request
        // 4. Send request, map errors via classify_sdk_error()
        // 5. Return ProviderResponse::success(json!({...}))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        // Lightweight read-only API call (list with max 1)
    }
}
```

### Config Pattern

Each config struct:

- Uses `#[serde(flatten)]` for `AwsBaseConfig`
- Implements manual `Debug` that redacts `role_arn` via `AwsBaseConfig`'s `Debug`
- Provides `with_*()` builder methods (all `#[must_use]`)
- Delegates common fields to `AwsBaseConfig` methods

### SES Integration

SES is unique -- it's not a standalone provider but a backend for the email provider.
The `acteon-email` crate's `EmailConfig` has a `backend` field (`"smtp"` or `"ses"`).
When `"ses"`, it constructs an `SesConfig` and creates an `SesClient` internally.
The `SesClient` wraps `aws_sdk_sesv2::Client` and implements `send_email()` and
`health_check()` methods directly, rather than implementing the `Provider` trait.

## Server Integration

### Config (`crates/server/src/config.rs`)

The `ProviderConfig` struct maps TOML fields to provider construction:

```toml
[[providers]]
name = "my-provider"
type = "aws-sns"          # Determines which provider to construct
aws_region = "us-east-1"
aws_endpoint_url = "..."  # Optional, shared across all AWS types
aws_role_arn = "..."      # Optional, shared
aws_session_name = "..."  # Optional, shared
aws_external_id = "..."   # Optional, shared
topic_arn = "..."         # SNS-specific
function_name = "..."     # Lambda-specific
event_bus_name = "..."    # EventBridge-specific
queue_url = "..."         # SQS-specific
bucket_name = "..."       # S3-specific
object_prefix = "..."     # S3-specific
```

### Wiring (`crates/server/src/main.rs`)

Provider construction in `main.rs` matches on `type`:

```rust
"aws-sns" => SnsProvider::new(SnsConfig::new(region).with_topic_arn(...)...),
"aws-lambda" => LambdaProvider::new(LambdaConfig::new(region)...),
"aws-eventbridge" => EventBridgeProvider::new(EventBridgeConfig::new(region)...),
"aws-sqs" => SqsProvider::new(SqsConfig::new(region)...),
"aws-s3" => S3Provider::new(S3Config::new(region).with_bucket(...)...),
```

All AWS types share the same pattern for wiring `aws_endpoint_url`, `aws_role_arn`,
`aws_session_name`, and `aws_external_id`.

## Testing Strategy

### Unit Tests (62 tests)

Each provider module has unit tests covering:

- Config construction and builder chain
- Serde roundtrip (serialize → deserialize)
- Debug output redaction
- Payload deserialization (all action types, minimal and full payloads)

### Integration Testing

AWS providers are integration-tested via:

1. **LocalStack** -- the `aws-event-pipeline` example exercises all providers end-to-end
2. **Simulation framework** -- mock providers verify the gateway's routing and chain
   orchestration without real AWS calls

No mock AWS clients are needed in unit tests because the provider logic (payload
deserialization, config resolution, key prefixing) is testable without SDK calls.
The SDK call boundary (`client.xxx().send().await`) is the integration test surface.
