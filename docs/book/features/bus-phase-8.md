# Agentic Bus — Phase 8

> **Scope**: 5-SDK parity for the bus surface. Rust shipped with
> Phases 1–6c. Phase 8 brings Python, Node, Go, and Java to the
> same shape, in language-by-language sub-phases. **Phase 8a:
> Python.** See the [master plan](../concepts/bus-master-plan.md).

The Rust client carries the bus surface that Phases 1–6c built —
topics, subscriptions, schemas, agents, conversations, tool-call
envelopes, streaming envelopes, and HITL approvals. Phase 8 closes
the cross-language gap so a Python (or Node, Go, Java) caller has
the same flat method names and the same DTO shapes as the Rust
SDK.

## What ships in Phase 8a (Python)

| Surface | Method names |
|---|---|
| Topics | `create_bus_topic`, `list_bus_topics`, `get_bus_topic`, `delete_bus_topic`, `publish_bus_message` |
| Subscriptions | `create_bus_subscription`, `list_bus_subscriptions`, `get_bus_subscription`, `delete_bus_subscription`, `get_bus_subscription_lag` |
| Schemas | `register_bus_schema`, `list_bus_schemas`, `get_bus_schema`, `delete_bus_schema` |
| Agents | `register_bus_agent`, `list_bus_agents`, `get_bus_agent`, `delete_bus_agent`, `heartbeat_bus_agent` |
| Conversations | `create_bus_conversation`, `list_bus_conversations`, `get_bus_conversation`, `delete_bus_conversation`, `transition_bus_conversation`, `append_bus_conversation_message`, `replay_bus_conversation_messages` |
| Tool envelopes (Phase 6a) | `post_bus_tool_call`, `post_bus_tool_result`, `lookup_bus_tool_result` |
| Streams (Phase 6b) | `post_bus_stream_chunk`, `post_bus_stream_end`, `bus_stream_consume_url` |
| Approvals (Phase 6c) | `list_bus_approvals`, `get_bus_approval`, `approve_bus_approval`, `reject_bus_approval` |

36 bus methods total, mounted onto :class:`acteon_client.ActeonClient`
via a thin `_BusClientMixin`. DTOs live in `acteon_client.bus_models`
as plain `@dataclass` types with explicit `to_dict` / `from_dict`,
matching the existing model style. Every dataclass is re-exported
from the package root.

## Method-name parity

The Python surface uses the same flat names as the Rust client
(`client.create_bus_topic(...)`, `client.post_bus_tool_call(...)`,
`client.approve_bus_approval(...)`). A polyglot agent can be
mechanically translated by replacing `let` with `=` and reshaping
struct literals into kwargs. This is intentional — the alternative
(a `client.bus.create_topic(...)` namespace) would have read more
Pythonic but would have forced every caller documented in Rust to
mentally rewrite the call site.

## `PostBusToolCallOutcome` — produced vs parked

`post_bus_tool_call` returns a tagged sum so the caller can branch
on whether the server produced the envelope (`outcome.produced`)
or parked it under a Phase 6c approval (`outcome.parked`):

```python
from acteon_client import ActeonClient, PostBusToolCall

client = ActeonClient("http://localhost:3000")
outcome = client.post_bus_tool_call(
    "agents", "demo", "planning-thread",
    PostBusToolCall(
        call_id="call-1",
        tool="billing.charge",
        arguments={"usd": 42},
        sender="planner-1",
        require_approval=True,            # Phase 6c gate
        approval_reason="paid action",
        approval_ttl_ms=600_000,
    ),
)
if outcome.was_parked:
    print(f"awaiting approval: {outcome.parked.approval_id}")
else:
    print(f"on Kafka at {outcome.produced.partition}:{outcome.produced.offset}")
```

A subsequent `approve_bus_approval` returns a
`BusApprovalDecisionResponse` whose `receipt` is the
`BusToolEnvelopeReceipt` from the post-approval produce — same
shape the immediate path returns. The natural `lookup_bus_tool_result`
flow then works identically.

## What `bus_stream_consume_url` builds (and what it doesn't)

The Python SDK does not bundle an SSE consumer — every Python
runtime has its own preferred SSE library
(`httpx-sse`, `aiohttp-sse-client2`, `sseclient`, `python-sse`,
…) and we'd have to take an opinionated bet on one of them or ship
multiple wrappers. Instead, `bus_stream_consume_url(...)` returns
the fully-formed URL with path segments percent-encoded the same
way the Rust SDK encodes them; plug it into your runtime's own
SSE consumer:

```python
url = client.bus_stream_consume_url(
    "agents", "demo", "thread-1", "story-1",
)
# Then with httpx-sse, for example:
import httpx
import httpx_sse
async with httpx.AsyncClient() as http:
    async with httpx_sse.aconnect_sse(http, "GET", url) as evt_stream:
        async for sse in evt_stream.aiter_sse():
            if sse.event == "bus.stream.end":
                break
            handle_chunk(sse.json())
```

A future iteration can land a small SSE iterator on the SDK if
operator demand pushes that way, but V1 keeps the dependency
surface minimal.

## What's deferred to Phase 8b/c/d

Node.js, Go, and Java parity ship in follow-up PRs — same
methodology (mixin / package / package), same flat method names,
same DTO shapes. The Python surface here is the reference; the
other languages translate from it.

## Trust model carries through

Every gate the server enforces in Rust applies identically to
Python callers — payload validation against schema bindings,
participant ACLs on private conversations, the read-side
`as_agent` requirement on lookups, the approval-id audit header
on Phase 6c-approved produces. Nothing about the Python client
loosens those guarantees; the client is purely a DTO shim over
the same REST surface the Rust SDK and the Phase 7 UI consume.
