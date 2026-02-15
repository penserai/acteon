# Data Retention Policies

Data retention policies give you per-tenant control over how long audit records, completed chain state, and resolved event records are kept before automatic cleanup. This is essential for:

- **Regulatory compliance** -- GDPR, SOC2, and HIPAA require organizations to define and enforce data retention schedules
- **Cost management** -- audit and state records accumulate indefinitely without retention policies, increasing storage costs
- **Privacy by design** -- automatically purge tenant data after the business-required retention period
- **Compliance hold** -- preserve audit records indefinitely for tenants under legal or regulatory hold

## How It Works

Retention policies operate at two levels:

1. **Audit TTL resolution** -- During every dispatch, the gateway computes the effective audit TTL for the action's tenant using a three-level resolution:
   - If the tenant has a retention policy with `compliance_hold = true`, the effective TTL is `None` (records never expire)
   - If the tenant has a retention policy with `audit_ttl_seconds` set, that value is used
   - Otherwise, the gateway-wide `audit_ttl_seconds` default applies

2. **Background reaper** -- A periodic background task scans for expired state entries (completed chains, resolved events) and deletes them according to the tenant's `state_ttl_seconds` and `event_ttl_seconds`. Tenants with `compliance_hold` are skipped entirely.

```
Audit TTL Resolution Order (most specific wins):

  1. compliance_hold = true   →  None (never expires)
  2. policy.audit_ttl_seconds →  per-tenant TTL
  3. gateway.audit_ttl_seconds →  global default
```

Each retention policy defines:

- A **tenant** and **namespace** scope
- Optional **audit TTL** (seconds) overriding the gateway default
- Optional **state TTL** (seconds) for completed/failed chain records
- Optional **event TTL** (seconds) for resolved event records
- A **compliance hold** flag that prevents any expiry

## Configuration

### Via the Gateway Builder (Rust)

```rust
use acteon_core::RetentionPolicy;
use acteon_gateway::GatewayBuilder;

let gateway = GatewayBuilder::new()
    .state(state)
    .lock(lock)
    .audit_ttl_seconds(86_400) // Global default: 24 hours
    .retention_policy(RetentionPolicy {
        id: "ret-001".into(),
        namespace: "notifications".into(),
        tenant: "acme".into(),
        enabled: true,
        audit_ttl_seconds: Some(2_592_000),  // 30 days
        state_ttl_seconds: Some(604_800),     // 7 days
        event_ttl_seconds: Some(259_200),     // 3 days
        compliance_hold: false,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        description: Some("Acme: 30-day audit retention".into()),
        labels: Default::default(),
    })
    .build()?;
```

### Via the REST API

Create, read, update, and delete retention policies through the `/v1/retention` endpoints. See the [API Reference](#api-reference) below.

### Via TOML Configuration

```toml
[background]
enable_retention_reaper = true
retention_check_interval_seconds = 3600
```

The retention reaper runs on the configured interval and scans all registered retention policies.

## Compliance Hold

When `compliance_hold` is set to `true` on a retention policy, the following effects apply:

- **Audit records** never expire, regardless of `audit_ttl_seconds` or the gateway default
- **Background reaper** skips this tenant entirely (no state or event cleanup)
- The `retention_skipped_compliance` metric is incremented for each skipped entry

This is designed for regulated environments where audit records must be preserved indefinitely:

```json
{
  "namespace": "notifications",
  "tenant": "healthcare-corp",
  "compliance_hold": true,
  "description": "HIPAA compliance hold - audit records preserved indefinitely"
}
```

To release a compliance hold, update the policy:

```bash
curl -X PUT "http://localhost:8080/v1/retention/{id}" \
  -H "Content-Type: application/json" \
  -d '{"compliance_hold": false, "audit_ttl_seconds": 7776000}'
```

## API Reference

All endpoints live under `/v1/retention`.

### `POST /v1/retention` -- Create

Create a new retention policy. Only one policy per namespace:tenant pair is allowed.

**Request body:**

```json
{
  "namespace": "notifications",
  "tenant": "acme",
  "audit_ttl_seconds": 2592000,
  "state_ttl_seconds": 604800,
  "event_ttl_seconds": 259200,
  "compliance_hold": false,
  "description": "Acme 30-day retention",
  "labels": {"tier": "premium"}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `namespace` | string | Yes | Namespace scope |
| `tenant` | string | Yes | Tenant scope |
| `audit_ttl_seconds` | integer | No | Override for the global audit TTL (seconds) |
| `state_ttl_seconds` | integer | No | TTL for completed chain state records (seconds) |
| `event_ttl_seconds` | integer | No | TTL for resolved event records (seconds) |
| `compliance_hold` | bool | No | When `true`, audit records never expire (default: `false`) |
| `description` | string | No | Human-readable description |
| `labels` | object | No | Arbitrary key-value labels |

**Response (201):**

```json
{
  "id": "ret-019462a1-...",
  "namespace": "notifications",
  "tenant": "acme",
  "enabled": true,
  "audit_ttl_seconds": 2592000,
  "state_ttl_seconds": 604800,
  "event_ttl_seconds": 259200,
  "compliance_hold": false,
  "created_at": "2026-02-14T12:00:00Z",
  "updated_at": "2026-02-14T12:00:00Z",
  "description": "Acme 30-day retention",
  "labels": {"tier": "premium"}
}
```

**Response (409):** A retention policy already exists for this namespace:tenant pair.

### `GET /v1/retention` -- List

List all retention policies, optionally filtered by namespace and tenant.

**Query parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `namespace` | string | No | Filter by namespace |
| `tenant` | string | No | Filter by tenant |
| `limit` | integer | No | Maximum number of results (default: 100) |
| `offset` | integer | No | Number of results to skip (default: 0) |

**Response (200):**

```json
{
  "policies": [
    {
      "id": "ret-019462a1-...",
      "namespace": "notifications",
      "tenant": "acme",
      "enabled": true,
      "audit_ttl_seconds": 2592000,
      "compliance_hold": false,
      "created_at": "2026-02-14T12:00:00Z",
      "updated_at": "2026-02-14T12:00:00Z"
    }
  ],
  "count": 1
}
```

### `GET /v1/retention/{id}` -- Get Detail

Retrieve the full definition of a retention policy.

**Response (200):** Full `RetentionPolicy` object.

**Response (404):** `{"error": "retention policy not found: {id}"}`

### `PUT /v1/retention/{id}` -- Update

Update an existing retention policy. Only provided fields are changed.

**Request body (partial):**

```json
{
  "audit_ttl_seconds": 7776000,
  "compliance_hold": true,
  "description": "Upgraded to compliance hold"
}
```

Updatable fields: `enabled`, `audit_ttl_seconds`, `state_ttl_seconds`, `event_ttl_seconds`, `compliance_hold`, `description`, `labels`.

**Response (200):** Updated `RetentionPolicy` object.

### `DELETE /v1/retention/{id}` -- Delete

Permanently delete a retention policy. The tenant reverts to the gateway default TTL.

**Response (204):** No content.

## Usage Examples

### Create a retention policy

```bash
curl -X POST http://localhost:8080/v1/retention \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "acme",
    "audit_ttl_seconds": 2592000,
    "state_ttl_seconds": 604800,
    "event_ttl_seconds": 259200,
    "description": "Acme 30-day audit retention"
  }'
```

### Set compliance hold

```bash
curl -X POST http://localhost:8080/v1/retention \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "healthcare-corp",
    "compliance_hold": true,
    "description": "HIPAA compliance hold"
  }'
```

### Update TTL (extend retention)

```bash
curl -X PUT "http://localhost:8080/v1/retention/ret-001" \
  -H "Content-Type: application/json" \
  -d '{"audit_ttl_seconds": 7776000}'
```

### Disable a policy temporarily

```bash
curl -X PUT "http://localhost:8080/v1/retention/ret-001" \
  -H "Content-Type: application/json" \
  -d '{"enabled": false}'
```

### Delete a retention policy

```bash
curl -X DELETE "http://localhost:8080/v1/retention/ret-001"
```

## Background Reaper

The background reaper is a periodic task that runs on a configurable interval (default: 3600 seconds / 1 hour). On each cycle it:

1. Reloads retention policies from the state store (hot-reload across instances)
2. For each enabled policy with `compliance_hold = false`:
   - Scans for completed/failed/cancelled chains older than `state_ttl_seconds` and deletes them
   - Scans for resolved events older than `event_ttl_seconds` and deletes them
3. For policies with `compliance_hold = true`, skips the tenant entirely
4. Records metrics for deleted entries, skipped entries, and errors

Enable the reaper in the server configuration:

```toml
[background]
enable_retention_reaper = true
retention_check_interval_seconds = 3600
```

The reaper operates independently of the audit TTL mechanism. Audit TTLs are enforced at write time (the audit store backend handles expiry), while the reaper handles state-store cleanup for chains and events.

## Monitoring

### Prometheus Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `retention_deleted_state` | Counter | State entries deleted by the retention reaper |
| `retention_skipped_compliance` | Counter | Entries skipped due to compliance hold |
| `retention_errors` | Counter | Errors encountered during retention reaper cycles |

### Structured Logging

| Event | Level | Description |
|-------|-------|-------------|
| Reaper cycle complete | info | `retention reaper cycle complete` with counts of deleted, skipped, errors |
| Chain reap error | error | `retention reaper: error reaping chains` with namespace, tenant, error |
| Event reap error | error | `retention reaper: error reaping events` with namespace, tenant, error |

## Best Practices

- **Start without compliance hold**: Use explicit `audit_ttl_seconds` values initially. Enable `compliance_hold` only for tenants with a genuine regulatory requirement.
- **Set all three TTLs**: Configure `audit_ttl_seconds`, `state_ttl_seconds`, and `event_ttl_seconds` together for consistent data lifecycle management.
- **Use labels for organization**: Labels like `tier: enterprise` or `compliance: hipaa` make it easy to audit and bulk-manage policies.
- **Test with disabled policies**: Before enforcing a new retention schedule, create the policy in a disabled state and verify the effective TTL via the API.
- **Monitor the reaper metrics**: A rising `retention_errors` count may indicate state-store connectivity issues.
- **Disable before deleting**: Disable a policy first to verify there are no unintended effects before permanently removing it.
- **Align with legal requirements**: Work with your compliance team to determine appropriate TTLs. GDPR typically requires data minimization, while HIPAA and SOC2 require long retention.

## Limitations

- **Single policy per namespace:tenant**: Each tenant can have at most one retention policy per namespace. Multiple overlapping policies for the same scope are not supported.
- **Audit TTL is write-time**: The effective audit TTL is determined when the audit record is written. Changing a retention policy does not retroactively update the TTL of existing records.
- **Reaper granularity**: The background reaper checks on a configurable interval (default: 1 hour). Data may persist slightly beyond the configured TTL until the next reaper cycle.
- **No per-action-type retention**: Retention policies apply to all action types within a namespace:tenant scope. Use labels and separate namespaces if different action types need different retention.
- **State backend dependency**: The reaper relies on the state backend's `scan_keys_by_kind` capability. Backends that do not support key scanning may have limited reaper functionality.
