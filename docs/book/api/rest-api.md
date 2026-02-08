# REST API Reference

Acteon exposes a RESTful HTTP API via Axum with auto-generated OpenAPI/Swagger documentation.

## Base URL

```
http://localhost:8080
```

## Interactive Documentation

- **Swagger UI**: [http://localhost:8080/swagger-ui/](http://localhost:8080/swagger-ui/)
- **OpenAPI Spec**: [http://localhost:8080/api-doc/openapi.json](http://localhost:8080/api-doc/openapi.json)

---

## Health & Metrics

### `GET /health`

Health check with metrics snapshot.

**Response:**

```json
{
  "status": "ok",
  "metrics": {
    "dispatched": 1500,
    "executed": 1200,
    "deduplicated": 150,
    "suppressed": 50,
    "rerouted": 30,
    "throttled": 20,
    "failed": 10,
    "grouped": 25,
    "pending_approval": 5
  }
}
```

### `GET /metrics`

Dispatch counters only.

**Response:**

```json
{
  "dispatched": 1500,
  "executed": 1200,
  "deduplicated": 150,
  "suppressed": 50,
  "rerouted": 30,
  "throttled": 20,
  "failed": 10
}
```

---

## Action Dispatch

### `POST /v1/dispatch`

Dispatch a single action through the gateway pipeline.

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dry_run` | bool | `false` | When `true`, evaluates rules without executing. See [Dry-Run Mode](../features/dry-run.md). |

**Request Body:**

```json
{
  "namespace": "notifications",
  "tenant": "tenant-1",
  "provider": "email",
  "action_type": "send_email",
  "payload": {
    "to": "user@example.com",
    "subject": "Hello!"
  },
  "dedup_key": "welcome-user@example.com",
  "metadata": {
    "labels": {
      "priority": "high"
    }
  },
  "status": "firing",
  "fingerprint": "alert-cluster1-cpu",
  "starts_at": "2026-01-15T10:00:00Z",
  "ends_at": "2026-01-15T11:00:00Z"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `namespace` | string | Yes | Logical namespace |
| `tenant` | string | Yes | Tenant identifier |
| `provider` | string | Yes | Target provider |
| `action_type` | string | Yes | Action discriminator |
| `payload` | object | Yes | Arbitrary JSON payload |
| `dedup_key` | string | No | Deduplication key |
| `metadata.labels` | object | No | Key-value labels |
| `status` | string | No | Current event state |
| `fingerprint` | string | No | Event correlation ID |
| `starts_at` | datetime | No | Event lifecycle start |
| `ends_at` | datetime | No | Event lifecycle end |

**Response (200):**

```json
{
  "outcome": "executed",
  "response": {
    "status": "success",
    "body": {"sent": true}
  }
}
```

**Possible Outcomes:**

| Outcome | Description |
|---------|-------------|
| `executed` | Successfully executed by provider |
| `deduplicated` | Already processed within TTL |
| `suppressed` | Blocked by rule |
| `rerouted` | Redirected to different provider |
| `throttled` | Rate limit exceeded |
| `failed` | Provider error after all retries |
| `grouped` | Added to event group |
| `state_changed` | Event state transitioned |
| `pending_approval` | Awaiting human approval |
| `chain_started` | Multi-step chain initiated |

### `POST /v1/dispatch/batch`

Dispatch multiple actions in a single request.

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dry_run` | bool | `false` | When `true`, evaluates rules without executing. See [Dry-Run Mode](../features/dry-run.md). |

**Request Body:**

```json
{
  "actions": [
    {
      "namespace": "notifications",
      "tenant": "tenant-1",
      "provider": "email",
      "action_type": "send_email",
      "payload": {"to": "alice@example.com"}
    },
    {
      "namespace": "notifications",
      "tenant": "tenant-1",
      "provider": "sms",
      "action_type": "send_sms",
      "payload": {"to": "+1234567890"}
    }
  ]
}
```

**Response (200):**

```json
{
  "results": [
    {"status": "success", "outcome": {"outcome": "executed", "response": {...}}},
    {"status": "error", "error": {"message": "Provider not found: sms"}}
  ]
}
```

---

## Rule Management

### `GET /v1/rules`

List all loaded rules.

**Response:**

```json
{
  "rules": [
    {
      "name": "dedup-emails",
      "priority": 10,
      "enabled": true,
      "description": "Deduplicate email sends"
    },
    {
      "name": "block-spam",
      "priority": 1,
      "enabled": true,
      "description": "Block spam actions"
    }
  ]
}
```

### `POST /v1/rules/reload`

Reload rules from the configured directory.

**Response:**

```json
{
  "loaded": 5,
  "errors": []
}
```

### `PUT /v1/rules/{name}/enabled`

Enable or disable a rule at runtime.

**Request Body:**

```json
{"enabled": false}
```

**Response:** `200 OK`

---

## Audit Trail

### `GET /v1/audit`

Query audit records with filters.

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `namespace` | string | Filter by namespace |
| `tenant` | string | Filter by tenant |
| `provider` | string | Filter by provider |
| `action_type` | string | Filter by action type |
| `outcome` | string | Filter by outcome |
| `verdict` | string | Filter by verdict |
| `matched_rule` | string | Filter by rule name |
| `caller_id` | string | Filter by caller |
| `chain_id` | string | Filter by chain |
| `from` | datetime | Start of range |
| `to` | datetime | End of range |
| `limit` | u32 | Max results (default: 50, max: 1000) |
| `offset` | u32 | Pagination offset |

**Response:**

```json
{
  "records": [...],
  "total": 150,
  "limit": 50,
  "offset": 0
}
```

### `GET /v1/audit/{action_id}`

Get a specific audit record.

---

## Events (State Machines)

### `GET /v1/events`

List events, optionally filtered by status.

**Query Parameters:** `status`, `namespace`, `tenant`

### `GET /v1/events/{fingerprint}`

Get event lifecycle state.

**Query Parameters:** `namespace`, `tenant`

### `PUT /v1/events/{fingerprint}/transition`

Transition an event to a new state.

**Request Body:**

```json
{
  "to_state": "acknowledged",
  "namespace": "monitoring",
  "tenant": "tenant-1"
}
```

---

## Approvals

### `GET /v1/approvals`

List pending approvals.

**Query Parameters:** `namespace`, `tenant`

### `POST /v1/approvals/{namespace}/{tenant}/{id}/approve`

Approve a pending action (requires HMAC signature).

**Query Parameters:** `sig`, `expires_at`, `kid` (optional)

### `POST /v1/approvals/{namespace}/{tenant}/{id}/reject`

Reject a pending action (requires HMAC signature).

**Query Parameters:** `sig`, `expires_at`, `kid` (optional)

---

## Event Groups

### `GET /v1/groups`

List active event groups.

### `GET /v1/groups/{group_key}`

Get group details including all events.

### `DELETE /v1/groups/{group_key}`

Force flush/close a group, triggering immediate notification.

---

## Embeddings

### `POST /v1/embeddings/similarity`

Compute cosine similarity between a text and a topic using the configured embedding provider. Useful for testing semantic match thresholds before writing rules.

**Rate limit:** 5 requests per minute per caller.

**Required permission:** `Dispatch` (admin or operator role).

**Request Body:**

```json
{
  "text": "The database connection pool is exhausted and queries are timing out",
  "topic": "Infrastructure issues, server problems"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `text` | string | Yes | The text to compare |
| `topic` | string | Yes | The topic to compare against |

**Response (200):**

```json
{
  "similarity": 0.82,
  "topic": "Infrastructure issues, server problems"
}
```

**Error Responses:**

| Status | Description |
|--------|-------------|
| `401` | Unauthorized |
| `403` | Insufficient permissions |
| `404` | Embedding provider not configured |
| `429` | Rate limit exceeded |
| `500` | Embedding computation failed |

---

## Event Streaming

### `GET /v1/stream`

Subscribe to real-time action outcomes via Server-Sent Events (SSE).

**Required permission:** `StreamSubscribe` (admin, operator, or viewer role).

**Query Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `namespace` | string | Filter by namespace |
| `action_type` | string | Filter by action type |
| `outcome` | string | Filter by outcome category (`executed`, `suppressed`, `failed`, `throttled`, `rerouted`, `deduplicated`) |
| `event_type` | string | Filter by event type (`action_dispatched`, `group_flushed`, `timeout`, `chain_advanced`, `approval_required`) |

**SSE Event Format:**

```
event: action_dispatched
id: 550e8400-e29b-41d4-a716-446655440000
data: {"id":"550e8400-...","timestamp":"2026-02-07T14:30:00Z","type":"action_dispatched","outcome":{...},"provider":"email","namespace":"alerts","tenant":"acme","action_type":"send_email","action_id":"661f9511-..."}
```

**SSE Event Types:**

| `event:` tag | Description |
|-------------|-------------|
| `action_dispatched` | Action processed through the dispatch pipeline |
| `group_flushed` | Batch of grouped events flushed |
| `timeout` | State machine timeout fired |
| `chain_advanced` | Task chain step advanced |
| `approval_required` | Action requires human approval |
| `lagged` | Client fell behind, events were skipped |

**Security:**
- Events are tenant-isolated (scoped callers only see their tenants)
- `ProviderResponse` bodies and headers are sanitized (replaced with `null`/empty)
- Approval URLs are redacted to `[redacted]`

**Error Responses:**

| Status | Description |
|--------|-------------|
| `401` | Unauthorized |
| `403` | Insufficient permissions (requires `StreamSubscribe`) |
| `429` | Too many concurrent SSE connections for this tenant |
| `503` | SSE streaming is not enabled |

**Example:**

```bash
curl -N -H "Authorization: Bearer <token>" \
  "http://localhost:8080/v1/stream?namespace=alerts&outcome=failed"
```

See [Event Streaming](../features/event-streaming.md) for full documentation.

---

## Circuit Breaker Admin

These endpoints require the **admin** or **operator** role.

### `GET /admin/circuit-breakers`

List all circuit breakers with their current distributed state and configuration.

**Response:**

```json
{
  "circuit_breakers": [
    {
      "provider": "email",
      "state": "closed",
      "failure_threshold": 5,
      "success_threshold": 2,
      "recovery_timeout_seconds": 60,
      "fallback_provider": "webhook"
    }
  ]
}
```

| Status | Description |
|--------|-------------|
| `200` | List of circuit breakers |
| `404` | Circuit breakers not enabled |

### `POST /admin/circuit-breakers/{provider}/trip`

Force-open a circuit breaker, immediately rejecting requests to the provider.

**Response:**

```json
{
  "provider": "email",
  "state": "open",
  "message": "circuit breaker tripped"
}
```

| Status | Description |
|--------|-------------|
| `200` | Circuit breaker tripped |
| `403` | Insufficient permissions |
| `404` | Circuit breaker not found or not enabled |

### `POST /admin/circuit-breakers/{provider}/reset`

Force-close a circuit breaker, restoring normal request flow.

**Response:**

```json
{
  "provider": "email",
  "state": "closed",
  "message": "circuit breaker reset"
}
```

| Status | Description |
|--------|-------------|
| `200` | Circuit breaker reset |
| `403` | Insufficient permissions |
| `404` | Circuit breaker not found or not enabled |

---

## Authentication

### `POST /v1/auth/login`

Authenticate and receive a JWT token.

**Request Body:**

```json
{
  "username": "admin",
  "password": "secret"
}
```

**Response:**

```json
{
  "token": "eyJ...",
  "expires_in": 3600
}
```

### `POST /v1/auth/logout`

Revoke the current JWT token.

**Headers:** `Authorization: Bearer <token>`

---

## Endpoint Summary

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check with metrics |
| `GET` | `/metrics` | Dispatch counters |
| `POST` | `/v1/dispatch` | Dispatch single action |
| `POST` | `/v1/dispatch/batch` | Dispatch multiple actions |
| `GET` | `/v1/rules` | List rules |
| `POST` | `/v1/rules/reload` | Reload rules |
| `PUT` | `/v1/rules/{name}/enabled` | Toggle rule |
| `GET` | `/v1/audit` | Query audit |
| `GET` | `/v1/audit/{action_id}` | Get audit record |
| `GET` | `/v1/events` | List events |
| `GET` | `/v1/events/{fingerprint}` | Get event |
| `PUT` | `/v1/events/{fingerprint}/transition` | Transition event |
| `GET` | `/v1/approvals` | List approvals |
| `POST` | `/v1/approvals/{ns}/{tenant}/{id}/approve` | Approve action |
| `POST` | `/v1/approvals/{ns}/{tenant}/{id}/reject` | Reject action |
| `GET` | `/v1/groups` | List groups |
| `GET` | `/v1/groups/{group_key}` | Get group |
| `DELETE` | `/v1/groups/{group_key}` | Flush group |
| `POST` | `/v1/embeddings/similarity` | Compute embedding similarity |
| `GET` | `/admin/circuit-breakers` | List circuit breakers |
| `POST` | `/admin/circuit-breakers/{provider}/trip` | Force-open circuit breaker |
| `POST` | `/admin/circuit-breakers/{provider}/reset` | Force-close circuit breaker |
| `GET` | `/v1/stream` | SSE event stream |
| `POST` | `/v1/auth/login` | Login |
| `POST` | `/v1/auth/logout` | Logout |
