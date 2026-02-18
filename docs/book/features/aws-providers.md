# AWS Providers

Acteon ships with native AWS provider integrations for **SNS**, **Lambda**, **EventBridge**, **SQS**, **S3**, and **SES** (via the email provider). All six are first-class citizens -- they implement the same `Provider` trait, participate in circuit breaking, health checks, and per-provider metrics, and require no external plugins.

## Overview

| Provider | AWS Service | Action Types | Use Case |
|----------|------------|--------------|----------|
| `aws-sns` | Simple Notification Service | `publish` | Fan-out alerts to subscriptions (email, SMS, HTTP, Lambda) |
| `aws-lambda` | Lambda | `invoke` | Run serverless functions for data processing |
| `aws-eventbridge` | EventBridge | `put_event` | Publish domain events to event buses |
| `aws-sqs` | Simple Queue Service | `send_message` | Queue messages for async processing |
| `aws-s3` | Simple Storage Service | `put_object`, `get_object`, `delete_object` | Store and retrieve objects (logs, artifacts, archives) |
| `ses` (email) | Simple Email Service | `send_email` | Send transactional emails via SES |

All AWS providers:

- Share a common authentication layer with automatic STS credential refresh
- Support IAM role assumption with `session_name` and `external_id` for cross-account access
- Support endpoint URL overrides for LocalStack and other AWS-compatible services
- Report per-provider health metrics (success rate, latency percentiles, error tracking)
- Map AWS SDK errors to the standard `ProviderError` enum for circuit breaker integration

## TOML Configuration

### AWS SNS

```toml
[[providers]]
name = "alert-fanout"
type = "aws-sns"
aws_region = "us-east-1"
topic_arn = "arn:aws:sns:us-east-1:123456789012:alerts"
# Optional fields:
# aws_endpoint_url = "http://localhost:4566"   # LocalStack
# aws_role_arn = "arn:aws:iam::123:role/sns"   # Cross-account
# aws_session_name = "acteon-sns"              # STS session name
# aws_external_id = "ext-123"                  # Trust policy external ID
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"aws-sns"` |
| `aws_region` | Yes | AWS region |
| `topic_arn` | No | Default SNS topic ARN. Can be overridden per-action in the payload |
| `aws_endpoint_url` | No | Endpoint URL override (for LocalStack) |
| `aws_role_arn` | No | IAM role ARN to assume via STS |
| `aws_session_name` | No | STS session name (defaults to `"acteon-aws-provider"`) |
| `aws_external_id` | No | External ID for cross-account trust policies |

### AWS Lambda

```toml
[[providers]]
name = "anomaly-detector"
type = "aws-lambda"
aws_region = "us-east-1"
function_name = "anomaly-detector"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"aws-lambda"` |
| `aws_region` | Yes | AWS region |
| `function_name` | No | Default Lambda function name. Can be overridden per-action |
| `aws_endpoint_url` | No | Endpoint URL override |
| `aws_role_arn` | No | IAM role ARN to assume |
| `aws_session_name` | No | STS session name |
| `aws_external_id` | No | External ID for cross-account trust policies |

### AWS EventBridge

```toml
[[providers]]
name = "event-bus"
type = "aws-eventbridge"
aws_region = "us-east-1"
event_bus_name = "application-events"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"aws-eventbridge"` |
| `aws_region` | Yes | AWS region |
| `event_bus_name` | No | Default event bus name. Can be overridden per-action |
| `aws_endpoint_url` | No | Endpoint URL override |
| `aws_role_arn` | No | IAM role ARN to assume |
| `aws_session_name` | No | STS session name |
| `aws_external_id` | No | External ID for cross-account trust policies |

### AWS SQS

```toml
[[providers]]
name = "metrics-queue"
type = "aws-sqs"
aws_region = "us-east-1"
queue_url = "https://sqs.us-east-1.amazonaws.com/123456789012/metrics"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"aws-sqs"` |
| `aws_region` | Yes | AWS region |
| `queue_url` | No | Default SQS queue URL. Can be overridden per-action |
| `aws_endpoint_url` | No | Endpoint URL override |
| `aws_role_arn` | No | IAM role ARN to assume |
| `aws_session_name` | No | STS session name |
| `aws_external_id` | No | External ID for cross-account trust policies |

### AWS S3

```toml
[[providers]]
name = "archive"
type = "aws-s3"
aws_region = "us-east-1"
bucket_name = "acteon-archive"
object_prefix = "telemetry/"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"aws-s3"` |
| `aws_region` | Yes | AWS region |
| `bucket_name` | No | Default S3 bucket name. Can be overridden per-action |
| `object_prefix` | No | Key prefix prepended to all object keys |
| `aws_endpoint_url` | No | Endpoint URL override |
| `aws_role_arn` | No | IAM role ARN to assume |
| `aws_session_name` | No | STS session name |
| `aws_external_id` | No | External ID for cross-account trust policies |

### SES Email Backend

SES is configured through the email provider by setting `backend = "ses"`:

```toml
[[providers]]
name = "email"
type = "email"
backend = "ses"
from_address = "noreply@example.com"
aws_region = "us-east-1"
# ses_configuration_set = "tracking-set"  # Optional tracking config set
# aws_endpoint_url = "http://localhost:4566"
# aws_role_arn = "arn:aws:iam::123:role/ses"
# aws_session_name = "acteon-ses"
# aws_external_id = "ext-123"
```

See the [Email provider documentation](native-providers.md) for full payload format details.

## Payload Formats

### SNS: `publish`

```json
{
  "message": "Critical temperature alert: 92°C in server room",
  "subject": "Temperature Alert",
  "topic_arn": "arn:aws:sns:us-east-1:123:alerts",
  "message_group_id": "floor-3",
  "message_deduplication_id": "temp-alert-001"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `message` | Yes | string | Message body to publish |
| `subject` | No | string | Subject line (for email subscriptions) |
| `topic_arn` | No | string | Override the default topic ARN |
| `message_group_id` | No | string | FIFO topic group ID |
| `message_deduplication_id` | No | string | FIFO topic dedup ID |

**Response:**

```json
{
  "message_id": "abc123-def456",
  "status": "published"
}
```

### Lambda: `invoke`

```json
{
  "function_name": "anomaly-detector",
  "payload": {"device_id": "sensor-001", "value": 92},
  "invocation_type": "RequestResponse"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `function_name` | No | string | Override the default function name |
| `payload` | No | object | JSON payload passed to the Lambda function |
| `invocation_type` | No | string | `"RequestResponse"` (sync, default) or `"Event"` (async) |

**Response:**

```json
{
  "function_name": "anomaly-detector",
  "status_code": 200,
  "body": {"anomaly": true, "reason": "temperature_critical"}
}
```

### EventBridge: `put_event`

```json
{
  "source": "acteon.iot",
  "detail_type": "SensorReading",
  "detail": {"device_id": "sensor-001", "value": 92},
  "event_bus_name": "building-events"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `source` | Yes | string | Event source identifier |
| `detail_type` | Yes | string | Event type name |
| `detail` | Yes | object/string | Event detail (JSON object or string) |
| `event_bus_name` | No | string | Override the default event bus |

**Response:**

```json
{
  "entries": 1,
  "failed_count": 0,
  "status": "published"
}
```

### SQS: `send_message`

```json
{
  "message_body": "{\"device_id\": \"sensor-001\", \"value\": 92}",
  "queue_url": "https://sqs.us-east-1.amazonaws.com/123/metrics",
  "delay_seconds": 10,
  "message_group_id": "sensor-001",
  "message_deduplication_id": "reading-001"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `message_body` | Yes | string | Message body (JSON string) |
| `queue_url` | No | string | Override the default queue URL |
| `delay_seconds` | No | integer | Delay before the message is visible (0-900) |
| `message_group_id` | No | string | FIFO queue group ID |
| `message_deduplication_id` | No | string | FIFO queue dedup ID |

**Response:**

```json
{
  "message_id": "abc123-def456",
  "status": "sent"
}
```

### S3: `put_object`

```json
{
  "key": "telemetry/2026/02/readings.json",
  "body": "{\"readings\": [...]}",
  "content_type": "application/json",
  "bucket": "data-archive",
  "metadata": {
    "source": "acteon",
    "pipeline_version": "2.0"
  }
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `key` | Yes | string | Object key (path within the bucket) |
| `body` | No | string | Object body as UTF-8 text. Mutually exclusive with `body_base64` |
| `body_base64` | No | string | Object body as base64-encoded bytes. Mutually exclusive with `body` |
| `content_type` | No | string | MIME type for the object |
| `bucket` | No | string | Override the default bucket |
| `metadata` | No | object | Key-value metadata pairs attached to the object |

**Response:**

```json
{
  "bucket": "data-archive",
  "key": "telemetry/2026/02/readings.json",
  "status": "uploaded"
}
```

### S3: `get_object`

```json
{
  "key": "telemetry/2026/02/readings.json",
  "bucket": "data-archive"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `key` | Yes | string | Object key to retrieve |
| `bucket` | No | string | Override the default bucket |

**Response** (UTF-8 content):

```json
{
  "bucket": "data-archive",
  "key": "telemetry/2026/02/readings.json",
  "content_type": "application/json",
  "content_length": 1234,
  "body": "{\"readings\": [...]}",
  "status": "downloaded"
}
```

**Response** (binary content):

```json
{
  "bucket": "data-archive",
  "key": "images/logo.png",
  "content_type": "image/png",
  "content_length": 5678,
  "body_base64": "iVBORw0KGgo...",
  "status": "downloaded"
}
```

### S3: `delete_object`

```json
{
  "key": "old/data.csv",
  "bucket": "data-archive"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `key` | Yes | string | Object key to delete |
| `bucket` | No | string | Override the default bucket |

**Response:**

```json
{
  "bucket": "data-archive",
  "key": "old/data.csv",
  "status": "deleted"
}
```

## Authentication and Credential Refresh

All AWS providers share a common authentication layer in `acteon-aws`. Credentials are resolved in this order:

1. **Default credential chain** -- environment variables, `~/.aws/credentials`, EC2 instance profile, ECS task role
2. **IAM role assumption** (if `aws_role_arn` is configured) -- uses `AssumeRoleProvider` with automatic credential refresh before expiry

The `AssumeRoleProvider` handles the STS `AssumeRole` call internally and refreshes credentials transparently. This means long-running Acteon instances never encounter `ExpiredTokenException` errors, unlike manual STS calls with static credentials.

### Cross-Account Access

For multi-account deployments, configure role assumption with an external ID:

```toml
[[providers]]
name = "cross-account-sns"
type = "aws-sns"
aws_region = "us-east-1"
aws_role_arn = "arn:aws:iam::987654321098:role/acteon-sns-publisher"
aws_session_name = "acteon-prod"
aws_external_id = "acteon-trust-key"
topic_arn = "arn:aws:sns:us-east-1:987654321098:alerts"
```

The `aws_external_id` is validated against the trust policy's `Condition` block, providing an additional layer of security for cross-account delegation.

## Health Check Behavior

| Provider | Health Check Method | Notes |
|----------|-------------------|-------|
| `aws-sns` | `ListTopics` (max 1) | Verifies API connectivity and credentials |
| `aws-lambda` | `ListFunctions` (max 1) | Verifies API connectivity and credentials |
| `aws-eventbridge` | `ListEventBuses` (max 1) | Verifies API connectivity and credentials |
| `aws-sqs` | `ListQueues` (max 1) | Verifies API connectivity and credentials |
| `aws-s3` | `ListBuckets` (max 1) | Verifies API connectivity and credentials |
| SES (email) | `GetAccount` | Verifies SES sending is enabled |

All health checks are lightweight read-only operations that do not modify any resources.

## Error Handling

AWS SDK errors are classified into the standard `ProviderError` enum:

| AWS Error Pattern | ProviderError Variant | Retryable | Circuit Breaker |
|-------------------|----------------------|-----------|-----------------|
| Connection/dispatch failure | `Connection` | Yes | Counts toward trip |
| Throttling / `TooManyRequests` | `RateLimited` | Yes | Counts toward trip |
| Timeout | `Timeout` | Yes | Counts toward trip |
| Credential errors | `Configuration` | No | Does not count |
| Invalid request | `Serialization` | No | Does not count |
| Other service errors | `ExecutionFailed` | No | Counts toward trip |

## Example: Dispatching via the API

```bash
# Publish to SNS
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "alerts",
    "tenant": "acme-corp",
    "provider": "alert-fanout",
    "action_type": "publish",
    "payload": {
      "message": "Critical alert: CPU at 99%",
      "subject": "CPU Alert"
    }
  }'

# Invoke Lambda
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "processing",
    "tenant": "acme-corp",
    "provider": "anomaly-detector",
    "action_type": "invoke",
    "payload": {
      "payload": {"device_id": "sensor-001", "value": 92}
    }
  }'

# Upload to S3
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "archive",
    "tenant": "acme-corp",
    "provider": "archive",
    "action_type": "put_object",
    "payload": {
      "key": "reports/2026/02/daily.json",
      "body": "{\"total\": 42}",
      "content_type": "application/json"
    }
  }'
```

## Example: Rust Client

```rust
use acteon_client::ActeonClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ActeonClient::new("http://localhost:8080", "your-api-token")?;

    // Publish to SNS
    client.dispatch_action(
        "alerts", "acme-corp", "alert-fanout", "publish",
        serde_json::json!({
            "message": "Critical alert!",
            "subject": "Alert"
        }),
    ).await?;

    // Invoke Lambda
    client.dispatch_action(
        "processing", "acme-corp", "anomaly-detector", "invoke",
        serde_json::json!({
            "payload": {"device_id": "sensor-001", "value": 92}
        }),
    ).await?;

    // Upload to S3
    client.dispatch_action(
        "archive", "acme-corp", "archive", "put_object",
        serde_json::json!({
            "key": "data/report.json",
            "body": "{\"total\": 42}",
            "content_type": "application/json"
        }),
    ).await?;

    Ok(())
}
```

## LocalStack Development

All AWS providers support endpoint URL overrides, making it easy to develop and test locally with [LocalStack](https://localstack.cloud/):

```bash
# Start LocalStack
docker run --rm -d --name localstack -p 4566:4566 localstack/localstack
```

Point every provider at `http://localhost:4566`:

```toml
[[providers]]
name = "alert-fanout"
type = "aws-sns"
aws_region = "us-east-1"
aws_endpoint_url = "http://localhost:4566"
topic_arn = "arn:aws:sns:us-east-1:000000000000:alerts"
```

See the [AWS Event-Driven Pipeline Guide](../guides/aws-event-pipeline.md) for a complete runnable example using LocalStack with all six provider types.
