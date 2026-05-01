# Agentic Bus — Phase 7

> **Scope**: Admin UI for the bus surface shipped in Phases 1–6c.
> Topics, Subscriptions (with per-partition lag), Agents (live
> heartbeat), Conversations (with thread drilldown), and HITL
> Approvals — all operator-facing, no backend changes. See the
> [master plan](../concepts/bus-master-plan.md).

Phases 1–6c built the wire format and HTTP surface. Phase 7 makes
that surface usable from a browser without curl. The UI is a single
`/bus` route with tabs across the bus primitives, plus a
`/bus/conversations/{ns}/{tenant}/{id}` drilldown for individual
threads. Polls the existing REST endpoints; no new server code.

## What ships in Phase 7

| Surface | Shape |
|---|---|
| Top-level page | `/bus` with tabs: Topics, Subscriptions, Agents, Conversations, Approvals |
| Conversation detail | `/bus/conversations/{namespace}/{tenant}/{id}` — replays the thread, color-codes envelope kind, links inline back to the originating approval row |
| Lag widget | Per-subscription expand-on-demand panel with partition-level committed / high-water-mark / lag |
| Agent heartbeat | Live (1 s) relative-timestamp + stale flag based on each agent's `heartbeat_ttl_ms` |
| Approvals queue | Decision card per row with operator id + decision note inputs, approve/reject buttons that gate on `decided_by` being set |
| Sidebar nav | Single "Agentic Bus" entry — the bus is conceptually one feature, so cluttering the nav with 5 separate links would lose the thread |
| Hooks | `useBusTopics`, `useBusSubscriptions`, `useBusSubscriptionLag`, `useBusAgents`, `useBusConversations`, `useBusConversation`, `useBusConversationMessages`, `useBusApprovals`, `useApproveBusApproval`, `useRejectBusApproval` (+ delete mutations) |

## Design decisions

### One route, five tabs

Topics, subscriptions, agents, conversations, and approvals are
five separate REST surfaces but one operational mental model: "is
my bus healthy?" Splitting them into five top-level sidebar entries
makes the operator hop between unrelated-looking pages. A single
`/bus` route with a `?tab=` query parameter keeps the
namespace/tenant filter sticky and lets the conversation thread
view link back to the matching approval row inside the same
top-level page.

### Polling, not SSE (V1)

Bus state changes (new conversations, agent heartbeats, approvals)
happen on operator timescales — every few seconds, not every
millisecond. V1 uses `react-query` polling: 5 s for hot paths
(lag, heartbeat, approvals queue), 10 s for cooler ones (topic
list, conversation list).

A future iteration can migrate to the existing `EventStream`
infra (the same SSE-based `/v1/stream` channel the rest of the UI
already consumes) once the operator UX needs it. Polling is a
better V1 because it works without any backend additions and
degrades gracefully when the bus feature flag is off.

### Operator id is required to decide approvals

The approve/reject buttons are disabled until the operator types
into `decided_by`. The persisted `BusApproval` row carries the
operator id forward into audit; making the button clickable
without it would let a curious tab make a permanent decision with
no attribution.

V2 will lift this from an unvalidated text field to an SSO-derived
identity once the auth layer can vouch for the caller, but the
UX shape — explicit click + traceable actor — stays the same.

### Lag traffic-light

`lag === 0`: green pill. `lag < 1000`: amber. Otherwise: red.
Coarse on purpose; the operator's question is "is this consumer
falling behind?" not "what's the exact offset gap?" — the
expanded partition table answers the latter when needed.

## Conversation thread view

Thread replay is the highest-value drilldown. Each message is
rendered as a card showing:

- `acteon.envelope.kind` badge (`tool_call`, `tool_result`,
  `stream_chunk`, `stream_end`, or just `message` for plain
  conversation entries).
- Sender (from `acteon.conversation.sender`).
- Routing tokens that are present: `tool.call_id`, `stream.id`,
  `approval.id` (the last linking back to the approval queue when
  a record was gated by Phase 6c).
- Partition / offset / produced timestamp.
- Pretty-printed payload.

This is the same primitive the simulation examples assert against
in `bus_tool_call_simulation.rs`, `bus_stream_simulation.rs`, and
`bus_approval_simulation.rs` — operators see the same audit trail
the simulations assert.

## What comes next

- **Phase 8** — 5-SDK parity for the bus surface (Python, Node,
  Go, Java) so non-Rust callers can drive what the UI already
  shows.
- **Phase 9** — Docs, migration guide, end-to-end multi-agent
  example, benchmarks vs raw Kafka.
- **UI follow-ups** — Migrate hot polls to SSE; populate the
  reserved `KeyKind::PendingBusApprovals` index so the approvals
  queue scales beyond a few hundred rows; add inline approve/reject
  affordance from the conversation thread view itself; add a
  publish-rate sparkline alongside the lag widget for cheaper
  "is this topic alive?" diagnostics.
