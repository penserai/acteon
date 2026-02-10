# Devil's Advocate: Action Status Subscriptions

> Constructive challenge of the architecture proposed in `docs/design-action-status-subscriptions.md`. For each concern, a recommendation is provided: **proceed**, **simplify**, **defer**, or **drop**.

---

## 1. Do We Even Need a New Endpoint?

**Challenge**: The existing `GET /v1/stream` endpoint already supports query-parameter filtering (`namespace`, `action_type`, `outcome`, `event_type`). Adding `action_id` and `chain_id` filter parameters to the existing `StreamQuery` struct would take approximately 20 lines of code in `stream.rs` and zero new infrastructure. Why introduce `GET /v1/subscribe/{entity_type}/{entity_id}` when `GET /v1/stream?chain_id=abc` achieves the same filtering?

**What already works**: The `StreamEvent` struct already carries `action_id: Option<String>` (line 32, `crates/core/src/stream.rs`). The gateway already emits `ChainAdvanced { chain_id }` events (line 66). Filtering on these fields in `make_event_stream()` is trivial -- it follows the exact same pattern as the existing `namespace`, `action_type`, and `event_type` filters at lines 374-400 of `crates/server/src/api/stream.rs`.

**The design acknowledges this** in Section 4.1 by choosing global broadcast filtering ("the same `tokio::sync::broadcast::Sender<StreamEvent>`"). If the underlying mechanism is identical, the endpoint difference is purely cosmetic -- an HTTP routing distinction that can be added at any time.

**Counter-argument for a new endpoint**: The design (Section 2.6) ties entity-specific behavior to the endpoint: entity validation (404 if not found), `include_history` catch-up, auto-close on terminal state (`subscription_end`). These are features that don't belong in the generic stream handler. This is a legitimate reason for a separate endpoint, BUT only if those features are actually needed (see Sections 3 and 4 below).

**Recommendation**: **Simplify**. Start by adding `action_id`, `chain_id`, and `group_id` query params to the existing `/v1/stream` endpoint. This is a 30-minute change that delivers 80% of the value. Only create `/v1/subscribe/` when catch-up replay and auto-close are implemented -- and only if there is user demand for them.

---

## 2. Per-Entity Channels vs. Global Broadcast Filtering

**The design made the right call** (Section 4.1). Global broadcast with server-side filtering is the correct approach for current and foreseeable scale. I agree completely with the rationale table.

**One additional concern**: The design (Section 12.4) notes the broadcast buffer is "default 256", but the builder code at `crates/gateway/src/builder.rs:86` shows the default is actually **1024**. This inconsistency should be corrected in the design document. At 1024 slots with 6 new event types adding more events per action lifecycle, the buffer may fill faster. Worth monitoring but not a blocker.

**Recommendation**: **Proceed**. Correct the buffer size in the design document.

---

## 3. Historical Catch-up Complexity

**Challenge**: The design (Section 5) proposes synthetic catch-up events that reconstruct the entity's history from state store data. For chains (Section 5.1), this means:

1. Loading `ChainState` from the state store
2. Iterating over `step_results`, synthesizing `ChainStepCompleted` events for each
3. Generating synthetic UUIDv7 IDs with a `synthetic: true` marker
4. Deduplicating against the live stream
5. Handling the race: a step completes between state load and broadcast subscription

**The race condition in step 5 is real and unresolved**. The design says "subscribe to the global broadcast channel with an entity-scoped filter" (Section 2.6, step 4), but the state load (step 3) happens BEFORE subscribing (same as the existing `/v1/stream` pattern where the broadcast subscription happens BEFORE the audit query at line 192 of `stream.rs`). If the design follows the existing pattern (subscribe first, then catch-up), synthetic events for already-completed steps will duplicate with live events for those same steps. The dedup logic needs to handle this -- `Last-Event-ID` won't help because synthetic events have fresh IDs.

**The existing `/v1/chains/{chain_id}` endpoint already returns full `ChainDetailResponse`** with per-step results, execution path, and status. A client that connects mid-flight can:
1. Call `GET /v1/chains/{chain_id}` to get current state (one HTTP request)
2. Subscribe to the stream filtered by `chain_id` for live updates going forward

This two-step approach is simple, has no race conditions, and requires zero new server-side code. The client knows exactly what state it has and when it started listening.

**The `synthetic: true` marker is a code smell**. It means clients must now handle two categories of the same event type -- real and synthetic -- with different semantics (synthetic events may have slightly different field accuracy since they're reconstructed). This leaks implementation complexity to clients.

**Recommendation**: **Drop** synthetic catch-up events for v1. Document the two-step REST+SSE pattern as the recommended approach. If users demand a single-connection experience, implement catch-up in a future phase with proper dedup.

---

## 4. Over-Engineering the Event Types

**Challenge**: The design (Section 3) adds six new `StreamEventType` variants simultaneously:
- `ChainStepCompleted`
- `ChainCompleted`
- `GroupEventAdded`
- `GroupResolved`
- `ActionStatusChanged`
- `ApprovalResolved`

Each new variant requires: (a) adding to the enum in `crates/core/src/stream.rs`, (b) adding a type tag in `stream_event_type_tag()`, (c) finding the right emission point in the gateway, (d) adding serde tests, (e) updating all 5 client SDKs, (f) updating documentation.

**The existing `ChainAdvanced` variant already exists** (line 66, `crates/core/src/stream.rs`) and is emitted when a chain step completes. It currently only carries `chain_id`, but enriching it with `step_name`, `step_index`, and `success` is a backward-compatible change (add optional fields with `#[serde(default)]`).

**However**, I'll concede the design makes a fair point by splitting `ChainStepCompleted` and `ChainCompleted` into distinct variants. The existing `ChainAdvanced` conflates "step done" with "chain progressing", and having a separate `ChainCompleted` event for terminal states is cleaner for clients that want "tell me when it's done". **Two new chain variants are justified**.

**Prioritization by actual user demand**:
- **High value**: `ChainStepCompleted` + `ChainCompleted` -- the core chain tracking use case.
- **Medium value**: `ApprovalResolved` -- approvals have a clear lifecycle and users waiting for approval resolution is a real use case.
- **Low value**: `GroupEventAdded` and `GroupResolved` -- groups already have `GroupFlushed`. The design (Section 6.3) even acknowledges that `handle_group` "does not have access to the `group_key`" and the return type needs modification. This is scope creep into the group manager.
- **Questionable value**: `ActionStatusChanged` -- the design (Section 3) ties this to state machine transitions, but the existing `Timeout` event already covers timeout-driven transitions, and `ActionDispatched` with `StateChanged` outcome covers dispatch-driven transitions. What incremental value does a separate `ActionStatusChanged` provide? The design even duplicates the emission point with `Timeout` events in `BackgroundProcessor::process_timeouts`.

**Recommendation**: **Simplify**. Ship Phase 1 with three variants:
1. `ChainStepCompleted` (new)
2. `ChainCompleted` (new)
3. `ApprovalResolved` (new)

Defer `GroupEventAdded`, `GroupResolved`, and `ActionStatusChanged` to a future phase. This cuts the gateway emission points from 7+ to 4, avoids modifying `GroupManager`'s return types, and avoids the `BackgroundProcessor` needing `stream_tx`.

---

## 5. SSE Protocol Choice

**The design made the right call** (Section 12.1). SSE is the correct protocol for unidirectional status push. The existing infrastructure handles it well, and WebSocket would add unnecessary complexity.

**One addition**: The design's `subscription_end` event (Section 2.5) with auto-close is a nice touch but creates a semantic mismatch with SSE: SSE clients expect the stream to be indefinite (the spec says "the user agent should reconnect" on close). If the server closes after `subscription_end`, some SSE client libraries will auto-reconnect, causing a loop where the client reconnects, gets a catch-up + `subscription_end`, and reconnects again.

**Mitigation**: The `subscription_end` event should include a `retry: 0` field (SSE spec allows setting retry interval) to tell clients NOT to reconnect. Or, better yet, don't auto-close -- let the client decide when to disconnect after receiving `subscription_end`. The stream can remain open for keep-alives without emitting events, and the idle timeout (Section 10, 5 minutes) will eventually clean it up.

**Recommendation**: **Proceed** with SSE. Reconsider auto-close behavior -- emit `subscription_end` but keep the stream open, letting the client close when ready. This avoids reconnection loops.

---

## 6. Connection Scaling

**The design correctly reuses `ConnectionRegistry`** (Section 2.7). Each subscription counts as one connection slot within the per-tenant limit (default 10).

**Concern**: With the current default of 10 connections per tenant, a client tracking 8 chains simultaneously uses 80% of its connection budget, leaving only 2 slots for the global firehose stream and other subscriptions. The design (Section 12.3) acknowledges this with "open multiple SSE connections (within the per-tenant limit)".

**When polling beats SSE**: If a client only needs to check chain status every few seconds (e.g., a CI/CD pipeline waiting for completion), polling `GET /v1/chains/{chain_id}` every 5 seconds is simpler, cheaper, and doesn't consume a connection slot. SSE subscriptions are better for dashboards showing real-time progress.

**Recommendation**: **Proceed** with the existing scaling model. Document when to use polling vs. SSE in the API docs. Consider raising the default per-tenant limit or making subscription connections count separately.

---

## 7. Feature Overlap with Audit Trail

The design does not explicitly address this, but the separation is clear:
- **Audit API**: Historical, query-based. Use for: post-hoc analysis, compliance, debugging.
- **Chains API**: Point-in-time status. Use for: checking current state.
- **SSE stream/subscriptions**: Real-time push. Use for: live dashboards, event-driven integrations.

**Recommendation**: **Proceed**, but document the distinction clearly in the API reference.

---

## 8. Client SDK Complexity

**Challenge**: The design (Section 8) adds `EntityType`, `SubscriptionFilter`, `subscribe()`, and `dispatch_and_follow()` to the Rust client, plus `SubscriptionEnd` to `StreamItem`. Multiply this across 5 SDKs.

**The `dispatch_and_follow` convenience method** (Section 8.1) is clever but creates a coupling between dispatch and subscription that doesn't exist at the protocol level. It requires the `DispatchResponse` to expose a `chain_id()` method, which means the response format needs to include chain IDs. This is a cross-cutting concern that touches the dispatch handler response format.

**If we simplify to "add chain_id filter to existing stream"**: The Rust client needs one new method on `StreamFilter` (`chain_id()`). The other SDKs need the equivalent. This is a ~10-line change per SDK.

**Recommendation**: **Simplify**. Start with filter params on the existing stream in the Rust client. Defer `dispatch_and_follow` -- it's a nice-to-have that adds coupling and can be implemented entirely client-side later.

---

## 9. Testing Complexity

**The design's implementation plan** (Section 11, Phase 5) lists 6 categories of tests. The good news is that the existing test patterns in `crates/server/src/api/stream.rs` (lines 437-1075) are well-structured and deterministic.

**Specific concern with catch-up tests** (Section 11, Phase 5, item 3): Testing synthetic catch-up event generation requires setting up state store fixtures (e.g., creating a `ChainState` with completed steps), which is more complex than the existing broadcast-injection tests. If we defer catch-up (as recommended in Section 3 above), this entire test category disappears.

**Recommendation**: **Proceed**. Follow the existing test patterns. Deferring catch-up simplifies the test surface significantly.

---

## 10. Backward Compatibility

**The design correctly identifies** (Section 3.2) that existing clients using `#[serde(tag = "type")]` will fail on unknown variants.

**Critical issue not addressed**: The existing Rust client's `StreamEventType` enum in `acteon-core` does NOT have `#[serde(other)]`. Adding new server-side variants will cause `parse_sse_frame()` in `crates/client/src/stream.rs` (line 222) to return `Error::Deserialization` for every new event type. This isn't a graceful degradation -- it's a hard error that will break existing clients.

**Action needed before shipping**: Add a catch-all variant to `StreamEventType`:

```rust
#[serde(other)]
Unknown,
```

And update `parse_sse_frame` to handle it. This MUST be released as a minor version bump of `acteon-core` and `acteon-client` BEFORE the new event types are deployed server-side.

**Recommendation**: **Proceed** with caution. The `#[serde(other)]` change to the client must be the FIRST thing shipped, ahead of any server-side changes.

---

## 11. Additional Challenge: `BackgroundProcessor` Gaining `stream_tx`

**The design** (Section 6.8) proposes adding `stream_tx: Option<broadcast::Sender<StreamEvent>>` to `BackgroundProcessor`. This is a new coupling between the background task infrastructure and the SSE stream.

**Currently, the `BackgroundProcessor` communicates via dedicated mpsc channels** (`group_flush_tx`, `timeout_tx`, `chain_advance_tx`, etc.). The server's main loop receives these events and acts on them. Adding `stream_tx` introduces a second communication path that bypasses the mpsc channel pattern.

**Alternative**: The server's main loop (which already receives `GroupFlushEvent` and `TimeoutEvent` via mpsc) can emit the corresponding `StreamEvent` when it processes these events. This keeps the `BackgroundProcessor` focused on detection (finding ready groups, expired timeouts) and delegates event emission to the server layer. No changes to `BackgroundProcessor` needed.

**Recommendation**: **Simplify**. If we defer `GroupResolved` and `ActionStatusChanged` (as recommended in Section 4), the `BackgroundProcessor` doesn't need `stream_tx` at all. For future phases, prefer emitting from the server's event processing loop rather than from the background processor directly.

---

## 12. Additional Challenge: 5-Minute Idle Timeout for Action Subscriptions

**The design** (Section 10) proposes a 5-minute idle timeout for `action` subscriptions where no events arrive. This is a server-side connection close that the client didn't request.

**Problem**: This creates unpredictable behavior. A client subscribing to an action that's waiting for human approval (which could take hours) will be disconnected after 5 minutes. The client must then reconnect, which triggers catch-up (if implemented), consuming server resources.

**Better approach**: Don't impose server-side idle timeouts on subscriptions. The per-tenant connection limit already prevents resource exhaustion. Let clients manage their own connection lifecycle. If a client subscribes and waits for an approval that takes 2 hours, that's fine -- it's one connection slot out of 10.

**Recommendation**: **Drop** the idle timeout for subscriptions. The keep-alive mechanism (15-second pings) already detects dead clients. Live clients should be allowed to wait indefinitely.

---

## Summary Table

| Decision | Recommendation | Rationale |
|----------|---------------|-----------|
| New `/v1/subscribe/` endpoint | **Simplify** | Add filters to existing `/v1/stream` first |
| Per-entity channels | **Proceed** (design agrees) | Global broadcast + filtering is correct |
| Historical catch-up | **Drop** for v1 | REST+SSE two-step pattern is simpler, no race conditions |
| 6 new event types | **Simplify** to 3 | Ship chain + approval events; defer group + state machine |
| SSE protocol choice | **Proceed** | Correct choice; reconsider auto-close behavior |
| Connection scaling | **Proceed** | Document polling vs. SSE tradeoffs |
| Audit overlap | **Proceed** | Document the distinction |
| Client SDK changes | **Simplify** | Filter params minimize SDK work; defer `dispatch_and_follow` |
| Testing approach | **Proceed** | Deferring catch-up simplifies tests significantly |
| Backward compatibility | **Proceed** | Ship `#[serde(other)]` FIRST, before server changes |
| `BackgroundProcessor` changes | **Drop** for v1 | Defer until group/state-machine events are needed |
| Idle timeout | **Drop** | Let clients manage their own lifecycle |

---

## Bottom Line

The design is thoughtful and well-structured, but it tries to deliver everything at once. The recommended phasing:

**Phase 1 (ship now, minimal risk):**
1. Add `#[serde(other)]` variant to `StreamEventType` in client SDKs
2. Add `ChainStepCompleted`, `ChainCompleted`, and `ApprovalResolved` variants
3. Add `chain_id` and `action_id` filter params to existing `/v1/stream`
4. Emit chain events from `advance_chain` and `cancel_chain`
5. Emit `ApprovalResolved` from approval resolution handlers

**Phase 2 (ship when demanded):**
1. Create `/v1/subscribe/{entity_type}/{entity_id}` endpoint
2. Add entity validation and `subscription_end` auto-close
3. Add `dispatch_and_follow` to client SDKs

**Phase 3 (ship when demanded):**
1. Add `GroupEventAdded`, `GroupResolved`, `ActionStatusChanged`
2. Add `stream_tx` to `BackgroundProcessor`
3. Implement historical catch-up with synthetic events

This phasing delivers the highest-value use case (chain progress tracking) in Phase 1 with minimal risk and code changes, while preserving the full design as a roadmap for future phases.
