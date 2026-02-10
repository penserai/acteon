# Action Status Subscriptions — Design Document

**Status**: Implemented
**Author**: Technical Architect
**Date**: 2026-02-09

## 1. Overview

Action Status Subscriptions allow clients to subscribe to real-time updates for a specific entity — an action, a chain execution, or an event group — via Server-Sent Events (SSE). Unlike the existing `GET /v1/stream` broadcast endpoint (which emits all gateway events with optional query-param filtering), subscriptions are entity-scoped: a client subscribes to *one* action ID, chain ID, or group ID and receives only events relevant to that entity.

### Motivation

The existing `GET /v1/stream` endpoint is designed for operational dashboards that monitor the full event firehose. It is not well-suited for use cases where a client dispatches an action and wants to track its progress:

- **Chain tracking**: After dispatching an action that starts a chain, the caller wants step-by-step progress updates and the final completion/failure status.
- **Approval tracking**: After dispatching an action that requires approval, the caller wants to know when it is approved or rejected.
- **Group tracking**: After adding an event to a group, the caller wants to know when the group is flushed or resolved.
- **State machine tracking**: After a state transition, the caller wants to follow subsequent transitions for the same entity.

---

## 2. New SSE Endpoint

### 2.1 Route

```
GET /v1/subscribe/{entity_type}/{entity_id}
```

Where:
- `entity_type` is one of: `action`, `chain`, `group`
- `entity_id` is the specific ID (action ID, chain execution ID, or group ID)

### 2.2 Query Parameters

| Parameter    | Type   | Description |
|-------------|--------|-------------|
| `include_history` | bool | Optional (default: `true`). Emit synthetic catch-up events for the entity's current state before switching to live stream. |
| `namespace` | string | Required for `chain` and `group` subscriptions. Namespace for tenant isolation. |
| `tenant` | string | Required for `chain` and `group` subscriptions. Tenant for tenant isolation. |

### 2.3 Authentication and Authorization

- Requires valid Bearer token or API key (same `AuthLayer` as `/v1/stream`).
- Requires `Permission::StreamSubscribe` (same as existing stream).
- Tenant isolation: the server validates that the entity belongs to a tenant the caller is authorized to see. For `chain` and `group` subscriptions, the entity's namespace/tenant is read from the state store and checked against the caller's grants. For `action` subscriptions, the `namespace` and `tenant` query params are required.

### 2.4 Entity ID Validation

Before establishing the SSE connection, the handler validates that the entity exists:

- **`chain`**: Load `ChainState` from the state store via `gateway.get_chain_status()`. If not found or wrong tenant, return `403` ("forbidden or not found"). Both cases use the same status code and message to avoid leaking entity existence (security review C1).
- **`group`**: Look up the group via `GroupManager::get_group()`. If not found or wrong tenant, return `403`.
- **`action`**: Actions are ephemeral (no persistent state beyond audit records). No entity validation is performed. The subscription is valid immediately — events are filtered by `action_id` on the `StreamEvent.action_id` field. If `include_history=true`, the handler queries the audit store via `get_by_action_id()` for catch-up.

### 2.5 Response Format

The response is a standard SSE stream. Each event has the form:

```
id: <UUIDv7>
event: <event_type_tag>
data: <JSON StreamEvent>

```

The `StreamEvent` payload reuses the existing `StreamEvent` struct with the new `StreamEventType` variants (Section 3).

When the entity reaches a terminal state (chain completed/failed/cancelled/timed_out, group resolved), a final event is emitted with `event: subscription_end` and the stream closes.

```
event: subscription_end
data: {"reason":"chain_completed","entity_type":"chain","entity_id":"abc-123"}

```

### 2.6 Connection Lifecycle

1. Client connects to `GET /v1/subscribe/chain/{chain_id}`.
2. Server validates auth, tenant isolation, and entity existence.
3. If `include_history=true` (default), server emits synthetic catch-up events from the entity's current state.
4. Server subscribes to the global broadcast channel with an entity-scoped filter.
5. Events flow until the entity reaches a terminal state or the client disconnects.
6. On terminal state, server emits `subscription_end` and closes.

### 2.7 Per-Tenant Connection Limits

Subscription connections share the same `ConnectionRegistry` and per-tenant limits as the existing `/v1/stream` endpoint. Each subscription counts as one connection slot.

---

## 3. New `StreamEventType` Variants

Six new variants are added to `StreamEventType` in `crates/core/src/stream.rs`:

```rust
/// A chain step completed (successfully or via skip).
ChainStepCompleted {
    /// The chain execution ID.
    chain_id: String,
    /// Name of the completed step.
    step_name: String,
    /// Index of the completed step (0-based).
    step_index: usize,
    /// Whether the step succeeded.
    success: bool,
    /// Name of the next step to execute, if any.
    next_step: Option<String>,
},

/// A chain execution reached a terminal state.
ChainCompleted {
    /// The chain execution ID.
    chain_id: String,
    /// Terminal status: `completed`, `failed`, `cancelled`, `timed_out`.
    status: String,
    /// The execution path taken through the chain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    execution_path: Vec<String>,
},

/// An event was added to a group.
GroupEventAdded {
    /// The group identifier.
    group_id: String,
    /// The group key used for aggregation.
    group_key: String,
    /// Current number of events in the group after this addition.
    event_count: usize,
},

/// An event group was resolved (manually or after flush).
GroupResolved {
    /// The group identifier.
    group_id: String,
    /// The group key.
    group_key: String,
},

/// A state machine transition occurred for an action's entity.
ActionStatusChanged {
    /// The action ID whose entity transitioned.
    action_id: String,
    /// Fingerprint of the entity in the state machine.
    fingerprint: String,
    /// The state machine name.
    state_machine: String,
    /// State before the transition.
    previous_status: String,
    /// State after the transition.
    new_status: String,
},

/// An approval request was resolved (approved or rejected).
ApprovalResolved {
    /// The approval ID.
    approval_id: String,
    /// The decision: `"approved"` or `"rejected"`.
    decision: String,
    /// Who made the decision, if recorded.
    decided_by: Option<String>,
},
```

### 3.1 Serde Tags

All variants use `#[serde(tag = "type", rename_all = "snake_case")]` (existing enum attribute), producing tags:
- `chain_step_completed`
- `chain_completed`
- `group_event_added`
- `group_resolved`
- `action_status_changed`
- `approval_resolved`

### 3.2 Backward Compatibility

The existing `StreamEventType` enum uses `#[serde(tag = "type")]`. Adding new variants is backward-compatible for deserialization: old clients that encounter unknown `type` tags will fail to deserialize that specific event but can skip it gracefully (the client SSE parser already handles individual parse failures). The new variants only appear on the new `/v1/subscribe` endpoint and the existing `/v1/stream` endpoint (where they enrich the firehose).

### 3.3 Updates to `stream_event_type_tag`

The `stream_event_type_tag` function in `crates/server/src/api/stream.rs` must be extended with arms for the six new variants.

### 3.4 Updates to `sanitize_outcome` and `reconstruct_outcome`

These functions do not need modification — the new event types do not contain `ActionOutcome` payloads. They carry only IDs and status strings.

---

## 4. Subscription Channel Architecture

### 4.1 Approach: Filter from Global Broadcast

The subscription endpoint reuses the existing `tokio::sync::broadcast::Sender<StreamEvent>` on the `Gateway` (the `stream_tx` field). The new endpoint subscribes to this broadcast and applies an entity-scoped filter.

**Rationale:**

| Approach | Memory | CPU | Complexity |
|---|---|---|---|
| Per-entity channels | O(active entities) additional channels | Low (direct delivery) | High (channel lifecycle management, cleanup) |
| Filter from global broadcast | Zero additional channels | O(events x subscribers) filtering | Low (reuses existing infrastructure) |

The global broadcast approach is chosen because:

1. **Simplicity**: No new channel management, no per-entity cleanup, no coordination between gateway and subscription handler.
2. **Event volume**: The gateway already broadcasts all events. The CPU cost of per-subscriber filtering is negligible — matching `action_id`, `chain_id`, or `group_id` is a string comparison.
3. **Memory**: No additional broadcast channels to allocate or garbage-collect.
4. **Consistency**: Both `/v1/stream` and `/v1/subscribe` use the same broadcast, guaranteeing event ordering.

### 4.2 Filter Implementation

The subscription handler creates a `BroadcastStream` from `gateway.stream_tx().subscribe()` and applies a `filter_map` that:

1. Checks tenant isolation (same as existing stream).
2. Matches the event's entity ID against the subscribed entity:
   - For `entity_type = "chain"`: match on `chain_id` field in `ChainStepCompleted`, `ChainCompleted`, `ChainAdvanced`, and `ActionDispatched` events where the `action_id` corresponds to the chain's origin action.
   - For `entity_type = "group"`: match on `group_id` field in `GroupEventAdded`, `GroupResolved`, `GroupFlushed`.
   - For `entity_type = "action"`: match on `StreamEvent.action_id == Some(entity_id)`, plus `ApprovalRequired`, `ApprovalResolved`, `ActionStatusChanged`, and `ScheduledActionDue` where the IDs match.
3. Optionally filters by `event_type` query parameter.

### 4.3 Connection Cleanup

When the client disconnects:
- The `BroadcastStream` is dropped, which unsubscribes from the broadcast channel.
- The `ConnectionGuard` is dropped, releasing the per-tenant connection slot.
- No entity-specific cleanup is required (no per-entity channels to tear down).

---

## 5. Historical Catch-up

> **Status**: Implemented. See `crates/server/src/api/subscribe.rs`.

When a client subscribes mid-flight to a running entity, it needs to know what has already happened.

### 5.1 Chain Catch-up

When `include_history=true` (default) for a `chain` subscription:

1. Load the `ChainState` from the state store via `gateway.get_chain_status()`.
2. For each completed step in `step_results` (non-`None` entries), sorted by `completed_at`, emit a synthetic `ChainStepCompleted` event with:
   - `chain_id`, `step_name`, `step_index`, `success`
   - `next_step`: derived from `execution_path` (the step after this one in the path)
3. If the chain is in a terminal state (`Completed`, `Failed`, `Cancelled`, `TimedOut`), emit a synthetic `ChainCompleted` event.
4. All synthetic events use a `UUIDv7` ID generated at catch-up time. No `synthetic: true` field is added — clients do not need to distinguish catch-up from live events.
5. After catch-up, switch to the live broadcast stream.

### 5.2 Group Catch-up

When `include_history=true` for a `group` subscription:

1. Load the `EventGroup` from `GroupManager::get_group()`.
2. Emit a synthetic `GroupEventAdded` event reflecting the current event count.
3. If the group state is `Notified`, emit a `GroupFlushed` event. If `Resolved`, emit a `GroupResolved` event.

### 5.3 Action Catch-up

For `action` subscriptions, catch-up uses the audit store:

1. Query the audit store via `get_by_action_id()` for the most recent record matching the action ID.
2. Apply tenant isolation (skip if caller not authorized for the record's tenant).
3. Reconstruct an `ActionDispatched` event from the audit record using `reconstruct_outcome` / `sanitize_outcome`.
4. Emit it as a synthetic catch-up event with a fresh `UUIDv7` ID.

### 5.4 Reconnection with `Last-Event-ID`

The subscription endpoint supports `Last-Event-ID` for reconnection:

1. If the header is present, extract the `UUIDv7` timestamp.
2. Skip catch-up events with IDs <= `Last-Event-ID`.
3. For the live stream, apply the same dedup logic as `/v1/stream` (skip broadcast events with IDs <= the last replayed ID).

This provides seamless reconnection: the client resumes from where it left off without missing or duplicating events.

---

## 6. Gateway Emission Points

Each new `StreamEventType` variant must be emitted at specific points in the gateway dispatch pipeline.

### 6.1 `ChainStepCompleted`

**File**: `crates/gateway/src/gateway.rs`, `advance_chain` method
**Location**: After each step result is persisted in `chain_state.step_results[step_idx]`, before the lock is released.

Emit at:
- Line ~1704 (step success, more steps to go)
- Line ~1735 (step success, chain complete — emit step event then chain event)
- Line ~1802 (step failed with Skip policy, more steps)
- Line ~1838 (step failed with Skip policy, chain complete)

```rust
let stream_event = StreamEvent {
    id: uuid::Uuid::now_v7().to_string(),
    timestamp: Utc::now(),
    event_type: StreamEventType::ChainStepCompleted {
        chain_id: chain_id.to_string(),
        step_name: step_config.name.clone(),
        step_index: step_idx,
        success: step_result.success,
        next_step: next_step_idx.map(|idx| chain_config.steps[idx].name.clone()),
    },
    namespace: namespace.to_string(),
    tenant: tenant.to_string(),
    action_type: Some(step_config.action_type.clone()),
    action_id: Some(chain_state.origin_action.id.to_string()),
};
let _ = self.stream_tx.send(stream_event);
```

### 6.2 `ChainCompleted`

**File**: `crates/gateway/src/gateway.rs`, `advance_chain` method
**Location**: After a chain reaches any terminal state (Completed, Failed, Cancelled, TimedOut), after `emit_chain_terminal_audit`.

Emit at:
- Line ~1547 (timed out)
- Line ~1735 (completed)
- Line ~1772 (step failed, Abort)
- Line ~1838 (step failed Skip, chain completed)
- Line ~1867 (step failed, DLQ)
- Line ~1896 (unexpected outcome)

Also in `cancel_chain` (line ~2274).

```rust
let stream_event = StreamEvent {
    id: uuid::Uuid::now_v7().to_string(),
    timestamp: Utc::now(),
    event_type: StreamEventType::ChainCompleted {
        chain_id: chain_id.to_string(),
        status: status_str.to_string(),
        execution_path: chain_state.execution_path.clone(),
    },
    namespace: namespace.to_string(),
    tenant: tenant.to_string(),
    action_type: Some(chain_state.chain_name.clone()),
    action_id: Some(chain_state.origin_action.id.to_string()),
};
let _ = self.stream_tx.send(stream_event);
```

### 6.3 `GroupEventAdded`

**File**: `crates/gateway/src/gateway.rs`, `handle_group` method
**Location**: After `group_manager.add_to_group` succeeds (line ~1264), before returning the `Grouped` outcome.

```rust
let stream_event = StreamEvent {
    id: uuid::Uuid::now_v7().to_string(),
    timestamp: Utc::now(),
    event_type: StreamEventType::GroupEventAdded {
        group_id: group_id.clone(),
        group_key: group_key.clone(), // requires extracting from GroupManager
        event_count: group_size,
    },
    namespace: action.namespace.to_string(),
    tenant: action.tenant.to_string(),
    action_type: Some(action.action_type.clone()),
    action_id: Some(action.id.to_string()),
};
let _ = self.stream_tx.send(stream_event);
```

**Note**: The `handle_group` method currently does not have access to the `group_key`. The `GroupManager::add_to_group` return type should be extended to also return the `group_key`, or it can be computed from the action and `group_by` fields (same logic used internally by `GroupManager`).

### 6.4 `GroupResolved`

**File**: `crates/gateway/src/gateway.rs` or `crates/gateway/src/background.rs`
**Location**: When a group transitions to `Resolved` state. Currently, groups transition to `Notified` after flushing (in `BackgroundProcessor::flush_ready_groups`). A `GroupResolved` event should be emitted when `GroupManager::resolve_group` is called (if such a method exists) or after the flush completes.

For the initial implementation, emit after `flush_ready_groups` completes for each group. The `BackgroundProcessor` needs access to `stream_tx` (passed via the builder).

### 6.5 `ActionStatusChanged`

**File**: `crates/gateway/src/gateway.rs`, `handle_state_machine` method
**Location**: After the state transition is persisted (line ~905+), before the lock is released.

```rust
let stream_event = StreamEvent {
    id: uuid::Uuid::now_v7().to_string(),
    timestamp: Utc::now(),
    event_type: StreamEventType::ActionStatusChanged {
        action_id: action.id.to_string(),
        fingerprint: fingerprint.clone(),
        state_machine: state_machine_name.to_string(),
        previous_status: current_state.clone(),
        new_status: target_state.clone(),
    },
    namespace: action.namespace.to_string(),
    tenant: action.tenant.to_string(),
    action_type: Some(action.action_type.clone()),
    action_id: Some(action.id.to_string()),
};
let _ = self.stream_tx.send(stream_event);
```

Also in `BackgroundProcessor::process_timeouts` for timeout-driven transitions (requires passing `stream_tx` to `BackgroundProcessor`).

### 6.6 `ApprovalResolved`

**File**: `crates/gateway/src/gateway.rs`, approval resolution methods
**Location**: After an approval is approved or rejected. The approval resolution currently happens in the server's `approve` and `reject` handlers (`crates/server/src/api/approvals.rs`), which call `gateway.resolve_approval(...)`. The event should be emitted after the approval record is updated.

```rust
let stream_event = StreamEvent {
    id: uuid::Uuid::now_v7().to_string(),
    timestamp: Utc::now(),
    event_type: StreamEventType::ApprovalResolved {
        approval_id: approval_id.to_string(),
        decision: decision.to_string(), // "approved" or "rejected"
        decided_by: decided_by.map(String::from),
    },
    namespace: namespace.to_string(),
    tenant: tenant.to_string(),
    action_type: None,
    action_id: Some(action_id.to_string()),
};
let _ = self.stream_tx.send(stream_event);
```

### 6.7 Summary of Gateway Changes

| Event Type | Emission Point | File |
|---|---|---|
| `ChainStepCompleted` | After step result persisted in `advance_chain` | `gateway.rs` |
| `ChainCompleted` | After terminal state in `advance_chain` and `cancel_chain` | `gateway.rs` |
| `GroupEventAdded` | After `add_to_group` in `handle_group` | `gateway.rs` |
| `GroupResolved` | After group flush in `flush_ready_groups` | `background.rs` |
| `ActionStatusChanged` | After state transition in `handle_state_machine` | `gateway.rs` |
| `ActionStatusChanged` | After timeout transition in `process_timeouts` | `background.rs` |
| `ApprovalResolved` | After approval decision in resolution methods | `gateway.rs` |

### 6.8 `BackgroundProcessor` Access to `stream_tx`

The `BackgroundProcessor` currently does not have access to the broadcast channel. To emit `GroupResolved` and `ActionStatusChanged` (from timeouts) events, the `BackgroundProcessor` and its builder need a new field:

```rust
pub struct BackgroundProcessor {
    // ... existing fields ...
    /// Optional broadcast sender for SSE events.
    stream_tx: Option<tokio::sync::broadcast::Sender<StreamEvent>>,
}
```

The `BackgroundProcessorBuilder` gets a corresponding `.stream_tx(tx)` method. The server wiring in `main.rs` passes `gateway.stream_tx().clone()` to the builder.

---

## 7. Server Endpoint Wiring

### 7.1 New Module

Create `crates/server/src/api/subscribe.rs` containing:

- `SubscribeQuery` struct (query params: `event_type`, `include_history`, `namespace`, `tenant`)
- `subscribe` handler function
- `EntityType` enum (`Action`, `Chain`, `Group`)
- `build_catchup_events` function (dispatches to chain/group/action catch-up)
- `entity_matches` filter function (checks if a `StreamEvent` belongs to the subscribed entity)

### 7.2 Route Registration

In `crates/server/src/api/mod.rs`, add to the `protected` router:

```rust
.route("/v1/subscribe/:entity_type/:entity_id", get(subscribe::subscribe))
```

### 7.3 Handler Signature

```rust
pub async fn subscribe(
    State(state): State<AppState>,
    axum::Extension(identity): axum::Extension<CallerIdentity>,
    Path((entity_type, entity_id)): Path<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<SubscribeQuery>,
) -> Result<impl IntoResponse, (StatusCode, axum::Json<serde_json::Value>)>
```

### 7.4 OpenAPI Annotations

Add `utoipa` annotations to the handler and types for Swagger UI documentation.

---

## 8. Client SDK Additions

### 8.1 Rust Client (`crates/client/src/`)

Add new types and methods to `ActeonClient`:

```rust
/// Entity type for status subscriptions.
pub enum EntityType {
    Action,
    Chain,
    Group,
}

/// Filter for status subscriptions.
pub struct SubscriptionFilter {
    pub event_type: Option<String>,
    pub include_history: Option<bool>,
    pub namespace: Option<String>,
    pub tenant: Option<String>,
}

impl ActeonClient {
    /// Subscribe to status updates for a specific entity.
    pub async fn subscribe(
        &self,
        entity_type: EntityType,
        entity_id: &str,
        filter: &SubscriptionFilter,
    ) -> Result<EventStream, Error> {
        let type_str = match entity_type {
            EntityType::Action => "action",
            EntityType::Chain => "chain",
            EntityType::Group => "group",
        };
        let url = format!("{}/v1/subscribe/{}/{}", self.base_url, type_str, entity_id);
        // ... same pattern as existing `stream` method
    }

    /// Convenience: dispatch an action and subscribe to its chain.
    ///
    /// If the dispatch produces `ChainStarted`, automatically subscribes to
    /// the chain ID and returns the event stream.
    pub async fn dispatch_and_follow(
        &self,
        action: &Action,
    ) -> Result<(DispatchResponse, Option<EventStream>), Error> {
        let resp = self.dispatch(action).await?;
        if let Some(chain_id) = resp.chain_id() {
            let stream = self.subscribe(
                EntityType::Chain,
                chain_id,
                &SubscriptionFilter::default(),
            ).await?;
            Ok((resp, Some(stream)))
        } else {
            Ok((resp, None))
        }
    }
}
```

### 8.2 `StreamItem` Extension

The existing `StreamItem` enum in `crates/client/src/stream.rs` gains a new variant:

```rust
pub enum StreamItem {
    Event(Box<StreamEvent>),
    Lagged { skipped: u64 },
    KeepAlive,
    /// The subscription has ended (entity reached terminal state).
    SubscriptionEnd {
        reason: String,
        entity_type: String,
        entity_id: String,
    },
}
```

The SSE frame parser is updated to handle `event: subscription_end`.

---

## 9. Error Handling

| Scenario | HTTP Status | Error Body |
|---|---|---|
| Unknown entity type | `400 Bad Request` | `{"error": "invalid entity type: must be action, chain, or group"}` |
| Entity not found (chain/group) | `404 Not Found` | `{"error": "chain not found: {id}"}` |
| Tenant mismatch | `403 Forbidden` | `{"error": "insufficient permissions"}` |
| Connection limit exceeded | `429 Too Many Requests` | `{"error": "too many concurrent SSE connections for this tenant"}` |
| Auth failure | `401 / 403` | Standard auth error |
| SSE not enabled | `503 Service Unavailable` | `{"error": "SSE streaming is not enabled"}` |

---

## 10. Keep-Alive and Timeouts

- **Keep-alive**: Same as `/v1/stream` — 15-second `KeepAlive` interval with `"ping"` text.
- **Idle timeout**: For `action` subscriptions (where there may never be a terminal event), the server closes the connection after 5 minutes of inactivity (no events for the subscribed entity). The client can reconnect with `Last-Event-ID` to resume.
- **Entity TTL**: For `chain` subscriptions, once the chain reaches a terminal state and the `subscription_end` event is emitted, the server closes the connection. The client receives a clean EOF.

---

## 11. Implementation Plan

### Phase 1: Core Types (Task #4)
1. Add six new `StreamEventType` variants to `crates/core/src/stream.rs`.
2. Add serde roundtrip tests for each variant.
3. Update `stream_event_type_tag` in `crates/server/src/api/stream.rs`.
4. Update `outcome_category` if applicable (new variants don't carry `ActionOutcome`, so no changes needed).

### Phase 2: Gateway Emission Points (Task #4)
1. Emit `ChainStepCompleted` and `ChainCompleted` in `advance_chain` and `cancel_chain`.
2. Emit `GroupEventAdded` in `handle_group`.
3. Emit `ActionStatusChanged` in `handle_state_machine`.
4. Emit `ApprovalResolved` in approval resolution.
5. Add `stream_tx` to `BackgroundProcessor` for `GroupResolved` and timeout-driven `ActionStatusChanged`.
6. Emit events from `BackgroundProcessor`.

### Phase 3: Subscribe Endpoint (new task)
1. Create `crates/server/src/api/subscribe.rs` with handler and types.
2. Wire route in `crates/server/src/api/mod.rs`.
3. Implement entity validation, catch-up, and filtering.
4. Add `subscription_end` logic for terminal states.

### Phase 4: Client SDK (new task)
1. Add `EntityType`, `SubscriptionFilter`, and `subscribe` method to `crates/client/src/`.
2. Add `SubscriptionEnd` variant to `StreamItem`.
3. Add `dispatch_and_follow` convenience method.

### Phase 5: Tests (Task #5)
1. Unit tests for new `StreamEventType` serde roundtrips.
2. Unit tests for entity matching filter logic.
3. Integration tests for catch-up event generation.
4. Tests for `subscription_end` emission on terminal states.
5. Tests for reconnection with `Last-Event-ID`.
6. Tests for tenant isolation on subscribe endpoint.

---

## 12. Open Questions and Future Considerations

### 12.1 WebSocket Alternative

SSE is unidirectional (server-to-client). If future features need bidirectional communication (e.g., client sending acknowledgments), WebSocket support could be added. For now, SSE is sufficient and simpler.

### 12.2 Subscription Persistence

Subscriptions are ephemeral (in-memory). If the server restarts, clients must reconnect. `Last-Event-ID` handles catch-up on reconnect. No subscription state is persisted.

### 12.3 Multi-Entity Subscriptions

The initial design supports one entity per connection. For clients that want to track multiple chains simultaneously, they can open multiple SSE connections (within the per-tenant limit) or use the existing `/v1/stream` with appropriate filters.

### 12.4 Rate Limiting on Subscription Events

The `stream_tx` broadcast channel has a fixed buffer (default 256). If the channel fills up (e.g., very high event volume), slow subscribers will lag. The existing `lagged` handling in `BroadcastStream` applies — the client receives a `lagged` event and continues from the latest event.

### 12.5 Metrics

New metrics to add:
- `acteon_subscriptions_active{entity_type}` — gauge of active subscriptions by type.
- `acteon_subscriptions_total{entity_type}` — counter of subscriptions created.
- `acteon_subscription_events_emitted{entity_type,event_type}` — counter of events sent to subscribers.
