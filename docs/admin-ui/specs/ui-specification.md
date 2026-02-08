# Acteon Admin UI -- Comprehensive UI Specification

This document catalogs every feature, configuration option, metric, data entity,
and API endpoint in the Acteon codebase that the Admin UI must expose. It is the
authoritative input for Task #3 (design) and Task #4 (implementation).

---

## Table of Contents

1. [Dashboard / Overview](#1-dashboard--overview)
2. [Action Dispatch](#2-action-dispatch)
3. [Rules Engine](#3-rules-engine)
4. [Audit Trail](#4-audit-trail)
5. [Event Lifecycle (State Machines)](#5-event-lifecycle-state-machines)
6. [Event Groups](#6-event-groups)
7. [Task Chains](#7-task-chains)
8. [Approvals](#8-approvals)
9. [Circuit Breakers](#9-circuit-breakers)
10. [Dead-Letter Queue](#10-dead-letter-queue)
11. [Real-Time Stream (SSE)](#11-real-time-stream-sse)
12. [Embedding / Semantic Routing](#12-embedding--semantic-routing)
13. [Rate Limiting](#13-rate-limiting)
14. [Authentication & Authorization](#14-authentication--authorization)
15. [Providers](#15-providers)
16. [Metrics](#16-metrics)
17. [Server Configuration Reference](#17-server-configuration-reference)
18. [Background Processing](#18-background-processing)
19. [LLM Guardrail](#19-llm-guardrail)
20. [Telemetry / Tracing](#20-telemetry--tracing)

---

## 1. Dashboard / Overview

### Purpose
Landing page showing system health at a glance.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Health check with embedded metrics snapshot |
| GET | `/metrics` | Standalone metrics counters |

### Data Model: `HealthResponse`
| Field | Type | Description |
|-------|------|-------------|
| `status` | string | `"ok"` |
| `metrics` | MetricsResponse | Embedded gateway metrics |

### Data Model: `MetricsResponse`
| Field | Type | Description |
|-------|------|-------------|
| `dispatched` | u64 | Total actions dispatched |
| `executed` | u64 | Successfully executed |
| `deduplicated` | u64 | Suppressed by dedup |
| `suppressed` | u64 | Suppressed by rules |
| `rerouted` | u64 | Rerouted to alternate provider |
| `throttled` | u64 | Rejected by throttle |
| `failed` | u64 | Failed execution |
| `pending_approval` | u64 | Awaiting human approval |
| `chains_started` | u64 | Chain executions started |
| `chains_completed` | u64 | Chains completed successfully |
| `chains_failed` | u64 | Chains that failed |
| `chains_cancelled` | u64 | Chains cancelled |
| `circuit_open` | u64 | Actions rejected by open circuit |
| `scheduled` | u64 | Actions scheduled for delayed dispatch |
| `embedding` | EmbeddingMetrics (optional) | Embedding provider metrics |

### Data Model: `EmbeddingMetricsResponse`
| Field | Type | Description |
|-------|------|-------------|
| `similarity_requests` | u64 | Total similarity API requests |
| `similarity_errors` | u64 | Failed similarity requests |
| `topic_cache_hits` | u64 | Topic embedding cache hits |
| `topic_cache_misses` | u64 | Topic embedding cache misses |
| `text_cache_hits` | u64 | Text embedding cache hits |
| `text_cache_misses` | u64 | Text embedding cache misses |

### Internal Metrics (from `GatewayMetrics`)
Additional counters available internally but not in the `/metrics` API response:
| Counter | Description |
|---------|-------------|
| `llm_guardrail_allowed` | Actions allowed by LLM guardrail |
| `llm_guardrail_denied` | Actions denied by LLM guardrail |
| `llm_guardrail_errors` | LLM guardrail evaluation errors |
| `circuit_transitions` | Circuit breaker state transitions |
| `circuit_fallbacks` | Fallback provider invocations |

### UI Views
- **KPI cards**: dispatched, executed, failed, pending_approval, circuit_open
- **Outcome donut chart**: breakdown of all outcome types
- **Chain status ring**: started vs completed vs failed vs cancelled
- **Trend sparklines**: dispatched/executed/failed over time (poll `/metrics`)
- **Embedding cache hit rate**: (hits / (hits + misses)) for topic and text
- **LLM guardrail stats**: allowed vs denied vs errors

### Real-Time Aspects
- Poll `/metrics` every 5-10 seconds for live counters
- Optionally connect to `/v1/stream` SSE for real-time event feed

---

## 2. Action Dispatch

### Purpose
Dispatch individual or batch actions through the gateway pipeline.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/dispatch` | Dispatch a single action |
| POST | `/v1/dispatch/batch` | Dispatch multiple actions (array body) |

### Request Model: `DispatchRequest`
| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `namespace` | string | yes | Logical namespace |
| `tenant` | string | yes | Tenant identifier |
| `provider` | string | yes | Target provider name |
| `action_type` | string | yes | Action type discriminator |
| `payload` | JSON object | yes | Arbitrary payload |
| `metadata` | object (string->string) | no | Key-value labels |
| `dedup_key` | string | no | Custom deduplication key |
| `fingerprint` | string | no | Pre-computed fingerprint |
| `status` | string | no | Initial status for state machine |
| `starts_at` | RFC 3339 datetime | no | Scheduled start (delayed dispatch) |
| `ends_at` | RFC 3339 datetime | no | Expiry time |

### Query Parameters
| Param | Type | Description |
|-------|------|-------------|
| `dry_run` | bool | If true, evaluate rules but do not execute |

### Response Model: `DispatchResponse`
| Field | Type | Description |
|-------|------|-------------|
| `action_id` | string | UUID v4 assigned to the action |
| `outcome` | string | One of 13 outcome types (see below) |
| `details` | JSON | Outcome-specific detail object |

### ActionOutcome Variants (13 total)
| Variant | Description | Detail Fields |
|---------|-------------|---------------|
| `Executed` | Provider executed successfully | `response` (ProviderResponse) |
| `Deduplicated` | Matched existing dedup key | `existing_key` |
| `Suppressed` | Blocked by Deny/Suppress rule | `rule_name` |
| `Rerouted` | Sent to alternate provider | `original_provider`, `target_provider`, `response` |
| `Throttled` | Over rate limit | `max_count`, `window_seconds`, `rule_name` |
| `Failed` | Provider execution failed | `error` (ActionError) |
| `Grouped` | Added to event group | `group_id`, `group_size` |
| `StateChanged` | State machine transition | `fingerprint`, `from_state`, `to_state` |
| `PendingApproval` | Awaiting human approval | `approval_id` |
| `ChainStarted` | First step of a chain | `chain_id`, `chain_name` |
| `DryRun` | Rules evaluated, not executed | `would_execute`, `verdict`, `matched_rule` |
| `CircuitOpen` | Provider circuit is open | `provider`, `fallback_used`, `fallback_response` |
| `Scheduled` | Delayed for future dispatch | `action_id`, `scheduled_for` |

### UI Views
- **Dispatch form**: namespace, tenant, provider (dropdown from registered providers), action_type, JSON payload editor, metadata key-value pairs, dedup_key, dry_run toggle
- **Batch dispatch**: JSON array editor or CSV import
- **Dispatch result panel**: shows outcome type, action_id, detail breakdown
- **Recent dispatches**: table of last N dispatches with outcome badges

### Write Actions
- Dispatch single action
- Dispatch batch

---

## 3. Rules Engine

### Purpose
View, manage, and reload routing rules that control the dispatch pipeline.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/rules` | List all loaded rules |
| POST | `/v1/rules/reload` | Reload rules from directory |
| PUT | `/v1/rules/{name}/enabled` | Toggle rule enabled/disabled |

### Data Model: `RuleSummary` (API response)
| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique rule name |
| `priority` | i32 | Evaluation order (lower = first) |
| `description` | string (optional) | Human-readable description |
| `enabled` | bool | Whether rule is active |
| `action_type` | string | The `RuleAction` variant name |
| `action_details` | JSON | Serialized action parameters |
| `source` | string | Where rule was loaded from (yaml/api/inline) |

### Full Rule IR Model (internal, for reference)
| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique identifier |
| `priority` | i32 | Evaluation order |
| `description` | string? | Optional description |
| `enabled` | bool | Active flag |
| `condition` | Expr | Expression tree (CEL or YAML-compiled) |
| `action` | RuleAction | One of 13 action variants |
| `source` | RuleSource | Yaml{file}, Api, Inline |
| `version` | u64 | Change tracking version |
| `metadata` | HashMap<String,String> | Arbitrary KV (e.g., `llm_policy`) |
| `timezone` | string? | IANA timezone for time conditions |

### RuleAction Variants (13 total)
| Variant | Parameters |
|---------|------------|
| `Allow` | (none) |
| `Deny` | (none) |
| `Deduplicate` | `ttl_seconds` |
| `Suppress` | (none) |
| `Reroute` | `target_provider` |
| `Throttle` | `max_count`, `window_seconds` |
| `Modify` | `changes` (JSON patch) |
| `Custom` | `name`, `params` (JSON) |
| `StateMachine` | `state_machine`, `fingerprint_fields` |
| `Group` | `group_by`, `group_wait_seconds`, `group_interval_seconds`, `max_group_size`, `template` |
| `RequestApproval` | `notify_provider`, `timeout_seconds`, `message` |
| `Chain` | `chain` (chain config name) |
| `Schedule` | `delay_seconds` |

### Reload Request
| Field | Type | Description |
|-------|------|-------------|
| `directory` | string? | Override the configured rules directory |

### Toggle Request: `SetEnabledRequest`
| Field | Type | Description |
|-------|------|-------------|
| `enabled` | bool | New enabled state |

### UI Views
- **Rules table**: sortable by priority, filterable by action type and enabled status
  - Columns: priority, name, description, action type, enabled toggle, source, version
- **Rule detail drawer**: full condition expression (rendered), action parameters, metadata, timezone
- **Reload button**: POST `/v1/rules/reload` with optional directory override
- **Enable/disable toggle**: inline in table row, calls PUT `/v1/rules/{name}/enabled`
- **Rule file watcher status**: indicator showing if filesystem watcher is active (from config `rules.directory`)

### Configuration Knobs
| Config Path | Type | Default | Description |
|-------------|------|---------|-------------|
| `rules.directory` | string? | none | Path to YAML rule files |
| `rules.default_timezone` | string? | UTC | Default timezone for time conditions |

---

## 4. Audit Trail

### Purpose
Search, inspect, and replay past action dispatches.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/audit` | Query audit records with filters |
| GET | `/v1/audit/{action_id}` | Get single audit record by action ID |
| POST | `/v1/audit/{action_id}/replay` | Replay a single action |
| POST | `/v1/audit/replay` | Bulk replay with query filters |

### Data Model: `AuditRecord`
| Field | Type | Description |
|-------|------|-------------|
| `id` | string | UUID v7 audit record ID |
| `action_id` | string | Original action ID |
| `chain_id` | string? | Chain execution ID (if part of chain) |
| `namespace` | string | Action namespace |
| `tenant` | string | Action tenant |
| `provider` | string | Target provider |
| `action_type` | string | Action type |
| `verdict` | string | Rule verdict (allow/deny/suppress/etc) |
| `matched_rule` | string? | Name of the rule that fired |
| `outcome` | string | Final outcome (executed/failed/etc) |
| `action_payload` | JSON? | Original payload (null if privacy mode) |
| `verdict_details` | JSON | Rule evaluation details |
| `outcome_details` | JSON | Execution outcome details |
| `metadata` | JSON | Action metadata labels |
| `dispatched_at` | datetime | When dispatched |
| `completed_at` | datetime | When processing completed |
| `duration_ms` | u64 | Pipeline duration |
| `expires_at` | datetime? | TTL expiry |
| `caller_id` | string | Who triggered the action |
| `auth_method` | string | jwt/api_key/anonymous |

### Query Parameters (`AuditQuery`)
| Param | Type | Description |
|-------|------|-------------|
| `namespace` | string? | Filter by namespace |
| `tenant` | string? | Filter by tenant |
| `provider` | string? | Filter by provider |
| `action_type` | string? | Filter by action type |
| `outcome` | string? | Filter by outcome |
| `verdict` | string? | Filter by verdict |
| `matched_rule` | string? | Filter by rule name |
| `caller_id` | string? | Filter by caller |
| `chain_id` | string? | Filter by chain ID |
| `from` | datetime? | Start of time range (RFC 3339) |
| `to` | datetime? | End of time range (RFC 3339) |
| `limit` | u32? | Page size (default 50, max 1000) |
| `offset` | u32? | Pagination offset |

### Pagination Model: `AuditPage`
| Field | Type | Description |
|-------|------|-------------|
| `records` | AuditRecord[] | Page of records |
| `total` | u64 | Total matching count |
| `limit` | u32 | Applied limit |
| `offset` | u32 | Applied offset |

### Replay Models
**Single replay** (`ReplayResult`):
| Field | Type | Description |
|-------|------|-------------|
| `original_action_id` | string | Source audit action ID |
| `new_action_id` | string | New action ID for replayed action |
| `success` | bool | Whether replay succeeded |
| `error` | string? | Error if failed |

**Bulk replay** (`ReplaySummary`):
| Field | Type | Description |
|-------|------|-------------|
| `replayed` | usize | Successfully replayed count |
| `failed` | usize | Failed count |
| `skipped` | usize | Skipped (no payload or unauthorized) |
| `results` | ReplayResult[] | Per-action results |

Bulk replay query supports same filters as audit query plus `limit` (default 50, max 1000).
Concurrency: up to 32 parallel replays (`REPLAY_CONCURRENCY`).

### UI Views
- **Audit log table**: paginated, sortable by `dispatched_at`
  - Columns: action_id, namespace, tenant, provider, action_type, verdict, outcome, duration_ms, dispatched_at, caller_id
  - Filters: all AuditQuery fields as filter chips/dropdowns
  - Time range picker for `from`/`to`
- **Audit detail panel**: full record with collapsible JSON for payload, verdict_details, outcome_details, metadata
- **Replay button**: on individual records and as bulk action with current filters
- **Replay result modal**: shows success/failure counts and per-action details

### Write Actions
- Replay single action (requires Dispatch permission)
- Bulk replay with filters (requires Dispatch permission)

### Configuration Knobs
| Config Path | Type | Default | Description |
|-------------|------|---------|-------------|
| `audit.enabled` | bool | false | Enable audit recording |
| `audit.backend` | string | "memory" | Backend: memory/postgres/clickhouse/elasticsearch |
| `audit.url` | string? | none | Connection URL |
| `audit.prefix` | string | "acteon_" | Table prefix |
| `audit.ttl_seconds` | u64? | 2592000 (30d) | Record TTL |
| `audit.cleanup_interval_seconds` | u64 | 3600 | Background cleanup interval |
| `audit.store_payload` | bool | true | Store action payloads |
| `audit.redact.enabled` | bool | false | Enable field redaction |
| `audit.redact.fields` | string[] | [] | Fields to redact (supports dot-path) |
| `audit.redact.placeholder` | string | "[REDACTED]" | Replacement text |

---

## 5. Event Lifecycle (State Machines)

### Purpose
Track events through state machine lifecycles with transitions and timeouts.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/events` | List events with filters |
| GET | `/v1/events/{fingerprint}` | Get event current state |
| POST | `/v1/events/{fingerprint}/transition` | Manually transition event state |

### Data Model: `StateMachineConfig`
| Field | Type | Description |
|-------|------|-------------|
| `name` | string | State machine name |
| `initial_state` | string | Starting state |
| `states` | string[] | All valid states |
| `transitions` | TransitionConfig[] | Allowed transitions |
| `timeouts` | TimeoutConfig[] | Automatic state timeouts |

### `TransitionConfig`
| Field | Type | Description |
|-------|------|-------------|
| `from` | string | Source state |
| `to` | string | Target state |
| `on_transition` | TransitionEffects? | Side effects |

### `TransitionEffects`
| Field | Type | Description |
|-------|------|-------------|
| `notify` | bool | Whether to send notification |
| `webhook_url` | string? | URL to call on transition |
| `metadata` | HashMap<String,String> | Extra metadata to attach |

### `TimeoutConfig`
| Field | Type | Description |
|-------|------|-------------|
| `in_state` | string | State to watch |
| `after_seconds` | u64 | Seconds before timeout fires |
| `transition_to` | string | Target state on timeout |

### Event State (stored in StateStore as JSON)
| Field | Type | Description |
|-------|------|-------------|
| `state` | string | Current state name |
| `fingerprint` | string | Event fingerprint |
| `updated_at` | datetime | Last transition time |
| `transitioned_by` | string | "action" or "timeout" |

### UI Views
- **Events table**: filterable by namespace, tenant, fingerprint, current state
  - Columns: fingerprint, current state, state machine, updated_at
- **Event detail**: current state, transition history, timeout status
- **Manual transition form**: select target state from allowed transitions
- **State machine visualization**: graph diagram of states, transitions, timeouts

### Write Actions
- Manually transition event state

---

## 6. Event Groups

### Purpose
View and manage batched event groups for noise reduction.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/groups` | List groups (query: namespace, tenant) |
| GET | `/v1/groups/{group_key}` | Get group detail |
| POST | `/v1/groups/{group_key}` | Flush group manually |
| DELETE | `/v1/groups/{group_key}` | Delete group |

### Data Model: `EventGroup`
| Field | Type | Description |
|-------|------|-------------|
| `group_id` | string | UUID v4 |
| `group_key` | string | SHA-256 hash of group_by fields |
| `labels` | HashMap<String,String> | Common labels |
| `events` | GroupedEvent[] | Collected events |
| `notify_at` | datetime | When flush is due |
| `state` | GroupState | Pending/Notified/Resolved |
| `created_at` | datetime | Group creation time |
| `updated_at` | datetime | Last modification |
| `trace_context` | HashMap<String,String> | Propagated trace context |

### `GroupedEvent`
| Field | Type | Description |
|-------|------|-------------|
| `action_id` | ActionId | Original action ID |
| `fingerprint` | string? | Event fingerprint |
| `status` | string? | Status label |
| `payload` | JSON | Event payload |
| `received_at` | datetime | When event was added |

### `GroupState` enum
- `Pending` -- collecting events, not yet flushed
- `Notified` -- flush occurred, notification sent
- `Resolved` -- operator resolved/acknowledged

### UI Views
- **Groups table**: group_key, label summary, event count, state badge, notify_at, created_at
  - Filter by namespace, tenant, state
- **Group detail**: full event list with payloads, labels, timeline
- **Flush button**: trigger immediate flush
- **Delete button**: remove group entirely

### Write Actions
- Flush group (POST)
- Delete group (DELETE)

---

## 7. Task Chains

### Purpose
Monitor and manage multi-step workflow chain executions with conditional branching.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/chains` | List chain executions |
| GET | `/v1/chains/{chain_id}` | Get chain detail |
| POST | `/v1/chains/{chain_id}/cancel` | Cancel running chain |

### Query Parameters (list)
| Param | Type | Description |
|-------|------|-------------|
| `namespace` | string | Required namespace filter |
| `tenant` | string | Required tenant filter |
| `status` | string? | Filter: running/completed/failed/cancelled/timed_out |

### Data Model: `ChainSummary`
| Field | Type | Description |
|-------|------|-------------|
| `chain_id` | string | Execution UUID |
| `chain_name` | string | Config name |
| `status` | string | Current status |
| `current_step` | usize | Current step index |
| `total_steps` | usize | Total steps in chain |
| `started_at` | datetime | Start time |
| `updated_at` | datetime | Last update |

### Data Model: `ChainDetailResponse`
| Field | Type | Description |
|-------|------|-------------|
| `chain_id` | string | Execution UUID |
| `chain_name` | string | Config name |
| `status` | string | Current status |
| `current_step` | usize | Current step index |
| `total_steps` | usize | Total step count |
| `steps` | ChainStepStatus[] | Per-step details |
| `started_at` | datetime | Start time |
| `updated_at` | datetime | Last update |
| `expires_at` | datetime? | Timeout expiry |
| `cancel_reason` | string? | Cancellation reason |
| `cancelled_by` | string? | Who cancelled |
| `execution_path` | string[] | Ordered step names actually executed (branch path) |

### `ChainStepStatus`
| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Step name |
| `provider` | string | Provider used |
| `status` | string | pending/completed/failed/skipped |
| `response_body` | JSON? | Provider response |
| `error` | string? | Error message |
| `completed_at` | datetime? | Completion time |

### Cancel Request
| Field | Type | Description |
|-------|------|-------------|
| `namespace` | string | Namespace |
| `tenant` | string | Tenant |
| `reason` | string? | Cancellation reason |
| `cancelled_by` | string? | Cancelling identity |

### Chain Configuration Model (`ChainConfig`)
| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique chain name |
| `steps` | ChainStepConfig[] | Ordered step definitions |
| `on_failure` | ChainFailurePolicy | abort/abort_no_dlq |
| `timeout_seconds` | u64? | Whole-chain timeout |
| `on_cancel` | ChainNotificationTarget? | Notification on cancel |

### `ChainStepConfig`
| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Step name (unique within chain) |
| `provider` | string | Provider to execute |
| `action_type` | string | Synthetic action type |
| `payload_template` | JSON | Template with `{{...}}` placeholders |
| `on_failure` | StepFailurePolicy? | abort/skip/dlq |
| `delay_seconds` | u64? | Delay before execution |
| `branches` | BranchCondition[] | Conditional next-step routing |
| `default_next` | string? | Fallback next step when no branch matches |

### `BranchCondition`
| Field | Type | Description |
|-------|------|-------------|
| `field` | string | Field to evaluate (e.g., `success`, `body.status`) |
| `operator` | BranchOperator | Eq/Neq/Contains/Exists |
| `value` | JSON? | Comparison value |
| `target` | string | Step name to jump to |

### Template Placeholders
- `{{origin.*}}` -- fields from the triggering action
- `{{prev.*}}` -- previous step result (branching-aware via execution_path)
- `{{steps.NAME.*}}` -- specific named step result

### Validation
- `ChainConfig.validate()` detects: duplicate step names, invalid step references in branches/default_next, cycles in branch graph

### UI Views
- **Chain definitions table**: name, step count, failure policy, timeout
  - Read from server config (no API to list definitions; must be shown from config)
- **Chain executions table**: filterable by namespace, tenant, status
  - Columns: chain_id, chain_name, status badge, progress (current/total), started_at, updated_at
- **Chain detail view**:
  - Step timeline/pipeline visualization
  - Per-step status cards with response/error collapsible
  - Execution path display (branch visualization)
  - Cancel button (for running chains)
  - Expiry countdown (if expires_at set)
- **Chain flow diagram**: visual DAG showing step connections, branches, current position

### Write Actions
- Cancel chain execution

### Configuration Knobs
| Config Path | Type | Default | Description |
|-------------|------|---------|-------------|
| `chains.definitions` | array | [] | Chain definitions |
| `chains.max_concurrent_advances` | usize | 16 | Max concurrent step advances |
| `chains.completed_chain_ttl_seconds` | u64 | 604800 (7d) | TTL for completed chain state |

---

## 8. Approvals

### Purpose
Human-in-the-loop approval workflow for actions flagged by rules.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/approvals` | List pending approvals (query: namespace, tenant) |
| GET | `/v1/approvals/{ns}/{tenant}/{id}` | Get approval status |
| POST | `/v1/approvals/{ns}/{tenant}/{id}/approve` | Approve action |
| POST | `/v1/approvals/{ns}/{tenant}/{id}/reject` | Reject action |

### Data Model: `ApprovalRecord` (internal)
| Field | Type | Description |
|-------|------|-------------|
| `action` | Action | The held action |
| `token` | string | HMAC-signed approval token |
| `rule` | string | Rule that triggered approval |
| `created_at` | datetime | When approval was created |
| `expires_at` | datetime | Expiry deadline |
| `status` | string | pending/approved/rejected/expired |
| `decided_by` | string? | Who approved/rejected |
| `decided_at` | datetime? | Decision timestamp |
| `message` | string? | Approval request message |
| `notification_sent` | bool | Whether initial notification was sent |

### Data Model: `ApprovalStatus` (API response)
| Field | Type | Description |
|-------|------|-------------|
| `approval_id` | string | Approval token |
| `action_id` | string | Held action ID |
| `status` | string | Current status |
| `rule` | string | Triggering rule name |
| `message` | string? | Approval message |
| `created_at` | datetime | Creation time |
| `expires_at` | datetime | Deadline |
| `decided_by` | string? | Decision maker |
| `decided_at` | datetime? | Decision time |

### HMAC Key Rotation
- `ApprovalKeySet`: signing key + verification keys (supports rotation)
- Configured via `server.approval_keys` (array of `{id, secret}`)
- Fallback: single `server.approval_secret`
- If neither set, random key generated on startup (tokens don't survive restart)

### Approval Notification Retry
- Background processor sweeps for pending approvals with `notification_sent == false`
- Emits `ApprovalRetryEvent` to retry notification delivery

### UI Views
- **Pending approvals table**: filterable by namespace, tenant
  - Columns: action_id, rule, message, status badge, created_at, expires_at, countdown
- **Approval detail**: full action payload, rule details, approve/reject buttons
- **Approval action buttons**: large approve (green) and reject (red) buttons
- **Approval history**: completed/expired approvals

### Write Actions
- Approve action
- Reject action

### Configuration Knobs
| Config Path | Type | Default | Description |
|-------------|------|---------|-------------|
| `server.approval_secret` | string? | random | Single HMAC secret |
| `server.approval_keys` | array? | none | Multiple named HMAC keys for rotation |
| `server.external_url` | string? | localhost | Base URL for approval links |

---

## 9. Circuit Breakers

### Purpose
Monitor and control provider circuit breakers for resilience.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| GET | `/admin/circuit-breakers` | List all circuit breakers with state |
| POST | `/admin/circuit-breakers/{provider}/trip` | Force-open a circuit |
| POST | `/admin/circuit-breakers/{provider}/reset` | Force-close a circuit |

### Data Model: `CircuitBreakerStatus` (API response)
| Field | Type | Description |
|-------|------|-------------|
| `provider` | string | Provider name |
| `state` | string | closed/open/half_open |
| `config` | CircuitBreakerConfig | Thresholds and settings |

### `CircuitBreakerConfig`
| Field | Type | Description |
|-------|------|-------------|
| `failure_threshold` | u32 | Failures before opening |
| `success_threshold` | u32 | Successes to close from half-open |
| `recovery_timeout` | Duration | Time before open -> half-open |
| `fallback_provider` | string? | Where to route when open |

### `CircuitState` enum
- `Closed` -- normal operation
- `Open` -- rejecting requests, waiting for recovery timeout
- `HalfOpen` -- testing with limited requests

### UI Views
- **Circuit breakers dashboard**: card per provider
  - State indicator (green/red/yellow for closed/open/half_open)
  - Config summary: thresholds, recovery timeout, fallback
  - Trip/Reset buttons (admin only)
- **Circuit breaker detail**: failure/success counters, state transition history
- **State transition timeline**: visual history of circuit state changes

### Write Actions
- Trip circuit (force open) -- requires `CircuitBreakerManage` permission
- Reset circuit (force close) -- requires `CircuitBreakerManage` permission

### Configuration Knobs
| Config Path | Type | Default | Description |
|-------------|------|---------|-------------|
| `circuit_breaker.enabled` | bool | false | Enable circuit breakers |
| `circuit_breaker.failure_threshold` | u32 | 5 | Default failures to open |
| `circuit_breaker.success_threshold` | u32 | 2 | Default successes to close |
| `circuit_breaker.recovery_timeout_seconds` | u64 | 60 | Default recovery timeout |
| `circuit_breaker.providers.{name}.failure_threshold` | u32? | inherit | Per-provider override |
| `circuit_breaker.providers.{name}.success_threshold` | u32? | inherit | Per-provider override |
| `circuit_breaker.providers.{name}.recovery_timeout_seconds` | u64? | inherit | Per-provider override |
| `circuit_breaker.providers.{name}.fallback_provider` | string? | none | Fallback provider |

---

## 10. Dead-Letter Queue

### Purpose
View and drain actions that exhausted all retry attempts.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/dlq/stats` | DLQ stats (enabled flag + entry count) |
| POST | `/v1/dlq/drain` | Drain all DLQ entries |

### Data Model: `DeadLetterEntry`
| Field | Type | Description |
|-------|------|-------------|
| `action` | Action | The failed action |
| `error` | string | Final error message |
| `attempts` | u32 | Total execution attempts |
| `timestamp` | SystemTime | When entry was created |

### DLQ Stats Response
| Field | Type | Description |
|-------|------|-------------|
| `enabled` | bool | Whether DLQ is active |
| `count` | usize | Number of entries |

### UI Views
- **DLQ stats card**: enabled badge, entry count
- **DLQ entries table**: action_id, provider, action_type, error, attempts, timestamp
- **Drain button**: clear all entries (with confirmation dialog)
- **Entry detail**: full action payload, error details

### Write Actions
- Drain DLQ (destructive, requires confirmation)

### Configuration Knobs
| Config Path | Type | Default | Description |
|-------------|------|---------|-------------|
| `executor.dlq_enabled` | bool | false | Enable dead-letter queue |

---

## 11. Real-Time Stream (SSE)

### Purpose
Server-Sent Events stream for live action and event monitoring.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/stream` | SSE event stream |

### Query Parameters
| Param | Type | Description |
|-------|------|-------------|
| `namespace` | string | Required namespace filter |
| `tenant` | string | Required tenant filter |
| `action_type` | string? | Filter by action type |
| `outcome` | string? | Filter by outcome category |
| `event_type` | string? | Filter by StreamEventType |

### Headers
| Header | Description |
|--------|-------------|
| `Last-Event-ID` | Resume from specific event ID (catch-up replay from audit) |

### Data Model: `StreamEvent`
| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID v7 | Monotonic event ID (sortable) |
| `timestamp` | datetime | Event timestamp |
| `event_type` | StreamEventType | Event category |
| `namespace` | string | Namespace |
| `tenant` | string | Tenant |
| `action_type` | string | Action type |
| `action_id` | ActionId | Related action ID |

### `StreamEventType` enum
- `ActionDispatched` -- new action processed
- `GroupFlushed` -- event group was flushed
- `Timeout` -- state machine timeout fired
- `ChainAdvanced` -- chain step completed
- `ApprovalRequired` -- new approval request
- `ScheduledActionDue` -- scheduled action ready

### Connection Management
- `ConnectionRegistry` tracks active SSE connections per tenant
- Max connections per tenant configurable via `server.max_sse_connections_per_tenant`
- Default: 10 connections per tenant
- `Last-Event-ID` catch-up: replays from audit store if available

### UI Integration
- **Live activity feed**: real-time event stream displayed as a scrolling log
- **Filtered views**: connect with namespace/tenant/event_type filters
- **Connection status indicator**: show connected/disconnected state
- **Catch-up on reconnect**: use Last-Event-ID to resume

### Configuration Knobs
| Config Path | Type | Default | Description |
|-------------|------|---------|-------------|
| `server.max_sse_connections_per_tenant` | usize? | 10 | Max SSE connections per tenant |

---

## 12. Embedding / Semantic Routing

### Purpose
Compute semantic similarity for content-based routing decisions.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/embeddings/similarity` | Compute cosine similarity |

### Request: `SimilarityRequest`
| Field | Type | Description |
|-------|------|-------------|
| `text` | string | Text to compare |
| `topic` | string | Topic to compare against |

### Response: `SimilarityResponse`
| Field | Type | Description |
|-------|------|-------------|
| `similarity` | f64 | Cosine similarity score [0.0, 1.0] |
| `topic` | string | The compared topic |

### Rate Limiting
- Tight per-caller limit: 5 requests per 60 seconds
- Uses custom rate limit bucket `embedding:{caller_id}`

### UI Views
- **Embedding tester**: form with text and topic inputs, similarity score output
- **Embedding metrics**: cache hit rates, request counts, error counts (from dashboard)
- **Configuration display**: provider, model, cache settings

### Configuration Knobs
| Config Path | Type | Default | Description |
|-------------|------|---------|-------------|
| `embedding.enabled` | bool | false | Enable embedding provider |
| `embedding.endpoint` | string | OpenAI embeddings URL | API endpoint |
| `embedding.model` | string | "text-embedding-3-small" | Model name |
| `embedding.api_key` | string | "" | API key (supports ENC[...]) |
| `embedding.timeout_seconds` | u64 | 10 | Request timeout |
| `embedding.fail_open` | bool | true | Allow on API failure |
| `embedding.topic_cache_capacity` | u64 | 10000 | Topic cache size |
| `embedding.topic_cache_ttl_seconds` | u64 | 3600 | Topic cache TTL |
| `embedding.text_cache_capacity` | u64 | 1000 | Text cache size |
| `embedding.text_cache_ttl_seconds` | u64 | 60 | Text cache TTL |

---

## 13. Rate Limiting

### Purpose
Per-caller and per-tenant request rate limiting using sliding window approximation.

### Architecture
- Distributed rate limiter backed by `StateStore` (works across server instances)
- Sliding window approximation algorithm (Cloudflare-style, ~2% error margin)
- Three tiers: caller default, caller anonymous, per-tenant
- Per-caller overrides by caller ID
- Per-tenant overrides by tenant ID
- Configurable fail-open/fail-closed on state store errors

### Data Model: `RateLimitTier`
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `requests_per_window` | u64 | 1000 | Max requests per window |
| `window_seconds` | u64 | 60 | Window duration |

### Tier Hierarchy
| Tier | Default | Description |
|------|---------|-------------|
| Caller default | 1000/60s | Authenticated callers |
| Caller anonymous | 100/60s | Unauthenticated callers |
| Tenant default | 10000/60s | Per-tenant (if enabled) |
| Caller overrides | per-caller | Specific caller overrides |
| Tenant overrides | per-tenant | Specific tenant overrides |

### Rate Limit Response Headers (when exceeded)
- HTTP 429 with `retry_after` seconds

### Rate Limit Result
| Field | Type | Description |
|-------|------|-------------|
| `allowed` | bool | Whether request is allowed |
| `limit` | u64 | Configured limit |
| `remaining` | u64 | Approximate remaining |
| `reset_after` | u64 | Seconds until window resets |

### UI Views
- **Rate limit configuration display**: tiers, overrides
- **Rate limit status**: current window usage per caller/tenant (would require new API)
- **Override management**: view configured overrides

### Configuration Knobs
| Config Path | Type | Default | Description |
|-------------|------|---------|-------------|
| `rate_limit.enabled` | bool | false | Enable rate limiting |
| `rate_limit.config_path` | string? | none | Path to ratelimit.toml |
| `rate_limit.on_error` | string | "allow" | Behavior on store error: allow/deny |

### Rate Limit File Config (`ratelimit.toml`)
| Section | Field | Default | Description |
|---------|-------|---------|-------------|
| `callers.default` | requests_per_window | 1000 | Default caller limit |
| `callers.default` | window_seconds | 60 | Default window |
| `callers.anonymous` | requests_per_window | 100 | Anonymous limit |
| `callers.anonymous` | window_seconds | 60 | Anonymous window |
| `callers.overrides.{id}` | (tier) | - | Per-caller override |
| `tenants.enabled` | bool | false | Enable per-tenant limits |
| `tenants.default` | requests_per_window | 10000 | Default tenant limit |
| `tenants.default` | window_seconds | 60 | Default window |
| `tenants.overrides.{id}` | (tier) | - | Per-tenant override |

---

## 14. Authentication & Authorization

### Purpose
JWT + API key authentication with RBAC and grant-based tenant/namespace scoping.

### API Endpoints
| Method | Path | Description |
|--------|------|-------------|
| POST | `/v1/auth/login` | Authenticate and receive JWT |
| POST | `/v1/auth/logout` | Revoke JWT token |

### Authentication Methods
1. **JWT**: username/password login, token in `Authorization: Bearer` header
2. **API Key**: pre-shared key in `Authorization: Bearer` header (hash-based lookup)
3. **Anonymous**: when auth is disabled, full admin access

### Data Model: `CallerIdentity`
| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Username or API key name |
| `role` | Role | Admin/Operator/Viewer |
| `grants` | Grant[] | Resource access grants |
| `auth_method` | string | jwt/api_key/anonymous |

### RBAC Roles
| Role | Permissions |
|------|-------------|
| Admin | Dispatch, AuditRead, RulesManage, RulesRead, CircuitBreakerManage, StreamSubscribe |
| Operator | Dispatch, AuditRead, RulesManage, RulesRead, CircuitBreakerManage, StreamSubscribe |
| Viewer | AuditRead, RulesRead, StreamSubscribe |

### Permission Enum
| Permission | Description |
|------------|-------------|
| `Dispatch` | Send actions through the pipeline |
| `AuditRead` | Read audit trail records |
| `RulesManage` | Reload/toggle rules |
| `RulesRead` | List rules |
| `CircuitBreakerManage` | Trip/reset circuit breakers |
| `StreamSubscribe` | Connect to SSE stream |

### Grant Model
| Field | Type | Description |
|-------|------|-------------|
| `tenants` | string[] | Allowed tenants (supports `"*"` wildcard) |
| `namespaces` | string[] | Allowed namespaces (supports `"*"` wildcard) |
| `actions` | string[] | Allowed action types (supports `"*"` wildcard) |

### Auth Provider Features
- **Hot-reload**: users and API keys can be reloaded without restart
- **File watcher**: auto-reload on auth.toml changes (configurable)
- **JWT token revocation**: stored in StateStore for distributed blacklisting
- **Password hashing**: bcrypt verification

### UI Views
- **Login page**: username/password form
- **User management display**: list users with roles and grants (read from config)
- **API key management display**: list API keys with roles and grants (read from config)
- **Session info**: current user identity, role, grants, auth method
- **Logout button**: revoke current JWT

### Configuration Knobs
| Config Path | Type | Default | Description |
|-------------|------|---------|-------------|
| `auth.enabled` | bool | false | Enable authentication |
| `auth.config_path` | string? | none | Path to auth.toml |
| `auth.watch` | bool? | true | Watch auth file for changes |

### Auth File Config (`auth.toml`)
| Section | Field | Description |
|---------|-------|-------------|
| `settings.jwt_secret` | string | JWT signing secret |
| `settings.jwt_expiry_seconds` | u64 | Token expiry duration |
| `users[].username` | string | User login name |
| `users[].password_hash` | string | Bcrypt hash |
| `users[].role` | string | admin/operator/viewer |
| `users[].grants` | Grant[] | Resource grants |
| `api_keys[].name` | string | Key display name |
| `api_keys[].key_hash` | string | SHA-256 hex hash |
| `api_keys[].role` | string | admin/operator/viewer |
| `api_keys[].grants` | Grant[] | Resource grants |

---

## 15. Providers

### Purpose
Named provider endpoints that actions are routed to.

### Provider Types
| Type | Description | Config Fields |
|------|-------------|---------------|
| `webhook` | HTTP POST to a URL | `url` (required), `headers` (optional) |
| `log` | Logs action and returns success | (none) |

### Provider Config (`ProviderConfig`)
| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique provider name |
| `provider_type` | string | "webhook" or "log" |
| `url` | string? | Target URL (webhook) |
| `headers` | HashMap<String,String> | Extra HTTP headers (webhook) |

### Provider Response Model
| Field | Type | Description |
|-------|------|-------------|
| `status` | ResponseStatus | Success/Failure/Partial |
| `body` | JSON? | Response body |
| `headers` | HashMap<String,String> | Response headers |

### UI Views
- **Providers table**: name, type, URL, header count
- **Provider detail**: full URL, headers (masked sensitive values), circuit breaker status link
- **Provider health indicator**: circuit breaker state overlay

### Configuration (via `acteon.toml`)
```toml
[[providers]]
name = "email"
type = "webhook"
url = "http://localhost:9999/webhook"
[providers.headers]
Authorization = "Bearer token"
```

---

## 16. Metrics

### Full Metrics Catalog

### Gateway Metrics (`GatewayMetrics` -- 19 atomic counters)
| Counter | Description | Dashboard Category |
|---------|-------------|-------------------|
| `dispatched` | Total actions dispatched | Core |
| `executed` | Successfully executed | Core |
| `deduplicated` | Suppressed by dedup | Core |
| `suppressed` | Suppressed by rules | Core |
| `rerouted` | Rerouted to alternate provider | Core |
| `throttled` | Rejected by throttle | Core |
| `failed` | Failed execution | Core |
| `pending_approval` | Awaiting approval | Workflow |
| `llm_guardrail_allowed` | LLM allowed | LLM |
| `llm_guardrail_denied` | LLM denied | LLM |
| `llm_guardrail_errors` | LLM errors | LLM |
| `chains_started` | Chains started | Chains |
| `chains_completed` | Chains completed | Chains |
| `chains_failed` | Chains failed | Chains |
| `chains_cancelled` | Chains cancelled | Chains |
| `circuit_open` | Actions rejected by open circuit | Resilience |
| `circuit_transitions` | Circuit state transitions | Resilience |
| `circuit_fallbacks` | Fallback invocations | Resilience |
| `scheduled` | Actions scheduled | Workflow |

### Embedding Metrics (separate)
| Counter | Description |
|---------|-------------|
| `similarity_requests` | Total similarity API requests |
| `similarity_errors` | Failed similarity requests |
| `topic_cache_hits` | Topic embedding cache hits |
| `topic_cache_misses` | Topic cache misses |
| `text_cache_hits` | Text embedding cache hits |
| `text_cache_misses` | Text cache misses |

---

## 17. Server Configuration Reference

### Full `acteon.toml` Schema

| Section | Key | Type | Default | Description |
|---------|-----|------|---------|-------------|
| **state** | backend | string | "memory" | State backend: memory/redis/postgres/dynamodb/clickhouse |
| | url | string? | none | Connection URL |
| | prefix | string? | "acteon" | Key prefix |
| | region | string? | none | AWS region (DynamoDB) |
| | table_name | string? | none | DynamoDB table |
| **rules** | directory | string? | none | YAML rules directory |
| | default_timezone | string? | UTC | Default timezone |
| **executor** | max_retries | u32? | 3 | Max retry attempts |
| | timeout_seconds | u64? | 30 | Execution timeout |
| | max_concurrent | usize? | 10 | Concurrency limit |
| | dlq_enabled | bool | false | Enable DLQ |
| **server** | host | string | "127.0.0.1" | Bind address |
| | port | u16 | 8080 | Bind port |
| | shutdown_timeout_seconds | u64 | 30 | Graceful shutdown timeout |
| | external_url | string? | localhost | External URL for links |
| | approval_secret | string? | random | HMAC signing secret |
| | approval_keys | array? | none | Multi-key HMAC rotation |
| | max_sse_connections_per_tenant | usize? | 10 | SSE connection limit |
| **audit** | enabled | bool | false | Enable audit |
| | backend | string | "memory" | Backend type |
| | url | string? | none | Connection URL |
| | prefix | string | "acteon_" | Table prefix |
| | ttl_seconds | u64? | 2592000 | Record TTL (30 days) |
| | cleanup_interval_seconds | u64 | 3600 | Cleanup interval |
| | store_payload | bool | true | Store payloads |
| | redact.enabled | bool | false | Enable redaction |
| | redact.fields | string[] | [] | Fields to redact |
| | redact.placeholder | string | "[REDACTED]" | Replacement |
| **auth** | enabled | bool | false | Enable auth |
| | config_path | string? | none | auth.toml path |
| | watch | bool? | true | Watch for changes |
| **rate_limit** | enabled | bool | false | Enable rate limiting |
| | config_path | string? | none | ratelimit.toml path |
| | on_error | string | "allow" | Error behavior |
| **background** | enabled | bool | false | Enable background processing |
| | group_flush_interval_seconds | u64 | 5 | Group flush check interval |
| | timeout_check_interval_seconds | u64 | 10 | Timeout check interval |
| | cleanup_interval_seconds | u64 | 60 | Cleanup interval |
| | enable_group_flush | bool | true | Auto-flush groups |
| | enable_timeout_processing | bool | true | Process timeouts |
| | enable_approval_retry | bool | true | Retry failed notifications |
| | enable_scheduled_actions | bool | false | Process scheduled actions |
| | scheduled_check_interval_seconds | u64 | 5 | Scheduled action check interval |
| | namespace | string | "" | Namespace for timeouts |
| | tenant | string | "" | Tenant for timeouts |
| **llm_guardrail** | enabled | bool | false | Enable LLM guardrail |
| | endpoint | string | OpenAI URL | API endpoint |
| | model | string | "gpt-4o-mini" | Model name |
| | api_key | string | "" | API key (supports ENC[...]) |
| | policy | string | "" | Global system prompt |
| | policies | map | {} | Per-action-type policies |
| | fail_open | bool | true | Allow on LLM failure |
| | timeout_seconds | u64? | 10 | Request timeout |
| | temperature | f64? | 0.0 | Sampling temperature |
| | max_tokens | u32? | 256 | Max response tokens |
| **embedding** | enabled | bool | false | Enable embeddings |
| | endpoint | string | OpenAI URL | API endpoint |
| | model | string | "text-embedding-3-small" | Model |
| | api_key | string | "" | API key (supports ENC[...]) |
| | timeout_seconds | u64 | 10 | Timeout |
| | fail_open | bool | true | Allow on failure |
| | topic_cache_capacity | u64 | 10000 | Topic cache size |
| | topic_cache_ttl_seconds | u64 | 3600 | Topic cache TTL |
| | text_cache_capacity | u64 | 1000 | Text cache size |
| | text_cache_ttl_seconds | u64 | 60 | Text cache TTL |
| **circuit_breaker** | enabled | bool | false | Enable circuit breakers |
| | failure_threshold | u32 | 5 | Failures to open |
| | success_threshold | u32 | 2 | Successes to close |
| | recovery_timeout_seconds | u64 | 60 | Recovery timeout |
| | providers.{name} | object | - | Per-provider overrides |
| **telemetry** | enabled | bool | false | Enable OpenTelemetry |
| | endpoint | string | "http://localhost:4317" | OTLP endpoint |
| | service_name | string | "acteon" | Service name |
| | sample_ratio | f64 | 1.0 | Sampling ratio |
| | protocol | string | "grpc" | Transport: grpc/http |
| | timeout_seconds | u64 | 10 | Exporter timeout |
| | resource_attributes | map | {} | Extra resource attrs |
| **providers** | (array) | - | - | Provider definitions |

---

## 18. Background Processing

### Purpose
Periodic background tasks for group flushing, timeout processing, chain advancement, scheduled actions, and approval retry.

### Background Tasks
| Task | Interval Default | Config Toggle | Description |
|------|-----------------|---------------|-------------|
| Group flush | 5s | `enable_group_flush` | Flush ready event groups |
| Timeout processing | 10s | `enable_timeout_processing` | Fire state machine timeouts |
| Chain advancement | 5s | `enable_chain_advancement` | Advance pending chain steps |
| Scheduled actions | 5s | `enable_scheduled_actions` | Dispatch due scheduled actions |
| Approval retry | 60s (cleanup) | `enable_approval_retry` | Retry unsent approval notifications |
| Cleanup | 60s | always on | Clean up resolved groups |

### Background Events
| Event Type | Description |
|------------|-------------|
| `GroupFlushEvent` | Group was flushed (group data + timestamp) |
| `TimeoutEvent` | State machine timeout fired (fingerprint, states, trace context) |
| `ChainAdvanceEvent` | Chain ready for next step (namespace, tenant, chain_id) |
| `ScheduledActionDueEvent` | Scheduled action ready (full action data) |
| `ApprovalRetryEvent` | Approval needs notification retry |

### UI Views
- **Background processing status panel**: shows which tasks are enabled/disabled
- **Task activity indicators**: last run time, events processed count
- **Enable/disable toggles**: (read-only display from config)

---

## 19. LLM Guardrail

### Purpose
AI-powered content safety evaluation for action payloads.

### Architecture
- Three-level policy resolution (most specific wins):
  1. Rule metadata `llm_policy` key (per-rule)
  2. Per-action-type policy map (`llm_guardrail.policies`)
  3. Global default (`llm_guardrail.policy`)

### LLM Evaluator Interface
```
evaluate(action, policy) -> LlmGuardrailResponse { allowed: bool, reason: string }
```

### `LlmGuardrailConfig`
| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `endpoint` | string | OpenAI URL | API endpoint |
| `model` | string | "gpt-4o-mini" | Model name |
| `api_key` | string | - | API key |
| `timeout_seconds` | u64 | 10 | Request timeout |
| `temperature` | f64 | 0.0 | Sampling temperature |
| `max_tokens` | u32 | 256 | Max response tokens |

### Fail-Open Behavior
When `fail_open = true` (default), actions are allowed through when the LLM is unreachable.

### Metrics
- `llm_guardrail_allowed` -- actions allowed
- `llm_guardrail_denied` -- actions denied
- `llm_guardrail_errors` -- evaluation errors

### UI Views
- **LLM guardrail configuration display**: endpoint, model, policies
- **Policy editor display**: global policy, per-action-type policies, per-rule metadata policies
- **Guardrail metrics**: allowed/denied/error rates (from dashboard)

### Configuration
See `llm_guardrail` section in [Server Configuration Reference](#17-server-configuration-reference).

---

## 20. Telemetry / Tracing

### Purpose
OpenTelemetry distributed tracing for end-to-end pipeline visibility.

### Trace Spans Cover
- HTTP ingress (request/response)
- Rule evaluation
- State store operations
- Provider execution
- Audit recording
- Chain step advancement

### Configuration
See `telemetry` section in [Server Configuration Reference](#17-server-configuration-reference).

### UI Views
- **Telemetry configuration display**: enabled, endpoint, service name, sample ratio, protocol
- **Link to external trace UI**: deep-link to Jaeger/Tempo/etc. using trace IDs from audit records

---

## Appendix A: State Store Backends

| Backend | Crate | Use Case |
|---------|-------|----------|
| Memory | `acteon-state-memory` | Development/testing |
| Redis | `acteon-state-redis` | Production (single-node or cluster) |
| PostgreSQL | `acteon-state-postgres` | Production (SQL-based) |
| DynamoDB | `acteon-state-dynamodb` | AWS serverless |
| ClickHouse | `acteon-state-clickhouse` | Analytics-heavy workloads |

### StateStore Trait Methods
| Method | Description |
|--------|-------------|
| `check_and_set` | Atomic set-if-not-exists with TTL |
| `get` | Read value by key |
| `set` | Write value with optional TTL |
| `delete` | Remove key |
| `increment` | Atomic counter increment |
| `compare_and_swap` | CAS with version check |
| `scan_keys` | Scan by namespace/tenant/kind/prefix |
| `scan_keys_by_kind` | Scan all keys of a kind globally |
| `index_timeout` | Add to timeout index |
| `remove_timeout_index` | Remove from timeout index |
| `get_expired_timeouts` | Get expired timeout keys |
| `index_chain_ready` | Add chain to ready index |
| `remove_chain_ready_index` | Remove chain from ready index |
| `get_ready_chains` | Get chains ready for advancement |

### KeyKind Enum (17 kinds)
| Kind | Description |
|------|-------------|
| Dedup | Deduplication state |
| Counter | Atomic counters |
| Lock | Distributed locks |
| State | Generic state |
| History | History records |
| RateLimit | Rate limit counters |
| EventState | State machine position |
| EventTimeout | Timeout tracking |
| Group | Event group data |
| PendingGroups | Pending group index |
| ActiveEvents | Active event index |
| Approval | Approval records |
| PendingApprovals | Pending approval index |
| Chain | Chain execution state |
| PendingChains | Pending chain index |
| ScheduledAction | Scheduled action data |
| PendingScheduled | Pending scheduled index |

---

## Appendix B: Audit Store Backends

| Backend | Crate | Description |
|---------|-------|-------------|
| Memory | `acteon-audit-memory` | In-memory (dev/test) |
| PostgreSQL | `acteon-audit-postgres` | SQL with migrations, cleanup |
| ClickHouse | `acteon-audit-clickhouse` | Column-oriented analytics |
| Elasticsearch | `acteon-audit-elasticsearch` | Full-text search |

### AuditStore Trait Methods
| Method | Description |
|--------|-------------|
| `record` | Persist audit record |
| `get_by_action_id` | Fetch by action ID |
| `get_by_id` | Fetch by audit record ID |
| `query` | Search with filters + pagination |
| `cleanup_expired` | Remove expired records |

---

## Appendix C: Complete API Route Map

| Method | Path | Handler | Auth | Permission |
|--------|------|---------|------|------------|
| GET | `/health` | `health` | no | - |
| GET | `/metrics` | `metrics` | no | - |
| POST | `/v1/auth/login` | `login` | no | - |
| POST | `/v1/auth/logout` | `logout` | yes | - |
| POST | `/v1/dispatch` | `dispatch` | yes | Dispatch |
| POST | `/v1/dispatch/batch` | `dispatch_batch` | yes | Dispatch |
| GET | `/v1/rules` | `list_rules` | yes | RulesRead |
| POST | `/v1/rules/reload` | `reload_rules` | yes | RulesManage |
| PUT | `/v1/rules/{name}/enabled` | `set_rule_enabled` | yes | RulesManage |
| GET | `/v1/audit` | `query_audit` | yes | AuditRead |
| GET | `/v1/audit/{action_id}` | `get_audit_record` | yes | AuditRead |
| POST | `/v1/audit/{action_id}/replay` | `replay_action` | yes | Dispatch |
| POST | `/v1/audit/replay` | `replay_audit` | yes | Dispatch |
| GET | `/v1/dlq/stats` | `dlq_stats` | yes | AuditRead |
| POST | `/v1/dlq/drain` | `dlq_drain` | yes | Dispatch |
| GET | `/v1/events` | `list_events` | yes | AuditRead |
| GET | `/v1/events/{fingerprint}` | `get_event` | yes | AuditRead |
| POST | `/v1/events/{fingerprint}/transition` | `transition_event` | yes | Dispatch |
| GET | `/v1/groups` | `list_groups` | yes | AuditRead |
| GET | `/v1/groups/{group_key}` | `get_group` | yes | AuditRead |
| POST | `/v1/groups/{group_key}` | `flush_group` | yes | Dispatch |
| DELETE | `/v1/groups/{group_key}` | `delete_group` | yes | Dispatch |
| GET | `/v1/chains` | `list_chains` | yes | AuditRead |
| GET | `/v1/chains/{chain_id}` | `get_chain` | yes | AuditRead |
| POST | `/v1/chains/{chain_id}/cancel` | `cancel_chain` | yes | Dispatch |
| POST | `/v1/embeddings/similarity` | `similarity` | yes | Dispatch |
| GET | `/v1/approvals` | `list_approvals` | yes | AuditRead |
| GET | `/v1/approvals/{ns}/{tenant}/{id}` | `get_approval` | no* | - |
| POST | `/v1/approvals/{ns}/{tenant}/{id}/approve` | `approve_action` | no* | - |
| POST | `/v1/approvals/{ns}/{tenant}/{id}/reject` | `reject_action` | no* | - |
| GET | `/admin/circuit-breakers` | `list_circuit_breakers` | yes | CircuitBreakerManage |
| POST | `/admin/circuit-breakers/{provider}/trip` | `trip_circuit_breaker` | yes | CircuitBreakerManage |
| POST | `/admin/circuit-breakers/{provider}/reset` | `reset_circuit_breaker` | yes | CircuitBreakerManage |
| GET | `/v1/stream` | `stream` | yes | StreamSubscribe |

*Approval endpoints use HMAC token authentication instead of JWT/API key.

---

## Appendix D: UI Navigation Structure (Recommended)

```
Sidebar Navigation:
  Dashboard           -> Section 1
  Dispatch            -> Section 2
  Rules               -> Section 3
  Audit Trail         -> Section 4
  Events              -> Section 5
  Groups              -> Section 6
  Chains              -> Section 7
  Approvals           -> Section 8
  Circuit Breakers    -> Section 9
  Dead-Letter Queue   -> Section 10
  Stream (Live)       -> Section 11
  Embeddings          -> Section 12
  Settings
    Rate Limiting     -> Section 13
    Auth & Users      -> Section 14
    Providers         -> Section 15
    LLM Guardrail     -> Section 19
    Telemetry         -> Section 20
    Server Config     -> Section 17
    Background Tasks  -> Section 18
```

---

## Appendix E: Role-Based View Restrictions

| View | Admin | Operator | Viewer |
|------|-------|----------|--------|
| Dashboard | Full | Full | Full (read) |
| Dispatch | Full | Full | Hidden |
| Rules | Full (manage) | Full (manage) | Read-only |
| Audit Trail | Full + replay | Full + replay | Read-only |
| Events | Full + transition | Full + transition | Read-only |
| Groups | Full + flush/delete | Full + flush/delete | Read-only |
| Chains | Full + cancel | Full + cancel | Read-only |
| Approvals | Full | Full | Read-only |
| Circuit Breakers | Full + trip/reset | Full + trip/reset | Hidden |
| DLQ | Full + drain | Full + drain | Read-only (stats) |
| Stream | Subscribe | Subscribe | Subscribe |
| Embeddings | Full | Full | Hidden |
| Settings | Full | Read-only | Hidden |
