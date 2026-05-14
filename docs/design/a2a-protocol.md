# Acteon A2A Protocol Implementation

**Status:** Draft
**Author:** Acteon Team
**Created:** 2026-05-14
**Updated:** 2026-05-14 (Core-First Convergence)

## Overview

This document proposes implementing the [Agent2Agent (A2A) Protocol](https://a2a-protocol.org/latest/specification/) in Acteon as a **primary architectural citizen**. Rather than a peripheral facade, A2A concepts (Tasks, AgentCards, and the 8-state lifecycle) will be promoted to **native primitives** in `acteon-core`.

This "Core-First" approach ensures that Acteon's hardened orchestration—rules, quotas, compliance hash chains, and multi-tenant auth—is the foundation for a robust, scalable A2A implementation suitable for enterprise-grade federated agent ecosystems.

## Motivation

A2A is rapidly becoming the "default interop fabric" for multi-vendor agent ecosystems. By elevating A2A to a core use case, Acteon achieves:

1.  **Architectural Convergence** — Acteon agents, chains, and swarms are natively A2A-compliant, reducing translation overhead and improving reliability.
2.  **Hardened Orchestration at Scale** — A2A Tasks inherit Acteon's compliance, audit, and sandboxed validation features out of the box.
3.  **Strategic Multi-Agent Foundation** — Acteon becomes the "safe substrate" for cross-vendor coordination, where every external interaction is tracked via a standardized, observable Task lifecycle.

## Convergence Mapping: A2A ↔ Acteon Core

| A2A Concept | Acteon Core Implementation | Location |
|---|---|---|
| `AgentCard` | Native extension to `Agent` struct (skills, interfaces, schemas) | `crates/core/src/bus_agent.rs` |
| `Task` | **NEW** `bus_task.rs` — Native Acteon primitive for asynchronous work | `crates/core/src/bus_task.rs` |
| `TaskState` | Unified 8-state machine used by both A2A and internal orchestration | `crates/core/src/bus_task.rs` |
| `Artifact` | Native `bus_stream.rs` extension with `append` / `lastChunk` support | `crates/core/src/bus_stream.rs` |
| `Message` / `Part` | Converged envelope formats for all bus traffic | `crates/core/src/bus_conversation.rs` |
| `requires_approval` | Maps natively to `BusApproval` | `crates/core/src/bus_approval.rs` |
| Task ↔ Chain | **Native Bridge**: A2A Tasks are backed by Acteon Chain execution | `crates/core/src/chain.rs` |

## Architecture: Core Convergence

A2A is integrated into the **core gateway loop**, not as a separate service. The `acteon-gateway` handles both internal bus events and A2A wire formats (JSON-RPC 2.0 / REST) using a shared protocol substrate.

```mermaid
graph TD
    External[External A2A clients<br/>ADK / Bedrock / Foundry]
    Internal[Internal callers<br/>Swarm, Chains, SDKs]
    
    subgraph "Acteon Gateway (Core Convergence)"
        Protocol[Protocol Codec Layer<br/>A2A JSON-RPC / REST / SSE]
        TaskEngine[Task Lifecycle Manager<br/>Native 8-state machine]
        ChainEngine[Chain Orchestrator]
    end

    External --> Protocol
    Internal --> Protocol
    
    Protocol --> TaskEngine
    TaskEngine <--> ChainEngine
    
    TaskEngine --> StateStore[(State store)]
    TaskEngine --> Audit[(Audit store)]
    TaskEngine --> Kafka[(Kafka)]
```

### Key Decisions for Core-First

1.  **Task ↔ Chain Foundation:** An A2A Task *is* the primary external representation of an Acteon Chain execution. When an external agent invokes Acteon, the lifecycle is managed by the Chain engine, and the state is projected via the A2A Task primitive.
2.  **Stateless Entrypoints:** The protocol layer in the gateway remains stateless. All Task state is persisted in the shared `StateStore` and synchronized via `Kafka` events, allowing horizontal scaling of A2A endpoints.
3.  **Identity Stamping:** A2A interactions use Phase 10's `Grant.agent_id`. Every external call is identity-bound, ensuring the audit trail shows the specific external agent identity alongside the tenant.
4.  **State Machine Convergence:** The 8-state A2A `TaskState` enum is adopted **verbatim** as the canonical lifecycle. Narrower internal enums (`ConversationState`, `ToolResultStatus`) remain in place for their respective domains, and the Task Engine projects from / into them at the bus boundary. Internal callers are not forced to reason in 8 states.
5.  **Breaking Changes Acceptable:** Acteon has no paid or external customers at this stage. The plan treats the existing bus envelope (`bus_conversation.rs`, `bus_stream.rs`, polyglot SDK message shapes) as freely mutable. No version-shimming or back-compat work is in scope.

### Inherits from Existing Infrastructure

The Core-First plan deliberately reuses what's already shipped, not rebuilds it:

- **SSE streaming + reconnect** (PRs #153–157, May 2026) — `Last-Event-ID` replay, per-tenant connection caps, slow-client backpressure. Drop-in for `SubscribeToTask` and push-notification fan-out.
- **JSON Schema registry** at publish-edge (`crates/bus/src/schema.rs`) — directly powers A2A `Skill.inputSchema` validation.
- **Audit hash chain + compliance verifier** (`crates/server/src/api/compliance.rs`) — every Task transition lands in the same tamper-evident audit pipeline.
- **mTLS stack** (`crates/crypto/src/tls.rs`) — already wired into the shared `reqwest::Client`; satisfies `MutualTlsSecurityScheme` for both inbound A2A requests and outbound push delivery.
- **Multi-tenant ACL** — A2A §3.3.2 ("never leak resource existence to unauthorized clients") is already how Acteon returns 403 vs. 404 on tenant mismatches.
- **Idempotency** — action/chain dedup-key infrastructure maps onto A2A `messageId` deduplication.
- **`Grant.agent_id` binding** (Phase 10) — per-agent API-key identity is already in the auth layer.

## Risks

- **A2A spec churn.** 1.0 was ratified in late 2025 and is still evolving. Keep the protocol codec layer thin and put the `A2A-Version` header on the critical path so future revisions don't cascade into the Task Engine.
- **utoipa recursion landmine.** `Task` references other tasks via `referenceTaskIds`, mirroring the `ChainStepStatus.parallel_sub_steps` infinite-schema-recursion bug. Use `#[schema(value_type = Object)]` on the recursive field from day one.
- **Payload size.** A2A `Part` carries arbitrary data (text, base64 raw, URL refs, JSON). Existing publish-edge caps (512KB content / 1MB payload) are tuned for the internal bus; A2A may need an explicit per-tenant tier or chunked-artifact streaming for large outputs.
- **Push delivery semantics.** A2A doesn't mandate exactly-once. Reuse the webhook provider's retry + DLQ pattern and stamp `acteon.push.attempt` headers for audit replay.
- **8-state surface area in clients.** SDK consumers will now see eight Task states. Worth a doc page distinguishing terminal (Completed/Failed/Canceled/Rejected) from interrupt (InputRequired/AuthRequired) states so library users don't write incorrect "is finished" checks.

## Implementation Plan

### Phase 1: Core Primitives (`acteon-core`) — ~5 days
- [ ] **Native Task:** Add `bus_task.rs` defining `Task` with the 8-state lifecycle, `Artifact`, `Message`, `Part`, and `PushNotificationConfig`. Include validation, serde, and utoipa schemas (apply `#[schema(value_type = Object)]` to recursive fields).
- [ ] **Artifact Streaming:** Update `bus_stream.rs` to include `append` and `last_chunk` metadata for native A2A artifact support.
- [ ] **Agent Evolution:** Extend `Agent` in `bus_agent.rs` with `skills[]`, `interfaces[]`, and JSON Schema capability definitions.
- [ ] **Converged Envelopes:** Align `bus_conversation.rs` message parts with A2A `Message`/`Part` semantics.
- [ ] Unit tests for state transitions, validation, and serde round-trips.

### Phase 2: Gateway Integration (`crates/gateway`) — ~6 days
- [ ] **Protocol Codecs:** Implement encoders/decoders for A2A JSON-RPC 2.0 and the REST binding (spec §11). Wire `A2A-Version` header negotiation with `VersionNotSupportedError`.
- [ ] **Task Engine:** Implement the lifecycle manager in the gateway, handling state transitions and persistence via the existing `State` backend (new `KeyKind::A2aTask`). Use CAS retries to mirror the bus's optimistic-locking pattern.
- [ ] **The Bridge:** Implement native mapping between `Task` state and `Chain` status — A2A `Submitted/Working` ↔ chain step progress; `InputRequired/AuthRequired` ↔ `BusApproval`; terminal states ↔ chain `StepResult`.
- [ ] **Audit Integration:** Stamp every A2A operation with `AuditEventKind::A2aTaskTransition`.
- [ ] **Idempotency:** Wire A2A `messageId` through existing dedup-key infrastructure.

### Phase 3: Discovery & SSE Bridge — ~4 days
- [ ] **Global Discovery Registry:** Implement a dynamic `DiscoveryService` that aggregates native `AgentCard` data for `/.well-known/agent.json`. Public and unauthenticated per spec.
- [ ] **High-Scale Streaming:** Integrate `SubscribeToTask` and `SendStreamingMessage` directly with the gateway's SSE bridge, reusing `Last-Event-ID` and per-tenant connection caps. Re-frame internal `StreamChunk` / `StreamEnd` records as A2A `StreamResponse` envelopes.
- [ ] Optional `GetExtendedAgentCard` for authenticated callers.

### Phase 4: Push Notifications & Security Schemes — ~4 days
- [ ] **Native Push Delivery:** Implement task-scoped webhook delivery (`Create/Get/List/Delete TaskPushNotificationConfig`). Reuse shared `reqwest::Client`, retry + DLQ, and audit-stamped envelope pattern from the webhook provider.
- [ ] **Security Schemes:** Map `APIKeySecurityScheme`, `HTTPAuthSecurityScheme` (Bearer), and `MutualTlsSecurityScheme` to native Acteon Grants and TLS configurations. (`OAuth2`/`OpenIdConnect` deferred to a follow-up.)

### Phase 5: Hardening & Validation — ~3 days
- [ ] **Recursive depth validation** for Tasks to prevent circular reference attacks via `referenceTaskIds`.
- [ ] **Payload caps:** A2A-specific size limits for `Part` content; chunked-artifact streaming for outputs exceeding the existing 1MB payload tier.
- [ ] **Security review:** Run the existing security-review skill against the new endpoints (`/.well-known/agent.json`, `/a2a/rpc`, push delivery worker).
- [ ] **Load test:** Add gateway benchmark covering streamed Task lifecycle under N concurrent subscribers.

### Phase 6: SDK & Simulation — ~5 days
- [ ] Update all polyglot SDKs (Rust, Python, Node, Go, Java) to support the native A2A Task primitives.
- [ ] Add `a2a_core_simulation.rs` demonstrating a Task pipeline through all 8 states (including `InputRequired` and `AuthRequired` interrupts).
- [ ] `docs/book/features/a2a.md` user-facing guide; promote this design doc to `docs/architecture/a2a.md` once shipped.
- [ ] CHANGELOG entry + README feature-matrix update.

### Pre-Commit Checks (per Phase)
- [ ] `cargo fmt --all`
- [ ] `cargo clippy --workspace --no-deps -- -D warnings`
- [ ] `cargo test --workspace --lib --bins --tests`
- [ ] `cargo check --all-targets`
- [ ] `(cd ui && npm run lint && npm run build)` when UI changes touched

**Total estimated effort: ~27 days (≈5.5 weeks single-engineer).** Phase ordering is deliberate — Phases 1–3 deliver an unauthenticated, streaming, discoverable A2A endpoint usable by external clients; Phases 4–6 layer in production-grade auth, push, hardening, and SDK parity.

## References

- [A2A Protocol Specification](https://a2a-protocol.org/latest/specification/)
- [A2A on GitHub](https://github.com/a2aproject/A2A)
- [Google announcement of A2A](https://developers.googleblog.com/en/a2a-a-new-era-of-agent-interoperability/)
- [Agent2Agent protocol upgrade — Google Cloud Blog](https://cloud.google.com/blog/products/ai-machine-learning/agent2agent-protocol-is-getting-an-upgrade)
- Internal: `docs/design/mcp-server.md` — sibling external-protocol implementation
- Internal: `docs/architecture/agent-swarm.md` — multi-agent orchestration this complements
