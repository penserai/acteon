# AWS Providers

Acteon ships with native AWS provider integrations for **SNS**, **Lambda**, **EventBridge**, **SQS**, **S3**, **SES** (via the email provider), **EC2**, and **Auto Scaling**. All eight are first-class citizens -- they implement the same `Provider` trait, participate in circuit breaking, health checks, and per-provider metrics, and require no external plugins.

## Overview

| Provider | AWS Service | Action Types | Use Case |
|----------|------------|--------------|----------|
| `aws-sns` | Simple Notification Service | `publish` | Fan-out alerts to subscriptions (email, SMS, HTTP, Lambda) |
| `aws-lambda` | Lambda | `invoke` | Run serverless functions for data processing |
| `aws-eventbridge` | EventBridge | `put_event` | Publish domain events to event buses |
| `aws-sqs` | Simple Queue Service | `send_message` | Queue messages for async processing |
| `aws-s3` | Simple Storage Service | `put_object`, `get_object`, `delete_object` | Store and retrieve objects (logs, artifacts, archives) |
| `ses` (email) | Simple Email Service | `send_email` | Send transactional emails via SES |
| `aws-ec2` | EC2 | `start_instances`, `stop_instances`, `reboot_instances`, `terminate_instances`, `hibernate_instances`, `run_instances`, `attach_volume`, `detach_volume`, `describe_instances` | Instance lifecycle management, EBS volume operations |
| `aws-autoscaling` | Auto Scaling | `describe_auto_scaling_groups`, `set_desired_capacity`, `update_auto_scaling_group` | Manage Auto Scaling Group capacity and settings |

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

### AWS EC2

```toml
[[providers]]
name = "compute"
type = "aws-ec2"
aws_region = "us-east-1"
# Optional defaults applied to run_instances when not overridden in the payload:
# default_security_group_ids = ["sg-0123456789abcdef0"]
# default_subnet_id = "subnet-0123456789abcdef0"
# default_key_name = "my-keypair"
# aws_endpoint_url = "http://localhost:4566"   # LocalStack
# aws_role_arn = "arn:aws:iam::123:role/ec2"   # Cross-account
# aws_session_name = "acteon-ec2"              # STS session name
# aws_external_id = "ext-123"                  # Trust policy external ID
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"aws-ec2"` |
| `aws_region` | Yes | AWS region |
| `default_security_group_ids` | No | Default security group IDs for `run_instances`. Can be overridden per-action |
| `default_subnet_id` | No | Default subnet ID for `run_instances`. Can be overridden per-action |
| `default_key_name` | No | Default key-pair name for `run_instances`. Can be overridden per-action |
| `aws_endpoint_url` | No | Endpoint URL override (for LocalStack) |
| `aws_role_arn` | No | IAM role ARN to assume via STS |
| `aws_session_name` | No | STS session name (defaults to `"acteon-aws-provider"`) |
| `aws_external_id` | No | External ID for cross-account trust policies |

### AWS Auto Scaling

```toml
[[providers]]
name = "scaling"
type = "aws-autoscaling"
aws_region = "us-east-1"
# aws_endpoint_url = "http://localhost:4566"   # LocalStack
# aws_role_arn = "arn:aws:iam::123:role/asg"   # Cross-account
# aws_session_name = "acteon-asg"              # STS session name
# aws_external_id = "ext-123"                  # Trust policy external ID
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"aws-autoscaling"` |
| `aws_region` | Yes | AWS region |
| `aws_endpoint_url` | No | Endpoint URL override (for LocalStack) |
| `aws_role_arn` | No | IAM role ARN to assume via STS |
| `aws_session_name` | No | STS session name (defaults to `"acteon-aws-provider"`) |
| `aws_external_id` | No | External ID for cross-account trust policies |

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

### EC2: `start_instances`

```json
{
  "instance_ids": ["i-0abc123def456789a", "i-0def456abc789012b"]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `instance_ids` | Yes | string[] | EC2 instance IDs to start |

**Response:**

```json
{
  "action": "start_instances",
  "instance_state_changes": [
    {
      "instance_id": "i-0abc123def456789a",
      "previous_state": "stopped",
      "current_state": "pending"
    }
  ]
}
```

### EC2: `stop_instances`

```json
{
  "instance_ids": ["i-0abc123def456789a"],
  "hibernate": false,
  "force": false
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `instance_ids` | Yes | string[] | EC2 instance IDs to stop |
| `hibernate` | No | bool | Hibernate the instances instead of stopping (default `false`) |
| `force` | No | bool | Force stop without graceful shutdown (default `false`) |

**Response:**

```json
{
  "action": "stop_instances",
  "hibernate": false,
  "instance_state_changes": [
    {
      "instance_id": "i-0abc123def456789a",
      "previous_state": "running",
      "current_state": "stopping"
    }
  ]
}
```

### EC2: `reboot_instances`

```json
{
  "instance_ids": ["i-0abc123def456789a"]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `instance_ids` | Yes | string[] | EC2 instance IDs to reboot |

**Response:**

```json
{
  "action": "reboot_instances",
  "instance_ids": ["i-0abc123def456789a"],
  "status": "rebooting"
}
```

### EC2: `terminate_instances`

```json
{
  "instance_ids": ["i-0abc123def456789a"]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `instance_ids` | Yes | string[] | EC2 instance IDs to terminate |

**Response:**

```json
{
  "action": "terminate_instances",
  "instance_state_changes": [
    {
      "instance_id": "i-0abc123def456789a",
      "previous_state": "running",
      "current_state": "shutting-down"
    }
  ]
}
```

### EC2: `hibernate_instances`

Sugar action type that internally dispatches `stop_instances` with `hibernate: true`.

```json
{
  "instance_ids": ["i-0abc123def456789a"]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `instance_ids` | Yes | string[] | EC2 instance IDs to hibernate |

**Response:** Same as `stop_instances` with `"hibernate": true`.

### EC2: `run_instances`

```json
{
  "image_id": "ami-0abcdef1234567890",
  "instance_type": "t3.micro",
  "min_count": 1,
  "max_count": 2,
  "key_name": "my-keypair",
  "security_group_ids": ["sg-0123456789abcdef0"],
  "subnet_id": "subnet-0123456789abcdef0",
  "user_data": "IyEvYmluL2Jhc2gKZWNobyBIZWxsbw==",
  "tags": {
    "env": "staging",
    "team": "platform"
  },
  "iam_instance_profile": "arn:aws:iam::123456789012:instance-profile/app-profile"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `image_id` | Yes | string | AMI ID to launch |
| `instance_type` | Yes | string | Instance type (e.g. `"t3.micro"`) |
| `min_count` | No | integer | Minimum instances to launch (default 1) |
| `max_count` | No | integer | Maximum instances to launch (default 1) |
| `key_name` | No | string | Key-pair name. Overrides config `default_key_name` |
| `security_group_ids` | No | string[] | Security group IDs. Overrides config `default_security_group_ids` |
| `subnet_id` | No | string | Subnet ID. Overrides config `default_subnet_id` |
| `user_data` | No | string | Base64-encoded user data script |
| `tags` | No | object | Key-value tags applied to launched instances |
| `iam_instance_profile` | No | string | IAM instance profile name or ARN |

**Response:**

```json
{
  "action": "run_instances",
  "reservation_id": "r-0abc123def456789a",
  "instances": [
    {
      "instance_id": "i-0abc123def456789a",
      "instance_type": "t3.micro",
      "state": "pending"
    }
  ]
}
```

### EC2: `attach_volume`

```json
{
  "volume_id": "vol-0abc123def456789a",
  "instance_id": "i-0abc123def456789a",
  "device": "/dev/sdf"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `volume_id` | Yes | string | EBS volume ID to attach |
| `instance_id` | Yes | string | Target EC2 instance ID |
| `device` | Yes | string | Device name (e.g. `"/dev/sdf"`) |

**Response:**

```json
{
  "action": "attach_volume",
  "volume_id": "vol-0abc123def456789a",
  "instance_id": "i-0abc123def456789a",
  "device": "/dev/sdf",
  "state": "attaching"
}
```

### EC2: `detach_volume`

```json
{
  "volume_id": "vol-0abc123def456789a",
  "instance_id": "i-0abc123def456789a",
  "device": "/dev/sdf",
  "force": false
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `volume_id` | Yes | string | EBS volume ID to detach |
| `instance_id` | No | string | Instance to detach from |
| `device` | No | string | Device name |
| `force` | No | bool | Force detachment (default `false`) |

**Response:**

```json
{
  "action": "detach_volume",
  "volume_id": "vol-0abc123def456789a",
  "instance_id": "i-0abc123def456789a",
  "state": "detaching"
}
```

### EC2: `describe_instances`

```json
{
  "instance_ids": ["i-0abc123def456789a"]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `instance_ids` | No | string[] | Instance IDs to describe. If empty, describes all instances |

**Response:**

```json
{
  "action": "describe_instances",
  "instances": [
    {
      "instance_id": "i-0abc123def456789a",
      "instance_type": "t3.micro",
      "state": "running",
      "public_ip": "54.123.45.67",
      "private_ip": "10.0.1.42"
    }
  ]
}
```

### Auto Scaling: `describe_auto_scaling_groups`

```json
{
  "auto_scaling_group_names": ["web-tier-asg", "api-tier-asg"]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `auto_scaling_group_names` | No | string[] | Group names to describe. If empty, describes all groups |

**Response:**

```json
{
  "action": "describe_auto_scaling_groups",
  "auto_scaling_groups": [
    {
      "auto_scaling_group_name": "web-tier-asg",
      "min_size": 2,
      "max_size": 10,
      "desired_capacity": 4,
      "instance_count": 4,
      "health_check_type": "ELB"
    }
  ]
}
```

### Auto Scaling: `set_desired_capacity`

```json
{
  "auto_scaling_group_name": "web-tier-asg",
  "desired_capacity": 6,
  "honor_cooldown": true
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `auto_scaling_group_name` | Yes | string | Auto Scaling Group name |
| `desired_capacity` | Yes | integer | Target capacity |
| `honor_cooldown` | No | bool | Whether to honor the group's cooldown period (default `false`) |

**Response:**

```json
{
  "action": "set_desired_capacity",
  "auto_scaling_group_name": "web-tier-asg",
  "desired_capacity": 6,
  "status": "updated"
}
```

### Auto Scaling: `update_auto_scaling_group`

```json
{
  "auto_scaling_group_name": "web-tier-asg",
  "min_size": 2,
  "max_size": 20,
  "desired_capacity": 8,
  "default_cooldown": 300,
  "health_check_type": "ELB",
  "health_check_grace_period": 120
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `auto_scaling_group_name` | Yes | string | Auto Scaling Group name |
| `min_size` | No | integer | New minimum size |
| `max_size` | No | integer | New maximum size |
| `desired_capacity` | No | integer | New desired capacity |
| `default_cooldown` | No | integer | Default cooldown period in seconds |
| `health_check_type` | No | string | Health check type (`"EC2"` or `"ELB"`) |
| `health_check_grace_period` | No | integer | Health check grace period in seconds |

**Response:**

```json
{
  "action": "update_auto_scaling_group",
  "auto_scaling_group_name": "web-tier-asg",
  "status": "updated"
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
| `aws-ec2` | `DescribeInstances` (dry-run) | `DryRunOperation` error = healthy; verifies IAM permissions |
| `aws-autoscaling` | `DescribeAutoScalingGroups` (max 1) | Verifies API connectivity and credentials |

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

# Start EC2 instances
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "compute",
    "tenant": "acme-corp",
    "provider": "compute",
    "action_type": "start_instances",
    "payload": {
      "instance_ids": ["i-0abc123def456789a"]
    }
  }'

# Scale up an Auto Scaling Group
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "scaling",
    "tenant": "acme-corp",
    "provider": "scaling",
    "action_type": "set_desired_capacity",
    "payload": {
      "auto_scaling_group_name": "web-tier-asg",
      "desired_capacity": 6
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

    // Start EC2 instances
    client.dispatch_action(
        "compute", "acme-corp", "compute", "start_instances",
        serde_json::json!({
            "instance_ids": ["i-0abc123def456789a"]
        }),
    ).await?;

    // Launch new EC2 instances
    client.dispatch_action(
        "compute", "acme-corp", "compute", "run_instances",
        serde_json::json!({
            "image_id": "ami-0abcdef1234567890",
            "instance_type": "t3.micro",
            "max_count": 2,
            "tags": {"env": "staging"}
        }),
    ).await?;

    // Scale up an Auto Scaling Group
    client.dispatch_action(
        "scaling", "acme-corp", "scaling", "set_desired_capacity",
        serde_json::json!({
            "auto_scaling_group_name": "web-tier-asg",
            "desired_capacity": 6
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

See the [AWS Event-Driven Pipeline Guide](../guides/aws-event-pipeline.md) for a complete runnable example using LocalStack with all eight provider types.
