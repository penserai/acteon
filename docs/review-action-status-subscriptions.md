# Security & Reliability Review: Action Status Subscriptions

**Reviewer**: Security & Reliability Advocate
**Date**: 2026-02-09
**Design document**: `docs/design-action-status-subscriptions.md`
**Verdict**: Conditionally approved -- address the findings below before implementation.

---

## Executive Summary

The design is structurally sound: it reuses the existing broadcast channel, leverages the established `ConnectionRegistry`, and requires the same `AuthLayer` as `/v1/stream`. However, entity-scoped subscriptions introduce a qualitatively new attack surface compared to the broadcast stream. The new endpoint accepts **attacker-controlled entity IDs** in the URL path and uses them to look up state, which creates opportunities for enumeration, resource abuse, and information leakage that the broadcast stream does not have.

Below are 14 findings organized by severity.

---

## CRITICAL Findings

### C1. Entity ID Enumeration via Timing and HTTP Status (Information Disclosure)

**Where**: Design Section 2.4 -- entity validation returns `404 Not Found` for non-existent chains/groups but accepts any action ID.

**Attack**: An attacker with valid credentials (even `Viewer` role) can enumerate chain and group IDs by probing `GET /v1/subscribe/chain/{id}`. A `404` reveals "this chain does not exist"; a `403` reveals "this chain exists but you cannot access it". The timing difference between a state-store lookup (chain exists, different tenant) and a fast `404` (no state found) further refines the signal.

**Recommendation**:
- Return the same error (`403 Forbidden`) regardless of whether the entity does not exist or the caller lacks access. The response body should say `"forbidden or not found"` without distinguishing the two cases.
- Ensure the code path for "entity exists but wrong tenant" and "entity does not exist" takes approximately the same time. A simple approach: always load the entity, then check tenant. If the load returns `None`, still return `403`.

**Code reference**: The existing `chains.rs:get_chain` at line 273 already returns `404` for missing chains. The new `subscribe.rs` handler must NOT follow this pattern for the subscription pre-check. Use a unified `403`.

---

### C2. Missing Namespace Filtering on Entity Subscriptions (Tenant Isolation Bypass)

**Where**: Design Section 2.3 and 4.2.

**Attack**: The design says tenant isolation is checked by loading the entity's namespace/tenant from the state store. However, the **filter function** (Section 4.2) only matches on entity IDs (`chain_id`, `group_id`, `action_id`). If two tenants could theoretically have entities with the same ID (e.g., both created a group with the same hash key), the filter would deliver events from the wrong tenant.

**Details**:
- Chain IDs are UUIDs generated server-side, so cross-tenant collision is astronomically unlikely.
- Group IDs/keys are derived from action fields and `group_by` rules. If two tenants use identical action types and group-by fields, they could produce the same `group_key`. The `GroupManager` is currently namespace-scoped, but the `StreamEvent` filter in Section 4.2 only checks `group_id` -- it does NOT check `event.tenant`.
- Action IDs are client-supplied strings (`action.id`). A malicious tenant could set their action ID to match another tenant's action ID. The `action` subscription filter (Section 4.2, third bullet) checks `StreamEvent.action_id == Some(entity_id)` -- no tenant check in the filter itself.

**Recommendation**:
- The entity filter function MUST also check `event.tenant` against the caller's allowed tenants. This is the same tenant filter already applied in the existing stream handler. The design's Section 4.2 says "Checks tenant isolation (same as existing stream)" in step 1 of the filter -- make absolutely sure this check runs BEFORE the entity-ID match, and that it is mandatory even when `allowed_tenants` is `None` (wildcard callers). Add a test that proves cross-tenant events are not delivered.
- For `action` subscriptions where the caller provides `namespace` and `tenant` as query params: validate that these query params match the caller's grants. A `Viewer` for `tenant-a` should not be able to pass `?tenant=tenant-b` and receive events for `tenant-b`.

**Code reference**: Existing tenant filter in `crates/server/src/api/stream.rs:367-370`.

---

## HIGH Findings

### H1. Connection Exhaustion via Subscription Amplification

**Where**: Design Section 2.7 -- subscriptions share the `ConnectionRegistry`.

**Attack**: The existing `/v1/stream` uses one SSE connection for all events. With entity subscriptions, a client naturally opens **many** connections (one per chain being tracked). A malicious or misbehaving client dispatching 100 chains would need 100 subscription connections. The default per-tenant limit is 10, which means:
1. Legitimate use is artificially capped (frustrating for heavy chain users).
2. An attacker at the limit blocks OTHER callers for the same tenant from using `/v1/stream`.

**Recommendation**:
- Introduce **separate** connection pools for `/v1/stream` (broadcast) and `/v1/subscribe` (entity). Give each its own limit. This prevents subscription usage from starving the broadcast stream.
- Alternatively, add a per-caller sublimit within the tenant pool. Each caller gets at most N subscription connections. This prevents one rogue API key from consuming the entire tenant's budget.
- Consider a higher default limit for subscriptions (e.g., 50) since they are inherently per-entity.

### H2. Catch-up Queries Hit State Store in Request Path (Latency/Availability Risk)

**Where**: Design Sections 5.1, 5.2, 5.3.

**Attack**: When `include_history=true` (the default), the subscribe handler performs a synchronous state-store lookup (for chains/groups) or audit-store query (for actions) before establishing the SSE stream. Under load, many simultaneous subscription requests will serialize on the state store, increasing latency for ALL gateway operations (dispatches, chain advances, etc.) because they share the same state store connection pool.

An attacker can amplify this by repeatedly connecting and disconnecting to the subscribe endpoint, forcing repeated state-store reads without maintaining a long-lived connection.

**Recommendation**:
- Add a rate limit specifically for subscription endpoint requests (not just connection count). Use `RateLimiter::check_custom_limit` with a tight per-caller tier (e.g., 10 subscription creations per minute).
- Set a deadline/timeout on the catch-up state-store query (e.g., 2 seconds). If the state store is slow, skip catch-up and start with live events only. Log a warning.
- Consider caching chain state for catch-up (LRU cache keyed by chain_id, TTL 5s) to avoid repeated identical lookups when multiple clients subscribe to the same chain.

### H3. `decided_by` in `ApprovalResolved` Leaks Internal Identity

**Where**: Design Section 3 -- `ApprovalResolved` variant.

**Attack**: The `decided_by` field reveals the identity of the human or system that approved/rejected an action. This is broadcast to ALL subscribers of the global `/v1/stream`, not just the subscription endpoint. A `Viewer` role caller can observe who approved what, which may violate privacy or compliance requirements (e.g., SOX separation of duties visibility).

**Recommendation**:
- Omit `decided_by` from the SSE event or replace it with a generic indicator (e.g., `"decided_by": "human"` vs `"decided_by": "system"`). The full identity is available in the audit trail for authorized callers.
- At minimum, add `decided_by` to the `sanitize_outcome`-equivalent for the new event types.

### H4. Synthetic Catch-up Events May Bypass Sanitization

**Where**: Design Section 5.1, 5.2.

**Attack**: The catch-up logic constructs synthetic `StreamEvent` objects from `ChainState.step_results`. The `StepResult` struct includes `response_body` (the provider's full response), which may contain PII, secrets, or internal data. If the catch-up code includes `response_body` in the synthetic event, it would bypass the `sanitize_outcome` function that normally strips this data.

**Recommendation**:
- The synthetic catch-up events (Section 5.1) must NOT include `response_body` from step results. The `ChainStepCompleted` event type only carries `chain_id`, `step_name`, `step_index`, `success`, `next_step` -- which is safe. Ensure the implementation does not accidentally include additional fields.
- Add an explicit test: create a chain with `step_results` containing sensitive `response_body`, perform a subscription catch-up, and verify the emitted events do not contain the response body.

---

## MEDIUM Findings

### M1. Action Subscription Idle Timeout Creates Predictable Disconnection

**Where**: Design Section 10 -- 5-minute idle timeout for action subscriptions.

**Issue**: The 5-minute idle timeout means an attacker can determine whether a given action ID has produced any events in the last 5 minutes. If the connection stays open, events are flowing. If it closes, the action is idle. This is a weak timing side-channel but could matter in sensitive contexts.

**Recommendation**:
- Make the idle timeout configurable (server config, not client-controlled).
- Emit keep-alive pings that are indistinguishable from real event delivery timing to reduce the timing signal. The current 15-second keep-alive interval is already included; just ensure it is not distinguishable from real events at the SSE protocol level (same `event:` tag format).

### M2. `subscription_end` Event Reveals Entity Terminal State to Broadcast Listeners

**Where**: Design Section 2.5.

**Issue**: The `subscription_end` event is sent on the entity subscription stream. If this event is also emitted on the global `stream_tx` broadcast channel, any `Viewer` on any tenant could observe when chains complete for other tenants (subject to existing tenant filtering). The design is ambiguous about whether `subscription_end` goes through the broadcast.

**Recommendation**:
- Clarify: `subscription_end` should be emitted **only** on the subscription stream, NOT on the global broadcast. It is a subscription lifecycle event, not a gateway domain event.
- The `ChainCompleted` event already covers the "chain finished" signal for the broadcast. No need to duplicate.

### M3. `Last-Event-ID` Replay Window Inconsistency

**Where**: Design Section 5.4.

**Issue**: The existing `/v1/stream` caps replay to `MAX_REPLAY_WINDOW` (300 seconds) and `MAX_REPLAY_EVENTS` (1000). The subscription endpoint also supports `Last-Event-ID` but the design does not specify whether the same limits apply. For entity subscriptions, 1000 events is generous (a chain rarely has more than ~100 steps), but 300 seconds is tight for long-running chains that may take hours.

**Recommendation**:
- For chain and group subscriptions, extend the replay window or remove it entirely -- rely on the state-store catch-up instead of audit-based replay. The catch-up logic (Section 5.1) already reconstructs the full chain state from the state store, making audit-based replay redundant for entity subscriptions.
- Document explicitly whether `Last-Event-ID` uses audit replay (like `/v1/stream`) or state-store catch-up (like `include_history`). Mixing both could produce duplicate events.

### M4. Missing Input Validation on `entity_type` and `entity_id` Path Parameters

**Where**: Design Section 2.1 -- URL path `GET /v1/subscribe/{entity_type}/{entity_id}`.

**Issue**: `entity_type` and `entity_id` are arbitrary strings from the URL. The design handles unknown `entity_type` with a 400 error (Section 9). However, `entity_id` is not validated for format. An excessively long `entity_id` (e.g., 1MB string) would be passed through to state-store lookups, potentially causing issues.

**Recommendation**:
- Validate `entity_id` length (max 256 characters is generous for any UUID or key).
- Validate `entity_id` character set (alphanumeric, hyphens, underscores, colons). Reject control characters, null bytes, and path traversal characters.
- Validate `entity_type` with an enum parse, not string comparison.

### M5. `BackgroundProcessor` Access to `stream_tx` Creates New Failure Surface

**Where**: Design Section 6.8.

**Issue**: The `BackgroundProcessor` currently operates independently of the SSE stream. Adding `stream_tx` coupling means that if the broadcast channel is full (all receivers lagged), `send()` returns an error. The `let _ = self.stream_tx.send(...)` pattern (fire-and-forget) handles this, but if `stream_tx` is `None` (Optional per the design), every emission point needs a conditional check.

**Recommendation**:
- Use `Option<broadcast::Sender<StreamEvent>>` and wrap emission in a helper method:
  ```rust
  fn emit_event(&self, event: StreamEvent) {
      if let Some(ref tx) = self.stream_tx {
          let _ = tx.send(event);
      }
  }
  ```
- Ensure the `BackgroundProcessor` does NOT block or retry on send failure. The current fire-and-forget pattern (`let _ = send(...)`) is correct. Do not change this to `.expect()` or error propagation.

---

## LOW Findings

### L1. Broadcast Channel Buffer Pressure from New Event Types

**Where**: Design Section 12.4.

**Issue**: The design adds 6 new `StreamEventType` variants that are all emitted through the same broadcast channel (default buffer size 256). A single chain with 10 steps generates at least 11 events (10 step + 1 completion). Under high chain throughput, this amplifies the event volume on the broadcast, increasing the probability that slow subscribers lag.

**Recommendation**:
- Document in the server config that the `stream_buffer_size` should be increased when using entity subscriptions with high chain throughput. Suggest a formula: `buffer_size >= max_concurrent_chains * avg_steps_per_chain`.
- Expose `stream_buffer_size` as a tunable config parameter if not already (it is set via `GatewayBuilder`).

### L2. Synthetic Events `UUIDv7` IDs May Confuse `Last-Event-ID` Dedup

**Where**: Design Section 5.1, point 4.

**Issue**: Synthetic catch-up events are assigned fresh `UUIDv7` IDs generated at catch-up time. These IDs will have timestamps at or after the subscription start. If the client disconnects and reconnects with `Last-Event-ID` set to a synthetic event ID, the dedup logic (which compares IDs lexicographically) will correctly skip events before the synthetic ID. However, live events that were emitted BEFORE the synthetic ID's timestamp but AFTER the actual historical event could be missed.

**Recommendation**:
- Accept this as a known limitation. Document that `Last-Event-ID` after catch-up may miss events that occurred in the gap between the state-store read and the broadcast subscription.
- The existing `/v1/stream` has the same gap (Section 5 of `stream.rs` subscribes to broadcast BEFORE querying audit). Ensure the subscription handler subscribes to the broadcast BEFORE loading catch-up state, as the existing handler does.

### L3. No Metrics for Subscription Endpoint Errors

**Where**: Design Section 12.5.

**Issue**: The proposed metrics track active subscriptions and emitted events but not error rates (auth failures, entity-not-found, connection limit reached). Error metrics are critical for detecting attacks.

**Recommendation**:
- Add `acteon_subscription_errors{entity_type, error_type}` counter where `error_type` is one of: `auth_failed`, `not_found_or_forbidden`, `connection_limit`, `invalid_entity_type`.
- Alert on `not_found_or_forbidden` rate spikes (possible enumeration attack).

---

## Reliability Findings

### R1. No Graceful Degradation When State Store Is Unavailable

**Issue**: If the state store is down, the subscription handler will fail to load `ChainState` for catch-up and entity validation, returning a `500 Internal Server Error`. This could cascade: if the state store has intermittent failures, every subscription attempt fails, leading to a thundering herd of retries.

**Recommendation**:
- If the state store is unavailable, skip entity validation and catch-up. Start the subscription in "live-only" mode. Emit a synthetic event: `event: warning data: {"message":"catch-up unavailable, live events only"}`.
- Return `503 Service Unavailable` ONLY if the state store is completely unresponsive (timeout after 2s), not on transient errors.

### R2. Completed Entity Cleanup and Memory

**Issue**: When a chain completes, connected subscribers receive `subscription_end` and the connection closes. This is clean. However, if a client subscribes to a chain that completed BEFORE the subscription (and `completed_chain_ttl` has expired, so the chain state is garbage-collected), the entity validation will return `404`/`403`. The client has no way to know the chain completed successfully.

**Recommendation**:
- For chains that have been garbage-collected, fall back to the audit store. Query for the chain's audit records and return a synthetic `ChainCompleted` event followed by `subscription_end`.
- Document the `completed_chain_ttl` interaction: clients should subscribe promptly after dispatch, or use the audit API for historical data.

### R3. Reconnection Race Condition with Synthetic Events

**Issue**: The `synthetic: true` field in catch-up events (Section 5.1, point 4) is informational for clients. But if a client reconnects with `Last-Event-ID` pointing to a synthetic event, the server must not attempt audit-based replay (synthetic events have no audit record). The server should detect that the `Last-Event-ID` is synthetic (its timestamp is at catch-up time, not historical event time) and fall back to state-store catch-up.

**Recommendation**:
- Use a distinct `UUIDv7` namespace or prefix for synthetic events (e.g., `synth-{UUIDv7}`) so the server can detect them on reconnect. OR: always use state-store catch-up for entity subscriptions (never audit replay), which sidesteps the issue.
- Simplest approach: for `/v1/subscribe`, always do state-store catch-up, never audit-based replay. `Last-Event-ID` is used only for live-stream dedup (skip broadcast events <= the ID).

---

## Summary Table

| ID | Severity | Category | Title | Blocks Implementation? |
|----|----------|----------|-------|----------------------|
| C1 | Critical | Security | Entity ID enumeration via 404/403 | Yes |
| C2 | Critical | Security | Missing tenant check in entity filter | Yes |
| H1 | High | Reliability | Connection exhaustion via subscription amplification | Yes |
| H2 | High | Reliability | Catch-up queries impact state store latency | Recommended |
| H3 | High | Security | `decided_by` leaks internal identity | Recommended |
| H4 | High | Security | Synthetic catch-up may bypass sanitization | Yes |
| M1 | Medium | Security | Idle timeout timing side-channel | No |
| M2 | Medium | Security | `subscription_end` on global broadcast | Recommended |
| M3 | Medium | Reliability | Replay window inconsistency for entity subs | Recommended |
| M4 | Medium | Security | Missing input validation on path params | Yes |
| M5 | Medium | Reliability | `BackgroundProcessor` coupling | No |
| L1 | Low | Reliability | Broadcast buffer pressure | No |
| L2 | Low | Reliability | Synthetic event ID dedup edge case | No |
| L3 | Low | Observability | Missing error metrics | No |
| R1 | -- | Reliability | No graceful degradation for state store | Recommended |
| R2 | -- | Reliability | Completed entity GC interaction | Recommended |
| R3 | -- | Reliability | Reconnection race with synthetic events | Recommended |

**Blocking items** (C1, C2, H4, M4) must be addressed before the implementation PR is merged. High-severity items (H1, H2, H3) should be addressed in the same PR or have tracked follow-up issues.
