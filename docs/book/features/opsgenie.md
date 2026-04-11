# OpsGenie Provider

Acteon ships with a first-class **OpsGenie** provider that creates, acknowledges, and closes alerts through the [OpsGenie Alert API v2](https://docs.opsgenie.com/docs/alert-api). It was built as part of the Alertmanager feature-parity initiative so ops teams migrating off Alertmanager can keep their existing OpsGenie runbooks — incident aliases, responder routing, priorities, and tags — without rewriting them.

Like Acteon's other native providers, `acteon-opsgenie`:

- Supports both US (`api.opsgenie.com`) and EU (`api.eu.opsgenie.com`) data residency regions.
- Accepts `ENC[...]` encrypted API keys in TOML configuration.
- Propagates W3C Trace Context (`traceparent`/`tracestate`) headers to downstream API calls.
- Maps HTTP 429 → retryable `RateLimited`; 401/403 → non-retryable `Configuration`.
- Uses the server's shared HTTP client, so it participates in circuit breaking, provider health checks, and per-provider metrics out of the box.

## TOML configuration

OpsGenie is the first provider to use Acteon's **nested provider config** pattern: every OpsGenie-specific setting lives under an `opsgenie.*` key rather than a flat `opsgenie_*` prefix at the top level. This keeps the top-level `[[providers]]` schema tractable as more providers land.

```toml
[[providers]]
name = "opsgenie-prod"
type = "opsgenie"
opsgenie.api_key = "ENC[AES256_GCM,data:abc123...]"
opsgenie.region = "us"                   # or "eu"
opsgenie.default_team = "platform-oncall"
opsgenie.default_priority = "P3"
opsgenie.default_source = "acteon"
# opsgenie.scope_aliases = true          # default — see "Multi-tenant isolation" below
# opsgenie.message_max_length = 130      # default — raise if OpsGenie lifts the cap
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used when dispatching actions |
| `type` | Yes | Must be `"opsgenie"` |
| `opsgenie.api_key` | Yes | Integration API key (the value that becomes `Authorization: GenieKey {key}`). Supports `ENC[...]` for encrypted storage. The plaintext is wrapped in a `SecretString` so it is zeroized on drop. |
| `opsgenie.region` | No | `"us"` (default) or `"eu"`. Accounts are pinned to one region at provisioning; picking the wrong one produces 401/403. |
| `opsgenie.default_team` | No | Team responder used when a payload omits `responders`. |
| `opsgenie.default_priority` | No | Default alert priority (`P1`..=`P5`). Falls back to `P3`. |
| `opsgenie.default_source` | No | Default `source` label shown on the alert UI. |
| `opsgenie.scope_aliases` | No | Whether to auto-prefix user-supplied aliases with `{namespace}:{tenant}:` for multi-tenant isolation. Defaults to `true`. See below. |
| `opsgenie.message_max_length` | No | Client-side `message` truncation cap. Defaults to 130 (the current OpsGenie API limit). |
| `opsgenie.api_base_url` | No | Override the API base URL. Tests only — do not set in production. |

## Multi-tenant isolation

Alerts dispatched by Acteon come from `(namespace, tenant)` scopes, but OpsGenie has no native tenant concept. On a shared `OpsGenie` integration key (common in large orgs), that means two tenants that both pick the raw alias `web-01-high-cpu` would otherwise collide — and Tenant A could close Tenant B's alert simply by guessing the alias.

**By default** (`opsgenie.scope_aliases = true`, which is the default), the provider rewrites every alias to `{namespace}:{tenant}:{raw_alias}` before sending it to OpsGenie. The prefix is applied identically on `create`, `acknowledge`, and `close` so all three resolve to the same incident, but two tenants can never refer to each other's alerts.

Set `opsgenie.scope_aliases = false` only if:

1. Every Acteon namespace/tenant has its own dedicated OpsGenie integration key, **or**
2. You genuinely need cross-tenant alias coordination (e.g., a platform team closing a customer alert from a shared runbook).

## Payload shape

Dispatch actions target the provider by name and carry an `event_action` in the payload that tells the provider which Alert API endpoint to hit:

| `event_action` | Endpoint | Required fields |
|---|---|---|
| `"create"` | `POST /v2/alerts` | `message` |
| `"acknowledge"` | `POST /v2/alerts/{alias}/acknowledge?identifierType=alias` | `alias` |
| `"close"` | `POST /v2/alerts/{alias}/close?identifierType=alias` | `alias` |

### Create

```json
{
  "event_action": "create",
  "message": "High error rate on checkout-api",
  "alias": "checkout-api-5xx",
  "description": "5xx rate crossed SLO threshold for 5 minutes.",
  "priority": "P2",
  "tags": ["checkout", "5xx", "slo-breach"],
  "responders": [
    { "name": "checkout-oncall", "type": "team" }
  ],
  "details": {
    "runbook": "https://wiki.example.com/runbook/checkout-5xx",
    "service": "checkout-api",
    "env": "production"
  },
  "source": "prometheus"
}
```

| Field | Type | Notes |
|-------|------|-------|
| `message` | string | **Required.** Short alert title. Truncated client-side to 130 characters (the API's max). |
| `alias` | string | Client-side deduplication key. Later `acknowledge`/`close` events use the same alias to target the same incident. |
| `description` | string | Long-form description shown in the alert detail view. |
| `priority` | string | `"P1"`..`"P5"`. Falls back to `opsgenie_default_priority`. |
| `responders` | array | List of responder objects (`{name, type}` or `{id, type}`). Falls back to a single-element list built from `opsgenie_default_team` when omitted. |
| `visible_to` | array | Same shape as `responders`. Controls who can view the alert. |
| `actions` | string[] | Pre-defined actions (e.g. `["ping", "reboot"]`). |
| `tags` | string[] | Free-form tags used for downstream routing. |
| `details` | object | Arbitrary key-value metadata shown in the alert UI. |
| `entity` | string | Domain entity the alert is about (e.g. `"web-01"`). |
| `source` | string | Source label. Falls back to `opsgenie_default_source`. |
| `user` | string | Username to attribute the creation to. |
| `note` | string | Operator note attached to the alert. |

### Acknowledge

```json
{
  "event_action": "acknowledge",
  "alias": "checkout-api-5xx",
  "user": "oncall-alice",
  "note": "Investigating — rolled back deploy #4823"
}
```

### Close

```json
{
  "event_action": "close",
  "alias": "checkout-api-5xx",
  "user": "oncall-alice",
  "note": "Rollback confirmed; 5xx rate back below threshold."
}
```

Both `acknowledge` and `close` require `alias` because the provider always looks up alerts by alias (`identifierType=alias`), not by the numeric alert ID. This keeps the create/ack/close sequence stable across process restarts and across alert-correlation chains.

## Rule integration

Because OpsGenie is just another named provider, every routing primitive Acteon already has works with it:

- **Reroute high-priority alerts to OpsGenie:**
  ```yaml
  rules:
    - name: reroute-p1-to-opsgenie
      priority: 1
      condition:
        field: action.payload.priority
        eq: "P1"
      action:
        type: reroute
        target_provider: opsgenie-prod
  ```
- **Dedup noisy alerts:** use Acteon's [deduplication](deduplication.md) with the alert's `alias` as the dedup key — OpsGenie then collapses the incident server-side through its own alias mechanism.
- **Silence maintenance-window alerts:** [silences](silences.md) apply before the provider dispatch, so an OpsGenie dispatch never leaves the gateway during an active silence.
- **Quota-bound an OpsGenie account:** add a [per-provider tenant quota](tenant-quotas.md) scoped to `provider: "opsgenie-prod"` to cap burst traffic independent of tenant-wide quotas.

## Outcome body

On success the provider returns an `Executed` outcome whose `body` carries the `OpsGenie` response:

```json
{
  "result": "Request will be processed",
  "took": 0.123,
  "request_id": "abc-1234-..."
}
```

The `request_id` is the handle you can use to poll the OpsGenie request-status endpoint if you need to confirm the eventual alert ID. Acteon treats the initial 202 Accepted as success (alert creation is asynchronous in OpsGenie's API).

## Error mapping

| HTTP status | `ProviderError` | Retryable? |
|-------------|-----------------|------------|
| 2xx | `Executed` (success) | — |
| 401 / 403 | `Configuration(...)` | No |
| 429 | `RateLimited` | Yes |
| 4xx (other) | `ExecutionFailed(...)` | No |
| 5xx | `ExecutionFailed(...)` | No |
| Transport failure | `Connection(...)` | Yes |

Invalid payloads (missing `message` on create, missing `alias` on ack/close, unknown `event_action`) map to `Serialization` and are **not** retryable — retrying a malformed payload never succeeds.

## Simulation example

A full end-to-end demo that walks through the `create` → `acknowledge` → `close` lifecycle plus rule-based P1 rerouting is in `crates/simulation/examples/opsgenie_simulation.rs`:

```bash
cargo run -p acteon-simulation --example opsgenie_simulation
```

The simulation uses a recording provider, so it runs offline with no real OpsGenie credentials.
