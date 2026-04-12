# Pushover Provider

!!! note "Opt-in feature flag"
    Pushover is not compiled into the default `acteon-server` build. Enable it with `cargo build -p acteon-server --features pushover`, or use `--features extras-alerting` to enable all opt-in messaging providers at once.

Acteon ships with a first-class **Pushover** provider that sends push notifications to mobile devices via the [Pushover Messages API][api]. Operators use it for any workflow that needs to reach a human phone or desktop — on-call paging, deployment notifications, approval prompts, scheduled reminders, and anything else that fits the single-shot "send a notification" shape of the Pushover API.

[api]: https://pushover.net/api

Pushover is the lowest-ceremony receiver in the set: it has no lifecycle (just fire-and-forget sends) and no client-side deduplication, so the provider is deliberately thin. Like the other native providers, `acteon-pushover`:

- Holds the application token and every user/group key as `SecretString`, zeroized on drop.
- Supports multiple recipients per provider instance so one config can fan notifications out to several Pushover users or groups.
- Maps 5xx / 408 → retryable `Connection` (via a `Transient` variant); 429 → retryable `RateLimited`; 401/403 → non-retryable `Configuration`; other 4xx → non-retryable `ExecutionFailed`.
- Handles the Pushover-specific quirk of **200 OK with `status: 0`** — a successful HTTP round-trip with a Pushover-layer failure in the body. The provider classifies those as permanent `ExecutionFailed` so rules don't silently succeed when the API actually rejected the call.
- Reuses the server's shared HTTP client, so it participates in circuit breaking, provider health checks, and per-provider metrics automatically.
- Propagates W3C Trace Context headers.

## TOML configuration

Pushover uses Acteon's **nested provider config** pattern. Every Pushover-specific setting lives under a `pushover.*` key.

```toml
[[providers]]
name = "pushover-ops"
type = "pushover"
pushover.app_token = "ENC[AES256_GCM,data:abc123...]"
pushover.default_recipient = "ops-oncall"

[providers.pushover.recipients]
ops-oncall = "ENC[AES256_GCM,data:def456...]"  # a user key (U...) or group key (G...)
dev-team   = "ENC[AES256_GCM,data:ghi789...]"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used when dispatching actions |
| `type` | Yes | Must be `"pushover"` |
| `pushover.app_token` | Yes | Application token (the `T...` key). Supports `ENC[...]`. |
| `pushover.recipients` | Yes (≥1) | Map of logical recipient name → user or group key (`U...` / `G...`). Values support `ENC[...]`. |
| `pushover.default_recipient` | No | Name of the default recipient used when the dispatch payload omits `user_key`. If there is only one recipient, it is used implicitly. |
| `pushover.api_base_url` | No | Override the Messages API base URL. Tests only. |

## Payload shape

Pushover accepts one `event_action`, `"send"` (also the default when omitted). The only required payload field is `message`; everything else is optional.

### Normal-priority notification

```json
{
  "message": "Deploy #4823 completed successfully.",
  "title": "CI/CD — production",
  "priority": 0,
  "url": "https://ci.example.com/build/4823",
  "url_title": "View build"
}
```

### Emergency-priority alert (requires `retry` + `expire`)

Emergency notifications bypass the recipient's quiet hours **and** require acknowledgment. The server re-notifies the user every `retry` seconds until they tap the notification, or `expire` seconds elapse:

```json
{
  "message": "Checkout API returning 5xx for >50% of traffic.",
  "title": "CRITICAL: checkout-api down",
  "priority": 2,
  "retry": 60,
  "expire": 3600,
  "sound": "siren",
  "url": "https://wiki.example.com/runbook/checkout-5xx",
  "url_title": "Open runbook"
}
```

| Field | Type | Notes |
|-------|------|-------|
| `message` | string | **Required.** Body of the notification. Truncated client-side to 1024 UTF-8 bytes (the API cap) — multi-byte characters are never split. |
| `title` | string | Title shown above the message. Truncated to 250 bytes. |
| `user_key` | string | Logical recipient name (matching a key in `pushover.recipients`). Falls back to `pushover.default_recipient` or the single-entry implicit default. |
| `priority` | int | `-2..=2`. `2` is emergency and requires `retry` + `expire`. |
| `retry` | int (seconds) | Seconds between re-notifications for emergency priority. **Minimum 30.** |
| `expire` | int (seconds) | Seconds until the server gives up re-notifying. **Maximum 10800** (3 hours). |
| `sound` | string | Notification sound name. See the [Pushover sounds list](https://pushover.net/api#sounds). |
| `url` | string | Supplementary URL. Truncated to 512 bytes. |
| `url_title` | string | Label for `url`. Truncated to 100 bytes. |
| `device` | string | Deliver to a specific device name only. |
| `html` | bool | Render message as HTML. Mutually exclusive with `monospace`. |
| `monospace` | bool | Render message as monospace. Mutually exclusive with `html`. |
| `timestamp` | int | Unix timestamp of the originating event. |
| `ttl` | int | Auto-delete after N seconds. |

### Client-side validation

The provider rejects obviously-broken payloads at build time instead of letting the Pushover server return an error:

- `priority = 2` without both `retry` and `expire` → `Serialization` error.
- `retry < 30` or `expire > 10800` → `Serialization` error.
- `html = true` **and** `monospace = true` → `Serialization` error.
- `priority` outside `-2..=2` → `Serialization` error.
- Unknown `user_key` → `Configuration` error.

All of these are non-retryable — retrying a malformed payload will never succeed.

## Rule integration

Because Pushover is just another named provider, every routing primitive Acteon already has works with it:

- **Reroute high-priority events** to Pushover by matching on `action.payload.priority >= 1` with a `reroute` rule.
- **Silence maintenance windows** with [silences](silences.md) — silences apply before the provider dispatch, so a Pushover notification never leaves the gateway during an active silence.
- **Quota-bound a Pushover account** with a [per-provider tenant quota](tenant-quotas.md) scoped to `provider: "pushover-ops"` — useful if you're on the free 10k-messages/month tier and need to enforce monthly caps.
- **Dedup noisy notifications** with Acteon's [deduplication](deduplication.md) using `Action.dedup_key` since Pushover itself has no server-side dedup.

## Outcome body

On success the provider returns an `Executed` outcome whose `body` carries the Pushover response:

```json
{
  "status": 1,
  "request": "a7b3e0c2-...",
  "receipt": "u1iycscu4e..."
}
```

The `request` field is the Pushover server request ID (useful for support tickets). The `receipt` field is only populated for **emergency-priority** notifications — you can poll the [Pushover receipts endpoint](https://pushover.net/api/receipts) with it to check whether the user has acknowledged the alert.

## Error mapping

| HTTP status | Pushover `status` | `ProviderError` | Retryable? |
|-------------|-------------------|-----------------|------------|
| 2xx | 1 | `Executed` (success) | — |
| 2xx | 0 | `ExecutionFailed(...)` | No — permanent API error in the body |
| 401 / 403 | — | `Configuration(...)` | No |
| 429 | — | `RateLimited` | Yes |
| 408, 5xx | — | `Connection(...)` (via `Transient`) | **Yes** — brief Pushover outages re-queue |
| Other 4xx | — | `ExecutionFailed(...)` | No |
| Transport failure | — | `Connection(...)` | Yes |

## Simulation example

A full demo — normal priority, emergency priority, and a rule-based reroute — is in `crates/simulation/examples/pushover_simulation.rs`:

```bash
cargo run -p acteon-simulation --example pushover_simulation
```

The simulation uses a recording provider, so it runs offline with no real Pushover credentials.
