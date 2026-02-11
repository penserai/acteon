# Tenant Usage Quotas

Tenant usage quotas let you enforce per-tenant limits on the number of actions dispatched within a rolling time window. This is useful for:

- **Billing enforcement** -- cap actions per tenant to match their subscription tier
- **Abuse prevention** -- block runaway automation that floods the pipeline
- **Fair usage** -- ensure no single tenant monopolizes shared infrastructure
- **Cost control** -- degrade to a cheaper provider when a tenant exceeds their budget

## How It Works

Quota checks run in the gateway dispatch pipeline **after** the distributed lock is acquired but **before** rule evaluation. This means quotas take precedence over all rules -- an action that exceeds its tenant's quota is rejected (or warned/degraded) regardless of which rules would have matched.

```
Dispatch Pipeline:
  1. Acquire distributed lock
  2. *** Quota check ***  <-- here
  3. Rule evaluation
  4. LLM guardrail
  5. Execute / suppress / reroute / ...
```

Each quota policy defines:

- A **tenant** and **namespace** scope
- A **maximum number of actions** per **time window**
- An **overage behavior** that determines what happens when the limit is exceeded

Usage counters are stored in the state backend with epoch-aligned windows, so all gateway instances agree on window boundaries without coordination.

## Configuration

### Via the Gateway Builder (Rust)

```rust
use acteon_core::{QuotaPolicy, QuotaWindow, OverageBehavior};
use acteon_gateway::GatewayBuilder;

let gateway = GatewayBuilder::new()
    .state(state)
    .lock(lock)
    .quota_policy(QuotaPolicy {
        id: "q-001".into(),
        namespace: "notifications".into(),
        tenant: "acme".into(),
        max_actions: 1000,
        window: QuotaWindow::Daily,
        overage_behavior: OverageBehavior::Block,
        enabled: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        description: Some("Acme daily limit".into()),
        labels: Default::default(),
    })
    .build()?;
```

### Via the REST API

Create, read, update, and delete quota policies through the `/v1/quotas` endpoints. See the [API Reference](#api-reference) below.

### Via TOML Configuration

```toml
[[quotas]]
id = "q-acme-daily"
namespace = "notifications"
tenant = "acme"
max_actions = 1000
window = "daily"
overage_behavior = "block"
enabled = true
description = "Acme daily limit"
```

## Quota Windows

| Window | Duration | Description |
|--------|----------|-------------|
| `hourly` | 1 hour | Rolling 3,600-second window |
| `daily` | 24 hours | Rolling 86,400-second window |
| `weekly` | 7 days | Rolling 604,800-second window |
| `monthly` | 30 days | Rolling 2,592,000-second window |
| `custom` | N seconds | Arbitrary window duration |

All windows are **epoch-aligned**, meaning the window start is computed as `floor(unix_timestamp / window_seconds) * window_seconds`. This ensures that all gateway instances agree on when a window starts and ends without any coordination.

### Custom Windows

For non-standard billing periods, use the `custom` window with a duration in seconds:

```json
{
  "window": {"custom": {"seconds": 7200}}
}
```

This creates a rolling 2-hour window.

## Overage Behaviors

When a tenant's usage reaches the configured `max_actions` limit, the `overage_behavior` determines what happens next.

### Block

The action is rejected immediately. The gateway returns `ActionOutcome::QuotaExceeded` and the counter is **not** incremented (the rejected action does not count toward usage).

```json
{
  "overage_behavior": "block"
}
```

**Outcome:** `QuotaExceeded { tenant, limit, used, overage_behavior: "block" }`

### Warn

The action is allowed to proceed. The counter is incremented past the limit. The gateway emits a warning log and increments the `quota_warned` metric.

```json
{
  "overage_behavior": "warn"
}
```

This is useful for soft limits where you want visibility into overages without disrupting tenants.

### Degrade

The action is rejected with a `QuotaExceeded` outcome that includes the fallback provider name. The caller (or a middleware) can use this to re-route the action to a cheaper or lower-priority provider.

```json
{
  "overage_behavior": {"degrade": {"fallback_provider": "log"}}
}
```

**Outcome:** `QuotaExceeded { ..., overage_behavior: "degrade:log" }`

### Notify

The action is allowed to proceed. The gateway increments the counter and sends a notification to the configured target (e.g., an email address or webhook URL).

```json
{
  "overage_behavior": {"notify": {"target": "admin@example.com"}}
}
```

## API Reference

All endpoints live under `/v1/quotas`. Namespace and tenant are provided as query parameters.

### `POST /v1/quotas` -- Create

Create a new quota policy.

**Request body:**

```json
{
  "namespace": "notifications",
  "tenant": "acme",
  "max_actions": 1000,
  "window": "daily",
  "overage_behavior": "block",
  "enabled": true,
  "description": "Acme daily limit",
  "labels": {"tier": "premium"}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `namespace` | string | Yes | Namespace scope |
| `tenant` | string | Yes | Tenant scope |
| `max_actions` | integer | Yes | Maximum actions per window |
| `window` | string/object | Yes | `"hourly"`, `"daily"`, `"weekly"`, `"monthly"`, or `{"custom": {"seconds": N}}` |
| `overage_behavior` | string/object | Yes | `"block"`, `"warn"`, `{"degrade": {"fallback_provider": "..."}}`, or `{"notify": {"target": "..."}}` |
| `enabled` | bool | No | Whether the policy is active (default: `true`) |
| `description` | string | No | Human-readable description |
| `labels` | object | No | Arbitrary key-value labels |

**Response (201):**

```json
{
  "id": "q-019462a1-...",
  "namespace": "notifications",
  "tenant": "acme",
  "max_actions": 1000,
  "window": "daily",
  "overage_behavior": "block",
  "enabled": true,
  "created_at": "2026-02-10T12:00:00Z",
  "updated_at": "2026-02-10T12:00:00Z"
}
```

### `GET /v1/quotas` -- List

List all quota policies, optionally filtered by namespace and tenant.

**Query parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `namespace` | string | No | Filter by namespace |
| `tenant` | string | No | Filter by tenant |

**Response (200):**

```json
{
  "quotas": [
    {
      "id": "q-019462a1-...",
      "namespace": "notifications",
      "tenant": "acme",
      "max_actions": 1000,
      "window": "daily",
      "overage_behavior": "block",
      "enabled": true,
      "description": "Acme daily limit",
      "created_at": "2026-02-10T12:00:00Z",
      "labels": {"tier": "premium"}
    }
  ]
}
```

### `GET /v1/quotas/{id}` -- Get Detail

Retrieve the full definition of a quota policy.

**Query parameters:** `namespace`, `tenant`

**Response (200):** Full `QuotaPolicy` object.

**Response (404):** `{"error": "quota policy not found"}`

### `PUT /v1/quotas/{id}` -- Update

Update an existing quota policy. Only provided fields are changed.

**Query parameters:** `namespace`, `tenant`

**Request body (partial):**

```json
{
  "max_actions": 2000,
  "description": "Upgraded to premium tier"
}
```

Updatable fields: `max_actions`, `window`, `overage_behavior`, `enabled`, `description`, `labels`.

**Response (200):** Updated `QuotaPolicy` object.

### `DELETE /v1/quotas/{id}` -- Delete

Permanently delete a quota policy. The usage counter is also cleaned up.

**Query parameters:** `namespace`, `tenant`

**Response (204):** No content.

### `GET /v1/quotas/{id}/usage` -- Get Usage

Retrieve the current usage for a quota policy within the active window.

**Query parameters:** `namespace`, `tenant`

**Response (200):**

```json
{
  "tenant": "acme",
  "namespace": "notifications",
  "used": 742,
  "limit": 1000,
  "remaining": 258,
  "window": "daily",
  "resets_at": "2026-02-11T00:00:00Z",
  "overage_behavior": "block"
}
```

## Usage Examples

### Create a quota policy

```bash
curl -X POST http://localhost:8080/v1/quotas \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "acme",
    "max_actions": 1000,
    "window": "daily",
    "overage_behavior": "block",
    "description": "Acme daily notification limit"
  }'
```

### Check current usage

```bash
curl "http://localhost:8080/v1/quotas/q-001/usage?namespace=notifications&tenant=acme"
```

### Update limit (upgrade tier)

```bash
curl -X PUT "http://localhost:8080/v1/quotas/q-001?namespace=notifications&tenant=acme" \
  -H "Content-Type: application/json" \
  -d '{"max_actions": 5000}'
```

### Disable a quota temporarily

```bash
curl -X PUT "http://localhost:8080/v1/quotas/q-001?namespace=notifications&tenant=acme" \
  -H "Content-Type: application/json" \
  -d '{"enabled": false}'
```

### Delete a quota

```bash
curl -X DELETE "http://localhost:8080/v1/quotas/q-001?namespace=notifications&tenant=acme"
```

## Monitoring

### Prometheus Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `quota_exceeded` | Counter | Actions blocked by quota (Block behavior) |
| `quota_warned` | Counter | Actions that exceeded quota but were allowed (Warn behavior) |
| `quota_degraded` | Counter | Actions degraded to a fallback provider (Degrade behavior) |

### Structured Logging

| Event | Level | Description |
|-------|-------|-------------|
| Quota exceeded (block) | info | `quota exceeded — blocking action` with tenant, limit, used |
| Quota exceeded (warn) | warn | `quota exceeded — warning, allowing action` with tenant, limit, used |
| Quota exceeded (degrade) | info | `quota exceeded — degrading to fallback provider` with tenant, fallback |
| Quota exceeded (notify) | info | `quota exceeded — notifying target` with tenant, target |

## Best Practices

- **Start with Warn**: Use Warn behavior initially to understand usage patterns before switching to Block.
- **Set meaningful descriptions**: Always include a `description` so quota policies are easy to identify in the UI and logs.
- **Use labels for tier management**: Labels like `tier: premium` or `plan: enterprise` make it easy to filter and bulk-update policies.
- **Monitor the `quota_warned` metric**: A rising warning count may indicate a tenant needs an upgrade or that limits need adjustment.
- **Prefer daily or hourly windows**: These align with natural billing cycles and are easier to reason about than custom windows.
- **Disable before deleting**: Disable a policy first to verify there are no unintended side effects before permanently removing it.

## Limitations

- **Single policy per namespace:tenant**: Each tenant can have at most one quota policy per namespace. Multiple overlapping policies for the same scope are not supported.
- **Counter precision**: Counters are stored as strings in the state backend and incremented non-atomically (read + write). In extremely high-throughput scenarios, a small number of actions may slip through just above the limit.
- **No per-action-type quotas**: Quotas apply to all action types within a namespace:tenant scope. Use rules for action-type-level control.
- **Window granularity**: The minimum effective window is determined by the state backend's TTL precision (typically 1 second).
