# VictorOps / Splunk On-Call Provider

Acteon ships with a first-class **VictorOps** (now Splunk On-Call) provider that posts alerts to the [REST endpoint integration][integration] — the same endpoint Alertmanager targets via its `victorops_configs`. It was built as part of Phase 4b of the Alertmanager feature-parity initiative so ops teams migrating off Alertmanager can reuse their existing VictorOps routing without rewriting runbooks.

[integration]: https://help.victorops.com/knowledge-base/rest-endpoint-integration-guide/

Like Acteon's other native providers, `acteon-victorops`:

- Supports multiple routing keys per provider instance so one config can fan alerts out to several VictorOps teams.
- Stores both the organization API key and every routing key as `SecretString`, zeroizing the plaintexts on drop.
- Auto-scopes `entity_id` with `{namespace}:{tenant}:` for multi-tenant isolation on shared integration keys (opt-out available).
- Propagates W3C Trace Context (`traceparent`/`tracestate`) headers.
- Maps 5xx / 408 → retryable `Connection`; 429 → retryable `RateLimited`; 401/403 → non-retryable `Configuration`; other 4xx → non-retryable `ExecutionFailed`.
- Reuses the server's shared HTTP client, so it participates in circuit breaking, provider health checks, and per-provider metrics automatically.

## TOML configuration

VictorOps uses Acteon's **nested provider config** pattern: every VictorOps-specific setting lives under a `victorops.*` key rather than at the top level of the `[[providers]]` entry.

```toml
[[providers]]
name = "victorops-prod"
type = "victorops"
victorops.api_key = "ENC[AES256_GCM,data:abc123...]"
victorops.default_route = "team-ops"
victorops.monitoring_tool = "acteon"     # default
# victorops.scope_entity_ids = true      # default — see "Multi-tenant isolation" below
# victorops.api_base_url = "..."         # testing only

[providers.victorops.routes]
team-ops   = "ENC[AES256_GCM,data:def456...]"
team-infra = "ENC[AES256_GCM,data:ghi789...]"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used when dispatching actions |
| `type` | Yes | Must be `"victorops"` |
| `victorops.api_key` | Yes | Organization-level REST integration key. Supports `ENC[...]`. |
| `victorops.routes` | Yes (≥1) | Map of logical route name → per-route routing key. Values support `ENC[...]`. |
| `victorops.default_route` | No | Name of the default route used when the dispatch payload omits `routing_key`. If there is only one route, it is used implicitly. |
| `victorops.monitoring_tool` | No | Value reported in the alert body's `monitoring_tool` field. Defaults to `"acteon"`. |
| `victorops.scope_entity_ids` | No | Whether to auto-prefix `entity_id` with `{namespace}:{tenant}:` for multi-tenant isolation. Defaults to `true`. See below. |
| `victorops.api_base_url` | No | Override the REST endpoint base URL. Tests only — do not set in production. |

Both the `api_key` and every value in `routes` get embedded as **URL path segments** in the final request (`POST /integrations/generic/20131114/alert/{api_key}/{routing_key}`). Both segments are percent-encoded using the `percent-encoding` crate so a stray character in either cannot inject an extra path component.

## Multi-tenant isolation

Alerts dispatched by Acteon come from `(namespace, tenant)` scopes, but VictorOps has no native tenant concept. On a shared integration key (common in large orgs), that means two tenants that both pick the raw `entity_id` `web-01-high-cpu` would otherwise collide — and Tenant A could resolve Tenant B's incident simply by guessing the `entity_id`.

**By default** (`victorops.scope_entity_ids = true`, which is the default), the provider rewrites every `entity_id` to `{namespace}:{tenant}:{raw_entity_id}` before sending it to VictorOps. The prefix is applied identically on `trigger`, `acknowledge`, and `resolve` so all three map to the same VictorOps incident, but two tenants can never refer to each other's alerts.

Set `victorops.scope_entity_ids = false` only if:

1. Every Acteon namespace/tenant has its own dedicated VictorOps integration key, **or**
2. You genuinely need cross-tenant `entity_id` coordination (e.g., a platform team resolving a customer alert from a shared runbook).

## Payload shape

Dispatch actions target the provider by name and carry an `event_action` in the payload. The provider maps `event_action` to a VictorOps `message_type`:

| `event_action` | VictorOps `message_type` | Purpose |
|---|---|---|
| `"trigger"` | `CRITICAL` | Firing alert — pages the on-call |
| `"warn"` | `WARNING` | Lower-priority alert, visible but does not page |
| `"info"` | `INFO` | Informational — does not page |
| `"acknowledge"` | `ACKNOWLEDGEMENT` | Oncall picked up the incident |
| `"resolve"` | `RECOVERY` | Incident resolved |

### Trigger

```json
{
  "event_action": "trigger",
  "entity_id": "checkout-api-5xx",
  "entity_display_name": "Checkout API 5xx rate above SLO",
  "state_message": "5xx rate crossed SLO threshold for 5 minutes.",
  "host_name": "checkout-api",
  "routing_key": "team-ops",
  "state_start_time": 1713897600
}
```

| Field | Type | Notes |
|-------|------|-------|
| `event_action` | string | **Required.** See the message-type table above. |
| `entity_id` | string | Deduplication key. **Required for `acknowledge` / `resolve`** so the lifecycle events correlate with the original trigger. Optional on `trigger` / `warn` / `info` (VictorOps will auto-assign). |
| `entity_display_name` | string | Short human-readable title. |
| `state_message` | string | Long-form body. |
| `host_name` | string | Domain entity the alert is about. |
| `routing_key` | string | Logical route name (matching a key in `victorops.routes`). Falls back to `victorops.default_route` or the single-entry implicit default. |
| `state_start_time` | int | Unix timestamp (seconds) of when the alerting condition started. |
| `monitoring_tool` | string | Overrides the provider's configured default. |

### Acknowledge

```json
{
  "event_action": "acknowledge",
  "entity_id": "checkout-api-5xx",
  "state_message": "Investigating — rolling back deploy #4823."
}
```

### Resolve

```json
{
  "event_action": "resolve",
  "entity_id": "checkout-api-5xx",
  "state_message": "Rollback confirmed; error rate back below threshold."
}
```

## Rule integration

Because VictorOps is just another named provider, every routing primitive Acteon already has works with it:

- **Reroute critical alerts** to the VictorOps integration by matching on `action.payload.severity == "critical"` with a `reroute` rule.
- **Silence maintenance windows** with [silences](silences.md) — silences apply before the provider dispatch, so a VictorOps alert never leaves the gateway during an active silence.
- **Quota-bound a VictorOps account** via a [per-provider tenant quota](tenant-quotas.md) scoped to `provider: "victorops-prod"` to cap burst traffic.
- **Dedup noisy alerts** with Acteon's [deduplication](deduplication.md) using the VictorOps `entity_id` as the dedup key — VictorOps then collapses the incident server-side through its own entity-id correlation.

## Outcome body

On success the provider returns an `Executed` outcome whose `body` carries the VictorOps response:

```json
{
  "result": "success",
  "entity_id": "incidents:tenant-1:checkout-api-5xx"
}
```

The returned `entity_id` is the **scoped** form that the provider actually sent (i.e., it includes the `{namespace}:{tenant}:` prefix when `scope_entity_ids` is on). Operators resolving incidents through the VictorOps UI should reference this exact value.

## Error mapping

| HTTP status | `ProviderError` | Retryable? |
|-------------|-----------------|------------|
| 2xx | `Executed` (success) | — |
| 401 / 403 | `Configuration(...)` | No |
| 429 | `RateLimited` | Yes |
| 408, 5xx | `Connection(...)` (via `Transient`) | **Yes** — a brief VictorOps outage re-queues the alert |
| Other 4xx | `ExecutionFailed(...)` | No |
| Transport failure | `Connection(...)` | Yes |

Invalid payloads (missing `entity_id` on ack/resolve, unknown `event_action`, unknown routing key) map to `Serialization` / `Configuration` and are **not** retryable.

## Simulation example

A full end-to-end demo that walks through the `trigger → acknowledge → resolve` lifecycle plus a rule-based critical reroute is in `crates/simulation/examples/victorops_simulation.rs`:

```bash
cargo run -p acteon-simulation --example victorops_simulation
```

The simulation uses a recording provider, so it runs offline with no real VictorOps credentials.
