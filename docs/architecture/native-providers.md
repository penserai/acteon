# Native Providers Architecture

## Overview

The Twilio, Microsoft Teams, and Discord providers are implemented as separate crates under `crates/integrations/` and follow a uniform architecture. Each crate contains four modules:

```
crates/integrations/{twilio,teams,discord}/src/
  config.rs    -- Provider-specific configuration struct
  error.rs     -- Provider-specific error enum + From<XxxError> for ProviderError
  provider.rs  -- Provider trait implementation (execute + health_check)
  types.rs     -- Request/response types (serialization structs)
  lib.rs       -- Public re-exports
```

All three providers implement the `Provider` trait defined in `crates/provider/src/provider.rs`, which makes them compatible with the `DynProvider` blanket impl and the provider registry.

---

## 1. Provider Trait Implementation

Each provider struct holds a configuration and a `reqwest::Client`:

```rust
pub struct TwilioProvider {
    config: TwilioConfig,
    client: Client,
}

impl Provider for TwilioProvider {
    fn name(&self) -> &str { "twilio" }
    async fn execute(&self, action: &Action) -> Result<ProviderResponse, ProviderError> { ... }
    async fn health_check(&self) -> Result<(), ProviderError> { ... }
}
```

The `Provider` trait is not object-safe (native `async fn`), but the blanket `DynProvider` impl enables `Arc<dyn DynProvider>` for dynamic dispatch in the registry.

Each provider follows the same execution flow:

1. Deserialize `action.payload` into a provider-specific `MessagePayload` struct
2. Validate required fields (return `InvalidPayload` on failure)
3. Build the outgoing HTTP request (provider-specific format)
4. Inject trace context via `acteon_provider::inject_trace_context()`
5. Send the HTTP request, check for rate limiting (HTTP 429)
6. Parse the response and return `ProviderResponse::success(body)`

---

## 2. Error Taxonomy

Each provider defines a local error enum with four variants:

```rust
pub enum TwilioError {   // Same pattern for TeamsError, DiscordError
    Http(reqwest::Error),
    Api(String),
    InvalidPayload(String),
    RateLimited,
}
```

These map to `ProviderError` via a `From` implementation:

| Provider Error | ProviderError | Retryable | Description |
|----------------|---------------|-----------|-------------|
| `Http(reqwest::Error)` | `Connection(String)` | Yes | Network/transport-level failure (DNS, TCP, TLS) |
| `Api(String)` | `ExecutionFailed(String)` | No | API returned non-success response (auth error, bad request) |
| `InvalidPayload(String)` | `Serialization(String)` | No | Missing or malformed fields in action payload |
| `RateLimited` | `RateLimited` | Yes | HTTP 429 Too Many Requests |

The `is_retryable()` method on `ProviderError` returns `true` for `Connection`, `Timeout`, and `RateLimited`, enabling the gateway's retry and circuit breaker infrastructure to distinguish transient from permanent failures.

### Error Flow

```
TwilioError::Api("Authentication Error")
    |
    v  (From<TwilioError> for ProviderError)
ProviderError::ExecutionFailed("Authentication Error")
    |
    v  (is_retryable() == false)
Gateway: fail immediately, no retry
```

```
TwilioError::RateLimited
    |
    v  (From<TwilioError> for ProviderError)
ProviderError::RateLimited
    |
    v  (is_retryable() == true)
Gateway: retry with backoff, update circuit breaker
```

---

## 3. Trace Context Propagation

All three providers use `acteon_provider::inject_trace_context()` to propagate W3C Trace Context headers on outgoing HTTP requests:

```rust
let response = acteon_provider::inject_trace_context(
    self.client.post(&url).json(&body),
)
.send()
.await?;
```

The `inject_trace_context()` function (defined in `crates/provider/src/trace_context.rs`) reads the current OpenTelemetry span context and injects `traceparent` and `tracestate` headers into the `reqwest::RequestBuilder`. When no global propagator is registered (OpenTelemetry disabled), this is a no-op.

This enables end-to-end distributed tracing from the Acteon gateway through to the downstream API (Twilio, Teams connector, Discord).

The feature is gated behind the `trace-context` Cargo feature flag on the `acteon-provider` crate.

---

## 4. Twilio: Form Encoding

Twilio is the only native provider that uses `application/x-www-form-urlencoded` instead of JSON. The `TwilioSendMessageRequest` struct uses `serde::Serialize` with `#[serde(rename = "...")]` to match Twilio's PascalCase field names:

```rust
#[derive(Serialize)]
pub struct TwilioSendMessageRequest {
    #[serde(rename = "To")]
    pub to: String,
    #[serde(rename = "From")]
    pub from: String,
    #[serde(rename = "Body")]
    pub body: String,
    #[serde(rename = "MediaUrl", skip_serializing_if = "Option::is_none")]
    pub media_url: Option<String>,
}
```

The request is sent with `.form(request)` instead of `.json(request)`:

```rust
self.client
    .post(&url)
    .basic_auth(&self.config.account_sid, Some(&self.config.auth_token))
    .form(request)  // <-- form encoding, not JSON
```

This produces a request body like:

```
To=%2B15559876543&From=%2B15551234567&Body=Hello+from+Acteon%21
```

Twilio also uses HTTP Basic Authentication (`Authorization: Basic base64(sid:token)`) rather than bearer tokens or webhook URL credentials.

---

## 5. Teams: Response Parsing

Teams incoming webhooks return a non-standard response: the literal string `"1"` with `Content-Type: text/plain` on success (HTTP 200). This is not JSON.

The provider handles this by reading the response as text and wrapping it:

```rust
// Teams returns literal "1" with HTTP 200 on success (not JSON).
let response_text = response.text().await.unwrap_or_default();

let response_body = serde_json::json!({
    "ok": true,
    "response": response_text,
});
```

The Teams provider supports two message formats:

1. **MessageCard** (`text` field): Builds an Office 365 MessageCard with `@type: "MessageCard"` and `@context: "https://schema.org/extensions"`. Optional `title`, `summary`, and `theme_color`.

2. **Adaptive Card** (`adaptive_card` field): Wraps the provided JSON in a Teams attachment envelope:

```json
{
  "type": "message",
  "attachments": [{
    "contentType": "application/vnd.microsoft.card.adaptive",
    "content": { ... adaptive card JSON ... }
  }]
}
```

At least one of `text` or `adaptive_card` must be present. If `adaptive_card` is provided, it takes precedence.

---

## 6. Discord: 204 vs 200 Response Handling

Discord webhook behavior depends on the `?wait=true` query parameter:

- **Without `?wait=true`** (default): Returns HTTP 204 No Content with an empty body
- **With `?wait=true`**: Returns HTTP 200 with a JSON body containing the created message object

The provider handles both cases:

```rust
let response_body = if status == reqwest::StatusCode::NO_CONTENT {
    serde_json::json!({ "ok": true })
} else {
    match response.json::<DiscordWebhookResponse>().await {
        Ok(resp) => serde_json::json!({
            "ok": true,
            "id": resp.id,
            "channel_id": resp.channel_id,
        }),
        Err(_) => serde_json::json!({ "ok": true }),
    }
};
```

The `DiscordConfig.wait` field controls whether `?wait=true` is appended to the webhook URL. When enabled, the response includes the message `id` and `channel_id`, which can be useful for message tracking or threading.

The `effective_url()` method handles URL construction, including the edge case where the webhook URL already contains query parameters:

```rust
fn effective_url(&self) -> String {
    if self.config.wait {
        if self.config.webhook_url.contains('?') {
            format!("{}&wait=true", self.config.webhook_url)
        } else {
            format!("{}?wait=true", self.config.webhook_url)
        }
    } else {
        self.config.webhook_url.clone()
    }
}
```

### Discord Health Check

Discord supports `GET` on webhook URLs, which returns the webhook metadata (type, id, name, channel_id) without posting a message. This makes the health check non-intrusive -- no messages are sent to the channel.

This contrasts with Teams, where the only way to verify the webhook is to actually send a message.

---

## 7. Module / File Layout

### Crate Structure

```
crates/integrations/twilio/
  Cargo.toml
  src/
    config.rs     -- TwilioConfig (account_sid, auth_token, from_number, api_base_url)
    error.rs      -- TwilioError enum + From<TwilioError> for ProviderError
    provider.rs   -- TwilioProvider: Provider impl, MessagePayload, send_message()
    types.rs      -- TwilioSendMessageRequest (form-encoded), TwilioApiResponse
    lib.rs        -- pub mod + re-exports

crates/integrations/teams/
  Cargo.toml
  src/
    config.rs     -- TeamsConfig (webhook_url)
    error.rs      -- TeamsError enum + From<TeamsError> for ProviderError
    provider.rs   -- TeamsProvider: Provider impl, MessagePayload, build_body()
    types.rs      -- TeamsMessageCard (Office 365 MessageCard schema)
    lib.rs        -- pub mod + re-exports

crates/integrations/discord/
  Cargo.toml
  src/
    config.rs     -- DiscordConfig (webhook_url, wait, default_username, default_avatar_url)
    error.rs      -- DiscordError enum + From<DiscordError> for ProviderError
    provider.rs   -- DiscordProvider: Provider impl, MessagePayload, effective_url()
    types.rs      -- DiscordWebhookRequest, DiscordEmbed, DiscordWebhookResponse
    lib.rs        -- pub mod + re-exports
```

### Server Integration

The server config (`crates/server/src/config.rs`) uses a flat `ProviderConfig` struct with optional fields for all provider types. The `provider_type` field (`"twilio"`, `"teams"`, `"discord"`) determines which fields are required.

---

## 8. Design Decisions

| Decision | Alternative | Rationale |
|----------|-------------|-----------|
| Separate crate per provider | Single `integrations` crate | Independent compilation, feature-gated dependencies, clear ownership |
| Local error enum per provider | Reuse `ProviderError` directly | Provider-specific error messages, clean `From` conversion boundary |
| Form encoding for Twilio | JSON (rejected by Twilio API) | Twilio's REST API mandates `x-www-form-urlencoded` |
| `text()` parsing for Teams | `json()` parsing (would fail on `"1"`) | Teams returns non-JSON success response |
| GET health check for Discord | POST minimal message (like Teams) | Non-intrusive -- no channel messages during health checks |
| POST health check for Teams | None available | Teams webhooks have no read-only endpoint |
| `reqwest::Client` per provider | Shared global client | Provider-specific timeouts and connection pool isolation |
