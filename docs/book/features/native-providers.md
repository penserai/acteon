# Native Providers

Acteon ships with built-in provider integrations for **Email**, **Slack**, **PagerDuty**, **Webhook**, **Twilio** (SMS), **Microsoft Teams**, and **Discord**. Native providers are first-class citizens -- they implement the same `Provider` trait, participate in circuit breaking, health checks, per-provider metrics, and tenant quotas, and require no external plugins.

!!! note "Default vs Opt-In Providers"
    Email, Slack, PagerDuty, Webhook, Twilio, and Microsoft Teams are part of the default `acteon-server` build. Discord is compiled when the `discord` feature flag is enabled (`cargo build -p acteon-server --features discord`) or as part of the `extras-alerting` feature group. See [Providers](../concepts/providers.md#messaging-and-on-call) for the complete provider matrix.

## Overview

| Provider | Transport | Auth Mechanism | Payload Format |
|----------|-----------|----------------|----------------|
| Email | SMTP / AWS SES | SMTP auth (user/pass) or AWS IAM | `application/json` (RFC 5322 MIME) |
| Slack | Slack Web API / Webhook | Bot Token (`xoxb-...`) or Webhook URL | `application/json` (Blocks / Text) |
| PagerDuty | Events API v2 | Routing Key | `application/json` (Trigger/Ack/Resolve) |
| Webhook | HTTP (REST / Webhook) | Bearer, Basic, API Key, HMAC-SHA256 | `application/json` |
| Twilio | REST API (form-encoded) | HTTP Basic Auth (Account SID + Auth Token) | `application/x-www-form-urlencoded` |
| Teams | Incoming Webhook | Webhook URL (URL is the credential) | `application/json` (MessageCard or Adaptive Card) |
| Discord | Webhook | Webhook URL (URL is the credential) | `application/json` (Content / Embeds) |

All native providers:

- Support `ENC[...]` encrypted secrets in TOML configuration
- Propagate W3C Trace Context (`traceparent`/`tracestate` headers) to downstream APIs
- Report per-provider health metrics (success rate, latency percentiles, error tracking)
- Handle HTTP 429 (Too Many Requests) as retryable `RateLimited` errors
- Use a 30-second HTTP client timeout by default

## TOML Configuration

### Email (SMTP & AWS SES)

```toml
# SMTP backend
[[providers]]
name = "corp-email"
type = "email"
email_backend = "smtp"              # "smtp" (default) or "ses"
smtp_host = "smtp.mailgun.org"
smtp_port = 587
username = "postmaster@example.com"
password = "ENC[AES256_GCM,data:...]"
from_address = "alerts@example.com"
tls = true

# AWS SES backend
[[providers]]
name = "ses-email"
type = "email"
email_backend = "ses"
aws_region = "us-east-1"
from_address = "notifications@example.com"
ses_configuration_set = "production-alerts"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"email"` |
| `email_backend` | No | `"smtp"` (default) or `"ses"` |
| `from_address` | Yes | Default sender email address |
| `smtp_host` | For SMTP | SMTP server hostname |
| `smtp_port` | No | SMTP port (default: 587 for submission, 465 for TLS) |
| `username` | No | SMTP authentication username |
| `password` | No | SMTP authentication password (supports `ENC[...]`) |
| `tls` | No | Whether to require TLS/STARTTLS (default: `true`) |
| `aws_region` | For SES | AWS region for SES API calls |
| `ses_configuration_set` | No | Optional SES configuration set name |

### Slack

```toml
[[providers]]
name = "slack-ops"
type = "slack"
webhook_url = "https://hooks.slack.com/services/T00/B00/XXXX"
default_channel = "#alerts"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"slack"` |
| `webhook_url` | Yes | Slack incoming webhook URL or Bot token (`xoxb-...`) |
| `default_channel` | No | Default target channel (can be overridden in payload) |

### PagerDuty

```toml
[[providers]]
name = "pagerduty-alerts"
type = "pagerduty"
routing_key = "ENC[AES256_GCM,data:...]"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"pagerduty"` |
| `routing_key` | Yes | PagerDuty Events API v2 32-character integration routing key (supports `ENC[...]`) |

### Webhook

```toml
[[providers]]
name = "custom-webhook"
type = "webhook"
url = "https://api.example.com/alerts"
headers = { "X-Source" = "acteon", "Authorization" = "Bearer ENC[...]" }
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"webhook"` |
| `url` | Yes | Target endpoint URL |
| `headers` | No | Map of custom HTTP headers sent with each request |

### Twilio

```toml
[[providers]]
name = "sms"
type = "twilio"
account_sid = "ACXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
auth_token = "ENC[AES256_GCM,data:abc123...]"
from_number = "+15551234567"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"twilio"` |
| `account_sid` | Yes | Twilio Account SID (starts with `AC`) |
| `auth_token` | Yes | Twilio Auth Token. Supports `ENC[...]` for encrypted storage |
| `from_number` | No | Default sender phone number in E.164 format. Can be overridden per-action via the `from` payload field |

### Microsoft Teams

```toml
[[providers]]
name = "teams-alerts"
type = "teams"
webhook_url = "https://outlook.office.com/webhook/xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"teams"` |
| `webhook_url` | Yes | Incoming Webhook URL from Teams channel configuration |

### Discord

```toml
[[providers]]
name = "discord-alerts"
type = "discord"
webhook_url = "https://discord.com/api/webhooks/123456789/abcdefg"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"discord"` |
| `webhook_url` | Yes | Discord webhook URL (from channel integrations settings) |

Discord also supports optional configuration for default username and avatar, configurable via the Rust API:

```rust
DiscordConfig::new("https://discord.com/api/webhooks/123/abc")
    .with_wait(true)               // Return created message object (200 instead of 204)
    .with_default_username("Acteon Bot")
    .with_default_avatar_url("https://example.com/avatar.png")
```

## Payload Format

### Email

Send an email notification via SMTP or AWS SES:

```json
{
  "to": "ops@example.com",
  "cc": ["lead@example.com"],
  "bcc": ["audit@example.com"],
  "subject": "Critical Alert: Database latency elevated",
  "body": "p99 database latency exceeded 500ms on shard 3.",
  "from": "alerts@example.com"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `to` | Yes | string | Destination email address (or array of addresses) |
| `subject` | Yes | string | Email subject line |
| `body` | Yes | string | Email body text (plain text or HTML) |
| `from` | No | string | Sender address (falls back to configured `from_address`) |
| `cc` | No | array | List of CC recipient email addresses |
| `bcc` | No | array | List of BCC recipient email addresses |
| `attachments` | No | array | Optional file attachments (see [Attachments](attachments.md)) |

### Slack

Send a notification to a Slack channel using plain text or Block Kit:

```json
{
  "channel": "#infrastructure-alerts",
  "text": "Disk space low on node worker-42",
  "blocks": [
    {
      "type": "section",
      "text": {
        "type": "mrkdwn",
        "text": "*Alert:* Disk usage at *92%* on `worker-42` (/var/lib/data)"
      }
    }
  ]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `text` | One of `text` or `blocks` | string | Fallback notification text |
| `blocks` | One of `text` or `blocks` | array | Slack Block Kit UI components |
| `channel` | No | string | Override target channel (e.g. `"#alerts"`, `"C12345678"`) |

### PagerDuty

Manage incident lifecycles via the PagerDuty Events API v2:

```json
{
  "event_action": "trigger",
  "summary": "Redis master node unresponsive",
  "severity": "critical",
  "source": "health-check",
  "dedup_key": "redis-cluster-master-down",
  "custom_details": {
    "node_id": "redis-01",
    "consecutive_timeouts": 5
  },
  "links": [
    { "href": "https://wiki.internal/runbooks/redis", "text": "Runbook" }
  ]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `event_action` | Yes | string | `"trigger"`, `"acknowledge"`, or `"resolve"` |
| `summary` | For `trigger` | string | Brief description of the incident |
| `severity` | No | string | `"critical"`, `"error"`, `"warning"`, or `"info"` |
| `dedup_key` | For `acknowledge`/`resolve` | string | Deduplication key correlating lifecycle events |
| `source` | No | string | Originating service or system hostname |
| `custom_details` | No | object | Arbitrary key-value context for responders |
| `links` | No | array | Array of `{href, text}` links attached to incident |
| `images` | No | array | Array of `{src, alt}` graph images attached to incident |

### Webhook

Send an HTTP request with arbitrary payload, headers, and method:

```json
{
  "url": "https://api.example.com/v1/incidents",
  "method": "POST",
  "body": {
    "service": "billing",
    "status": "degraded",
    "error_rate": 0.08
  },
  "headers": {
    "X-Source": "monitoring-gateway"
  }
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `body` | Yes | object/string | JSON payload delivered in the request body |
| `url` | No | string | Override endpoint URL (falls back to provider `url`) |
| `method` | No | string | HTTP method: `"POST"` (default), `"PUT"`, `"PATCH"` |
| `headers` | No | object | Additional request headers |

### Twilio SMS

Send an SMS message. Requires `to` (destination) and `body` (message text). The `from` field is optional if a default `from_number` is configured.

```json
{
  "to": "+15559876543",
  "body": "Server alert: CPU usage at 95%",
  "from": "+15551234567"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `to` | Yes | string | Destination phone number in E.164 format |
| `body` | Yes | string | SMS message text |
| `from` | No | string | Sender phone number (falls back to configured `from_number`) |
| `media_url` | No | string | URL for MMS media attachment |

**Response body** on success:

```json
{
  "sid": "SMxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
  "status": "queued"
}
```

### Microsoft Teams

Send a message to a Teams channel. Requires at least one of `text` (MessageCard) or `adaptive_card` (Adaptive Card).

**Simple MessageCard:**

```json
{
  "text": "Deployment complete",
  "title": "CI/CD Pipeline",
  "theme_color": "00FF00"
}
```

**Adaptive Card:**

```json
{
  "adaptive_card": {
    "type": "AdaptiveCard",
    "version": "1.4",
    "body": [
      {
        "type": "TextBlock",
        "text": "Build #42 passed all tests",
        "weight": "Bolder",
        "size": "Medium"
      }
    ]
  }
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `text` | One of `text` or `adaptive_card` | string | Message body text (supports basic Markdown) |
| `title` | No | string | Card title (MessageCard only) |
| `summary` | No | string | Summary text for notifications (MessageCard only) |
| `theme_color` | No | string | Hex color code without `#` prefix, e.g. `"FF0000"` |
| `adaptive_card` | One of `text` or `adaptive_card` | object | Full Adaptive Card JSON object |

When `adaptive_card` is provided, it is wrapped in the Teams attachment envelope automatically. When `text` is provided, it is formatted as an Office 365 MessageCard with the `@type: "MessageCard"` schema.

**Response body** on success:

```json
{
  "ok": true,
  "response": "1"
}
```

### Discord

Send a message to a Discord channel. Requires at least one of `content` (plain text) or `embeds` (rich embed objects).

**Simple text message:**

```json
{
  "content": "Build passed!"
}
```

**Rich embed message:**

```json
{
  "content": "Build status update",
  "embeds": [
    {
      "title": "Build #42",
      "description": "All tests passed",
      "color": 65280,
      "fields": [
        {
          "name": "Duration",
          "value": "3m 42s",
          "inline": true
        },
        {
          "name": "Branch",
          "value": "main",
          "inline": true
        }
      ],
      "footer": {
        "text": "Acteon CI"
      }
    }
  ]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `content` | One of `content` or `embeds` | string | Plain text message content |
| `username` | No | string | Override the webhook's default username |
| `avatar_url` | No | string | Override the webhook's default avatar URL |
| `tts` | No | bool | Whether to send as text-to-speech |
| `embeds` | One of `content` or `embeds` | array | Array of embed objects (max 10) |

**Embed object fields:**

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `title` | No | string | Embed title |
| `description` | No | string | Embed description |
| `color` | No | integer | Color as a decimal integer (e.g., `16711680` for red, `65280` for green) |
| `fields` | No | array | Array of `{name, value, inline?}` field objects |
| `footer` | No | object | Footer with `text` field |
| `timestamp` | No | string | ISO 8601 timestamp |

**Response body** on success (without `?wait=true`):

```json
{
  "ok": true
}
```

**Response body** on success (with `?wait=true`):

```json
{
  "ok": true,
  "id": "1234567890",
  "channel_id": "9876543210"
}
```

## Secret Management

The Twilio `auth_token` field supports the `ENC[...]` envelope for encrypted secrets. This integrates with Acteon's payload encryption at rest infrastructure:

```toml
[[providers]]
name = "sms"
type = "twilio"
account_sid = "ACXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
auth_token = "ENC[AES256_GCM,data:base64encodedciphertext...]"
```

For Teams and Discord, the webhook URL itself is the authentication credential. While webhook URLs are not wrapped in `ENC[...]` (they are used as-is for HTTP requests), they should be treated as secrets:

- Do not commit webhook URLs to version control
- Use environment variable substitution or external secret managers
- Rotate webhook URLs periodically via the Teams/Discord admin panels

## Health Check Behavior

Each provider implements a `health_check()` method that validates connectivity and credentials.

### Twilio

Performs a `GET` request to the Account API endpoint:

```
GET https://api.twilio.com/2010-04-01/Accounts/{AccountSid}.json
```

This verifies that the Account SID and Auth Token are valid and the Twilio API is reachable. An HTTP 200 response with account details indicates a healthy provider. Rate-limited responses (HTTP 429) are reported as `ProviderError::RateLimited`.

### Microsoft Teams

Sends a minimal message to the webhook URL:

```
POST {webhook_url}
Content-Type: application/json

{"text": "health check"}
```

Teams incoming webhooks do not have a dedicated health endpoint, so the provider sends a lightweight message. Any successful HTTP response from the webhook host confirms the URL is reachable and valid. This does result in a "health check" message appearing in the Teams channel.

### Discord

Performs a `GET` request to the webhook URL:

```
GET {webhook_url}
```

Discord returns the webhook object (name, channel, guild) on GET requests without executing the webhook. This provides a non-intrusive health check -- no message is posted to the channel.

## Error Handling

All native providers map their internal errors to the standard `ProviderError` enum:

| Internal Error | ProviderError Variant | Retryable |
|----------------|----------------------|-----------|
| HTTP transport / connection failure | `Connection` | Yes |
| Upstream timeout | `Timeout` | Yes |
| API rejection / HTTP 4xx (except 429) | `ExecutionFailed` | No |
| Invalid / unparseable payload fields | `Serialization` | No |
| HTTP 429 Too Many Requests | `RateLimited` | Yes |
| Invalid credentials / host config | `Configuration` | No |

Retryable errors participate in circuit breaker trips and retry policies. Non-retryable errors immediately fail the action.

## Example: Dispatching via the API

```bash
# Send SMS via Twilio
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "alerts",
    "tenant": "acme-corp",
    "provider": "sms",
    "action_type": "send_sms",
    "payload": {
      "to": "+15559876543",
      "body": "Server alert: disk usage at 90%"
    }
  }'

# Send Teams notification
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "alerts",
    "tenant": "acme-corp",
    "provider": "teams-alerts",
    "action_type": "notify",
    "payload": {
      "text": "Deployment complete",
      "title": "CI/CD",
      "theme_color": "00FF00"
    }
  }'

# Send Discord notification with embed
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "alerts",
    "tenant": "acme-corp",
    "provider": "discord-alerts",
    "action_type": "notify",
    "payload": {
      "content": "Build passed!",
      "embeds": [{
        "title": "Build #42",
        "description": "All tests passed",
        "color": 65280
      }]
    }
  }'

# Send Email notification
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "alerts",
    "tenant": "acme-corp",
    "provider": "corp-email",
    "action_type": "send_email",
    "payload": {
      "to": "oncall@example.com",
      "subject": "Critical: Shard Failover",
      "body": "Database shard 4 failed over to replica."
    }
  }'
```

## Example: Rust Client

```rust
use acteon_client::ActeonClientBuilder;
use acteon_core::Action;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ActeonClientBuilder::new("http://localhost:8080")
        .api_key("your-api-token")
        .build()?;

    // Send SMS
    let sms_action = Action::new(
        "alerts", "acme-corp", "sms", "send_sms",
        serde_json::json!({
            "to": "+15559876543",
            "body": "Server alert!"
        }),
    );
    client.dispatch(&sms_action).await?;

    // Send Teams message
    let teams_action = Action::new(
        "alerts", "acme-corp", "teams-alerts", "notify",
        serde_json::json!({
            "text": "Deployment complete",
            "title": "CI/CD",
            "theme_color": "00FF00"
        }),
    );
    client.dispatch(&teams_action).await?;

    // Send Discord message
    let discord_action = Action::new(
        "alerts", "acme-corp", "discord-alerts", "notify",
        serde_json::json!({
            "content": "Build passed!",
            "embeds": [{
                "title": "Build #42",
                "description": "All tests passed",
                "color": 65280
            }]
        }),
    );
    client.dispatch(&discord_action).await?;

    // Send Email
    let email_action = Action::new(
        "alerts", "acme-corp", "corp-email", "send_email",
        serde_json::json!({
            "to": "ops@example.com",
            "subject": "Production Notice",
            "body": "Deployment completed successfully."
        }),
    );
    client.dispatch(&email_action).await?;

    Ok(())
}
```
