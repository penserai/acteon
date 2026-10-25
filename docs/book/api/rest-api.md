# REST API Reference

Tenant context comes from the authenticated JWT or API key. Requests do not
accept tenant selectors. See [credential-derived context](tenant-context.md).

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

### `GET /metrics/prometheus`

Prometheus text exposition metrics, including configuration-backed audit and
dead-letter retention TTL gauges.

### `GET /v1/metrics/alerts/prometheus.yaml`

Generated Prometheus alerting rules for the features enabled in the running
configuration. The endpoint is public like the scrape endpoint and contains
no secrets or tenant labels.

---

## Action Dispatch

### `POST /v1/dispatch`

Dispatch a single action through the gateway pipeline.

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dry_run` | bool | `false` | When `true`, evaluates rules without executing. See [Dry-Run Mode](../features/dry-run.md). |

**Request Headers:**

| Header | Required | Description |
|--------|----------|-------------|
| `Content-Type` | Yes | Must be `application/json` |
| `Authorization` | When auth enabled | Bearer token or API key |
| `traceparent` | No | W3C Trace Context parent (e.g., `00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01`). When present and [distributed tracing](../features/distributed-tracing.md) is enabled, the server-side trace is linked to the caller's trace. |
| `tracestate` | No | W3C Trace Context state. Vendor-specific trace data propagated alongside `traceparent`. |

**Request Body:**

```json
{
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
| `provider_pending` | Retained provider attempt is in flight, requires reconciliation, or awaits retry; observe its original receipt and do not resend as fresh work |
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

**Request Headers:**

Supports the same `traceparent` and `tracestate` headers as the single dispatch endpoint. The trace context applies to the batch request span; each individual action creates a child span within it.

**Request Body:**

```json
{
  "actions": [
    {
      "provider": "email",
      "action_type": "send_email",
      "payload": {"to": "alice@example.com"}
    },
    {
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

**Query Parameters:** `status`

### `GET /v1/events/{fingerprint}`

Get event lifecycle state.

Context is derived from the authenticated credential.

### `PUT /v1/events/{fingerprint}/transition`

Transition an event to a new state.

**Request Body:**

```json
{
  "to_state": "acknowledged"
}
```

---

## Approvals

### `GET /v1/approvals`

List pending approvals.

Context is derived from the authenticated credential.

### `POST /v1/approvals/{id}/approve`

Approve a pending action (requires authenticated operator or admin access).

Send the operator JWT or service API key in the authorization header.

### `POST /v1/approvals/{id}/reject`

Reject a pending action (requires authenticated operator or admin access).

Send the operator JWT or service API key in the authorization header.

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
  "http://localhost:8080/v1/stream?outcome=failed"
```

See [Event Streaming](../features/event-streaming.md) for full documentation.

---

## Recurring Actions

### `POST /v1/recurring`

Create a new recurring action.

**Request Body:**

```json
{
  "cron_expr": "0 9 * * MON-FRI",
  "timezone": "US/Eastern",
  "provider": "email",
  "action_type": "send_digest",
  "payload": {"to": "team@example.com"},
  "description": "Weekday morning digest"
}
```

**Response (201):**

```json
{
  "recurring_id": "uuid-...",
  "next_execution": "2026-02-10T14:00:00Z",
  "cron_expr": "0 9 * * MON-FRI",
  "timezone": "US/Eastern",
  "enabled": true
}
```

| Status | Description |
|--------|-------------|
| `201` | Recurring action created |
| `400` | Invalid cron expression, timezone, or missing required fields |

### `GET /v1/recurring`

List recurring actions in the credential context.

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `enabled` | bool | No | Filter by enabled status |

### `GET /v1/recurring/{id}`

Get a recurring action by ID.

Context is derived from the authenticated credential.

| Status | Description |
|--------|-------------|
| `200` | Recurring action found |
| `404` | Recurring action not found |

### `PUT /v1/recurring/{id}`

Update a recurring action (partial update).

Context is derived from the authenticated credential.

| Status | Description |
|--------|-------------|
| `200` | Recurring action updated |
| `400` | Invalid cron expression or timezone |
| `404` | Recurring action not found |

### `DELETE /v1/recurring/{id}`

Delete a recurring action.

Context is derived from the authenticated credential.

| Status | Description |
|--------|-------------|
| `204` | Deleted |
| `404` | Recurring action not found |

### `POST /v1/recurring/{id}/pause`

Pause a recurring action (stops future executions).

Context is derived from the authenticated credential.

### `POST /v1/recurring/{id}/resume`

Resume a paused recurring action (recomputes next execution from now).

Context is derived from the authenticated credential.

See [Recurring Actions](../features/recurring-actions.md) for full documentation.

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

## Task Chains & Definitions

### `GET /v1/chains`

List active and completed chains. Filters by `status` within the credential context.

### `GET /v1/chains/{chain_id}`

Get chain execution details including step results and current step.

### `POST /v1/chains/{chain_id}/cancel`

Cancel an in-flight chain execution.

### `GET /v1/chains/{chain_id}/dag`

Return the directed acyclic graph (DAG) representation of the chain.

### `GET /v1/chains/definitions`

List reusable chain definitions.

### `PUT /v1/chains/definitions/{name}`

Create or update a chain definition template.

---

## Task Queues & Workers

### `POST /v1/queues/{queue}/tasks`

Enqueue a task for distributed worker execution.

### `POST /v1/queues/{queue}/poll`

Poll for available tasks with lease acquisition.

### `POST /v1/queues/tasks/{task_id}/heartbeat`

Renew a task lease during long-running execution.

### `POST /v1/queues/tasks/{task_id}/complete`

Mark a leased worker task as successfully completed.

### `POST /v1/queues/tasks/{task_id}/fail`

Mark a leased worker task as failed with optional retry.

---

## Workflows as Code & Executions

### `POST /v1/workflows/start`

Start a durable workflow execution from code (Python / TypeScript SDK).

### `GET /v1/workflows/executions`

List durable workflow executions.

### `GET /v1/executions/{execution_id}/history`

Retrieve append-only execution event history log.

### `POST /v1/executions/{execution_id}/signal/{signal_name}`

Deliver an asynchronous external signal to a waiting execution.

---

## Agent Interop (A2A Protocol v1.0)

### `POST /a2a`

A2A JSON-RPC 2.0 endpoint supporting the v1 `SendMessage`, `GetTask`,
`ListTasks`, `CancelTask`, and `GetExtendedAgentCard` methods. `ListTasks`
provides filter-bound cursor pagination and the context, state, transition-time,
history, and artifact selectors defined by A2A 1.0. The four v1 Task push
notification configuration methods use schema-closed ProtoJSON and cursor
pagination. Pre-1.0 slash-separated methods remain compatibility aliases.

### `POST /a2a/agents/{agent}`

Official A2A 1.0 JSON-RPC bound to one configured agent service. `SendMessage`
enters the governed execution runtime and `GetTask` revalidates the original
authenticated source. Admission returns the opaque
`X-Acteon-Agent-Source-Context` receipt; agent principals must return it for
observation or continuation. The route also supports `GetExtendedAgentCard`,
`SendStreamingMessage`, `SubscribeToTask`, governed `ListTasks`, and governed `CancelTask`.
Cancellation commits a future-start fence before returning. It reports a
confirmed canceled Task only for a local non-start or qualified no-effect
finality; otherwise the Task remains in its observed state with
`metadata.acteonCancellation.state` set to `requested`. Both streaming methods use the
exact card's capability, the shared tenant connection quota, and current
source-provenance validation before every emitted status, artifact, or recovery
snapshot. `ListTasks` scans only governed acceptances for this exact agent,
filters them by the current source, sorts by last update descending, and binds
its cursor to the caller, source receipt, and filters. Agent principals need an
exact job receipt and therefore cannot enumerate sibling jobs that share their
credential. Push methods remain unsupported and never fall back to the tenant
Task engine.

### `POST /a2a/v1/message:send`

A2A REST binding for submitting agent messages and tasks. An
`InputRequired` continuation must carry the current Task `taskId`, matching
`contextId`, and `metadata["acteon.challengeId"]` equal to the Task's
`pendingApprovalId`. It appends the response and resumes the Task; identical
retries are safe. An expired challenge fails the still-paused Task. An
`AuthRequired` Task cannot be authorized by message content.

### `POST /a2a/agents/{agent}/v1/tasks/{id}/authorization:request`

The authenticated recipient agent opens an `AuthRequired` challenge with an
opaque `authorizationRequestId`. Acteon supplies the verifier, recipient,
credential authority, audience, scopes, and TTL from the service binding.

### `POST /a2a/agents/{agent}/v1/tasks/{id}/authorization:resolve`

The authenticated original requester asks Acteon to verify the exact
`challengeId`. The body contains no credential. A denial leaves the challenge
pending; transient verifier failure returns `503` without changing task state.

### `POST /a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}/authorization:resolve`

The currently authenticated source agent asks the target host to resolve the
accepted peer task's exact `challengeId`. The body contains no credential or
verifier evidence. Acteon records an at-most-once handoff in the configured
state backend. A response is `resolved`, `rejected`, or `uncertain`; after an
ambiguous response, the same request observes the target and can settle
`resolved` without replaying the verifier operation. Cancellation, rejection,
or failure after an ambiguous response remains `uncertain` because it cannot
prove that authorization succeeded.

### `GET /a2a/v1/tasks/{id}`

Retrieve task lifecycle status, output artifacts, and sub-task graphs.

### `GET /a2a/v1/tasks/{id}/events`

Server-Sent Events (SSE) stream of real-time task lifecycle transitions.

### `GET /a2a/.well-known/agent-card.json`

Public A2A 1.0 ProtoJSON Agent Card discovery endpoint. Only a published
JSON-RPC interface is advertised until the HTTP+JSON binding has its own v1
wire adapter and conformance gate. Push notifications follow the published
capability after their v1 adapter passed external client conformance; streaming
remains false until its v1 adapter passes the same gate.

### `GET /a2a/.well-known/agent.json`

Compatibility discovery endpoint returning Acteon's stored card or
tenant-aggregated catalog.

---

## Agentic Message Bus

### `GET /v1/bus/topics` & `POST /v1/bus/topics`

List and create Kafka-backed topics.

### `POST /v1/bus/publish`

Publish an event to a topic with JSON schema validation.

### `GET /v1/bus/subscriptions` & `POST /v1/bus/subscriptions`

Manage durable subscriptions with offset tracking.

### `POST /v1/bus/subscriptions/{id}/ack`

Acknowledge processed message offset.

### `GET /v1/bus/agents` & `POST /v1/bus/agents`

List and register autonomous agents with heartbeat monitoring.

### `GET /v1/bus/conversations` & `POST /v1/bus/conversations`

List and create multi-agent conversation threads.

---

## Tenant Quotas & Governance

### `GET /v1/quotas` & `POST /v1/quotas`

List and configure per-tenant quota policies.

### `GET /v1/quotas/{id}/usage`

Inspect real-time quota consumption and remaining allowance.

### `POST /v1/quotas/reload`

Trigger manual reload of static quota manifests.

### `GET /v1/silences` & `POST /v1/silences`

List and create Alertmanager-compatible alert silences.

### `GET /v1/time-intervals`

List and manage temporal routing windows.

### `GET /v1/retention` & `POST /v1/retention`

List and configure audit and DLQ data retention policies.

---

## Resilience & Dead-Letter Queue

### `GET /v1/dlq/stats`

Dead-letter queue message count and failure breakdown.

### `POST /v1/dlq/drain`

Drain dead-letter queue records for redelivery.

### `GET /v1/providers/health`

Real-time provider latency, error count, and circuit breaker status.

---

## Templates & Plugins

### `GET /v1/templates` & `POST /v1/templates`

List and create MiniJinja payload templates.

### `POST /v1/templates/render`

Preview rendered template output with sample context.

### `GET /v1/plugins`

List loaded WebAssembly (WASM) rule plugins.

---

## Compliance & Cryptographic Proofs

### `GET /v1/compliance/status`

Check active SOC2 / HIPAA compliance enforcement settings.

### `POST /v1/audit/verify`

Verify the SHA-256 tamper-evident audit log hash chain.

### `GET /v1/actions/{id}/verify`

Cryptographically verify an inbound action's Ed25519 signature.

### `GET /.well-known/acteon-signing-keys`

JWKS-style discovery endpoint for public signing keys.

---

## Swarm Orchestration

### `GET /v1/swarm/runs`

List autonomous agent swarm runs and execution progress.

### `POST /v1/swarm/runs/{run_id}/cancel`

Cancel an active swarm execution run.

---

## Endpoint Summary

| Method | Path | Description |
|--------|------|-------------|
| **System & Monitoring** |||
| `GET` | `/health` | Health check with metrics snapshot |
| `GET` | `/metrics` | Dispatch counters |
| `GET` | `/metrics/prometheus` | Prometheus metrics scrape endpoint |
| `GET` | `/v1/metrics/alerts/prometheus.yaml` | Generated Prometheus alerting rules |
| **Action Dispatch** |||
| `POST` | `/v1/dispatch` | Dispatch single action (supports `?dry_run=true`) |
| `POST` | `/v1/dispatch/batch` | Dispatch multiple actions atomically |
| `GET` | `/v1/stream` | Real-time SSE event stream |
| **Rules & Evaluation** |||
| `GET` | `/v1/rules` | List loaded rules |
| `POST` | `/v1/rules/reload` | Reload rules from directory |
| `PUT` | `/v1/rules/{name}/enabled` | Toggle rule |
| `POST` | `/v1/rules/evaluate` | Dry-run trace evaluation |
| `GET` | `/v1/rules/coverage` | Rule condition test coverage report |
| **Audit & Replay** |||
| `GET` | `/v1/audit` | Query audit records |
| `GET` | `/v1/audit/{action_id}` | Get audit record |
| `POST` | `/v1/audit/replay` | Replay actions from audit trail |
| `POST` | `/v1/audit/verify` | Verify cryptographic SHA-256 audit hash chain |
| `GET` | `/v1/actions/{id}/verify` | Verify Ed25519 signature of an action |
| **Stateful Events & Groups** |||
| `GET` | `/v1/events` | List events |
| `GET` | `/v1/events/{fingerprint}` | Get event lifecycle state |
| `PUT` | `/v1/events/{fingerprint}/transition` | Transition event state |
| `GET` | `/v1/groups` | List active notification groups |
| `GET` | `/v1/groups/{group_key}` | Get group details |
| `DELETE` | `/v1/groups/{group_key}` | Force flush group |
| **Chains & Workflows** |||
| `GET` | `/v1/chains` | List chains |
| `GET` | `/v1/chains/{chain_id}` | Get chain status |
| `POST` | `/v1/chains/{chain_id}/cancel` | Cancel chain execution |
| `GET` | `/v1/chains/{chain_id}/dag` | Chain execution DAG |
| `GET` | `/v1/chains/definitions` | List chain definitions |
| `POST` | `/v1/workflows/start` | Start code-based durable workflow |
| `GET` | `/v1/executions` | List durable executions |
| `GET` | `/v1/executions/{id}/history` | Execution event history |
| `POST` | `/v1/queues/{queue}/poll` | Worker queue poll with lease |
| `POST` | `/v1/queues/tasks/{id}/complete` | Complete worker task |
| **A2A Protocol & Bus** |||
| `POST` | `/a2a` | A2A JSON-RPC 2.0 endpoint |
| `POST` | `/a2a/agents/{agent}` | Exact-agent A2A JSON-RPC through the governed execution runtime |
| `POST` | `/a2a/v1/message:send` | A2A REST message/task submit |
| `GET` | `/a2a/agents/{agent}/v1/tasks/{id}/peers?skill={exact-skill}` | List current safe, source-authorized registry options; descriptions are untrusted and the result grants no authority |
| `POST` | `/a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/message:send` | Governed peer submission from an accepted agent task |
| `POST` | `/a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}:refresh` | Revalidate authority and journal the latest accepted remote task snapshot |
| `POST` | `/a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}:cancel` | Persist and deliver at most one governed remote cancellation; reconcile ambiguity by safe observation |
| `POST` | `/a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}/message:send` | Persist and deliver one exact response to the remote task's active input challenge |
| `POST` | `/a2a/agents/{agent}/v1/tasks/{id}/peers/{target}/{skill}/submissions/{submission}/authorization:resolve` | Persist an opaque remote authorization handoff and reconcile response loss without verifier replay |
| `GET` | `/a2a/v1/tasks/{id}` | A2A task details |
| `GET` | `/a2a/.well-known/agent.json` | Authenticated A2A agent card discovery |
| `GET` | `/v1/bus/topics` | List bus topics |
| `POST` | `/v1/bus/publish` | Publish message to bus topic |
| `GET` | `/v1/bus/subscriptions` | List durable subscriptions |
| `GET` | `/v1/bus/agents` | List registered bus agents |
| `GET` | `/v1/bus/conversations` | List conversation threads |
| **Governance & Resilience** |||
| `GET` | `/v1/quotas` | List tenant quotas |
| `GET` | `/v1/silences` | List alert silences |
| `GET` | `/v1/time-intervals` | List time intervals |
| `GET` | `/v1/retention` | List retention policies |
| `GET` | `/v1/providers/health` | Provider health metrics & circuit breakers |
| `GET` | `/v1/governance/federation/deliveries` | Page through sanitized outbound federation evidence under independent reconciliation authority |
| `POST` | `/v1/governance/federation/deliveries/{delivery_id}:abandon` | Stop local recovery while preserving an unknown peer outcome |
| `GET` | `/v1/governance/federation/deliveries/{delivery_id}/correlation` | Read immutable correlation for the currently ambiguous peer stage |
| `POST` | `/v1/governance/federation/deliveries/{delivery_id}:reconcile` | Accept proof through the deployment-installed peer-finality verifier without peer traffic |
| `GET` | `/admin/circuit-breakers` | List circuit breakers |
| `POST` | `/admin/circuit-breakers/{provider}/trip` | Force-open circuit breaker |
| `POST` | `/admin/circuit-breakers/{provider}/reset` | Force-close circuit breaker |
| `GET` | `/v1/dlq/stats` | DLQ statistics |
| `POST` | `/v1/dlq/drain` | Drain DLQ entries |
| `GET` | `/v1/approvals` | List approvals |
| `POST` | `/v1/approvals/{id}/approve` | Approve action |
| `POST` | `/v1/approvals/{id}/reject` | Reject action |
| `POST` | `/v1/recurring` | Create recurring action |
| `GET` | `/v1/recurring` | List recurring actions |
| `POST` | `/v1/auth/login` | Login |
| `POST` | `/v1/auth/logout` | Logout |

## Explicit execution permits

Independently authorized operators inspect and change workforce relationships
through `GET /v1/workforce` and
`POST /v1/workforce/changes`. These endpoints use typed team, membership,
ownership, assignment, mandate and represented-permit declarations. See
[Agent workforce](../features/workforce.md) for all change kinds and current
authority checks.

When the execution-authority deployment profile is enabled, single and batch
`POST /v1/dispatch` requests select issued permits through
`x-acteon-execution-permits`, for example:
`[{"id":"maya-incident","accepted_revision":1}]`. Ordinary authentication and
grants still apply. See [Execution permits](../features/execution-permits.md) for
configuration, typed SDK methods, refusal handling and supported execution paths.
Dry runs can omit references.

## Federation import admission

An independently authorized manager publishes or terminally revokes one of the
deployment's exact trust declarations through:

```text
POST /v1/governance/federation/trusts
POST /v1/governance/federation/trusts/revoke
```

A foreign peer authenticated as its configured local principal first obtains
the capability card addressed to its configured foreign domain:

```text
GET /a2a/federation/agents/{agent}/capabilities
```

The response contains exact signed `payload` bytes and an Ed25519 `signature`.
It binds the current agent service digest, skill, protocol revision, replay and
finality guarantees, authentication schemes, peer audience, key epoch and a
short validity window. The caller cannot choose the audience.

The peer submits exact signed authority bytes through:

```text
POST /a2a/federation/envelopes:import
```

The JSON body is `{"envelope":{"payload":"...","signature":"..."}}`.
Payloads are limited to 64 KiB and the signature is exactly 128 lowercase hex
characters encoding a 64-byte Ed25519 signature. Success returns a correlated
`FederationImportReceipt` with
`execution_started: false`; the endpoint does not create a task or effect. See
[A2A federation status](../features/a2a.md#federation-status) for trust,
identity and service-binding guarantees.

To execute or replay an admitted message, the same configured peer calls:

```text
POST /a2a/federation/agents/{agent}/v1/message:send
```

The body is
`{"invocation":{"payload":"...","signature":"..."},"message":{...}}`.
The signed payload binds the retained import and exact prepared provider input.
Success returns the durable A2A `Task`. Repeating the exact request returns that
task without duplicating a completed provider effect. A different message,
agent, import, signature or current binding is rejected.

## Historical provider executions

Read `GET /v1/governance/executions/{execution_id}` using the authenticated credential
query parameters. This is a provider execution UUID, which may differ from a
chain execution ID. Authentication must produce Acteon's private middleware
proof; caller-supplied actor or role labels do not authorize access.

A deployment manager needs `can_read_history = true` and must include the
execution's authenticated subject in its `subjects` allowlist. The server checks
current authority before and after reading. Reads return no execution or
settlement grant and perform no state repair. Original and reconciliation
evidence remain distinct, with digest verification against the authority ledger.
Unknown or inaccessible executions return `404`; denied management authority
returns `403`; missing pinned evidence returns `503`; conflicting evidence or an
authority change during observation returns `409`.
