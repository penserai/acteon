# Native Providers

Acteon ships with built-in provider integrations for **Twilio** (SMS), **Microsoft Teams**, and **Discord**, alongside the existing Webhook, Email, Slack, and PagerDuty providers. Native providers are first-class citizens -- they implement the same `Provider` trait, participate in circuit breaking, health checks, and per-provider metrics, and require no external plugins.

## Overview

| Provider | Transport | Auth Mechanism | Payload Format |
|----------|-----------|----------------|----------------|
| Twilio | REST API (form-encoded) | HTTP Basic Auth (Account SID + Auth Token) | `application/x-www-form-urlencoded` |
| Teams | Incoming Webhook | Webhook URL (URL is the credential) | `application/json` (MessageCard or Adaptive Card) |
| Discord | Webhook | Webhook URL (URL is the credential) | `application/json` |

All three providers:

- Support `ENC[...]` encrypted secrets in TOML configuration
- Propagate W3C Trace Context (`traceparent`/`tracestate` headers) to downstream APIs
- Report per-provider health metrics (success rate, latency percentiles, error tracking)
- Handle HTTP 429 (Too Many Requests) as retryable `RateLimited` errors
- Use a 30-second HTTP client timeout by default

## TOML Configuration

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

All three providers map their internal errors to the standard `ProviderError` enum:

| Internal Error | ProviderError Variant | Retryable |
|----------------|----------------------|-----------|
| HTTP transport failure | `Connection` | Yes |
| API error response | `ExecutionFailed` | No |
| Invalid/missing payload fields | `Serialization` | No |
| HTTP 429 Too Many Requests | `RateLimited` | Yes |

Retryable errors participate in the circuit breaker and retry infrastructure. Non-retryable errors (invalid payloads, API rejections) fail immediately without retry.

## Example: Dispatching via the API

```bash
# Send SMS via Twilio
curl -X POST http://localhost:8080/v1/actions \
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
curl -X POST http://localhost:8080/v1/actions \
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
curl -X POST http://localhost:8080/v1/actions \
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
```

## Example: Rust Client

```rust
use acteon_client::ActeonClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ActeonClient::new("http://localhost:8080", "your-api-token")?;

    // Send SMS
    client.dispatch_action(
        "alerts", "acme-corp", "sms", "send_sms",
        serde_json::json!({
            "to": "+15559876543",
            "body": "Server alert!"
        }),
    ).await?;

    // Send Teams message
    client.dispatch_action(
        "alerts", "acme-corp", "teams-alerts", "notify",
        serde_json::json!({
            "text": "Deployment complete",
            "title": "CI/CD",
            "theme_color": "00FF00"
        }),
    ).await?;

    // Send Discord message
    client.dispatch_action(
        "alerts", "acme-corp", "discord-alerts", "notify",
        serde_json::json!({
            "content": "Build passed!",
            "embeds": [{
                "title": "Build #42",
                "description": "All tests passed",
                "color": 65280
            }]
        }),
    ).await?;

    Ok(())
}
```
