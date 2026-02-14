# Acteon Feature Brainstorm

> Generated from a thorough codebase review (44 crates, 6 state backends,
> 5 audit backends, 3 rule DSLs, 3 integration providers).

---

## 1. New Integration Providers

### Generic Webhook/HTTP Provider
The most impactful single addition. Acteon ships with Email, Slack, and
PagerDuty providers, but there is no generic HTTP webhook provider. Adding
one lets users integrate with *any* service that accepts HTTP callbacks
without writing a custom provider. Configuration would include URL templates,
header injection, retry policies, and response validation.

### Twilio (SMS/Voice)
Natural complement to email and Slack for notification use cases. Supports
SMS, MMS, and voice calls via a well-documented REST API.

### Microsoft Teams
Common enterprise request alongside Slack. Teams uses incoming webhooks
(simple) or the Bot Framework (richer). Starting with incoming webhooks
covers 80% of use cases.

### Discord
Webhook-based, straightforward to implement, popular for dev/ops alerting
and community notifications.

### AWS SNS/SQS
For cloud-native architectures that want to fan out actions to AWS messaging
infrastructure. Enables Acteon to act as a bridge between internal events
and AWS-native consumers.

### Kafka Producer
Emit actions to Kafka topics for downstream consumers. Useful in
event-driven architectures where Acteon handles the routing/dedup logic
and Kafka handles distribution.

---

## 2. Real-Time & Streaming

### WebSocket/SSE Event Stream — IMPLEMENTED
Currently the API is request/response only. A real-time stream endpoint
(`GET /v1/stream`) would let dashboards and monitoring tools subscribe to
action outcomes as they happen, without polling. Filter by namespace,
tenant, action_type, or outcome.

**Implemented**: SSE endpoint at `GET /v1/stream` with server-side filtering
by namespace, tenant, action_type, outcome, and event_type. Includes tenant
isolation, per-tenant connection limits, outcome sanitization (PII/secrets
stripped), backpressure handling (lagged events), and 15s keep-alive. Rust
client SDK supports `ActeonClient::stream()` with `StreamFilter` builder.
See [Event Streaming](book/features/event-streaming.md) for full docs.

### Action Status Subscriptions — IMPLEMENTED
Subscribe to updates on a specific action ID, chain, or group. Particularly
useful for long-running chains and approval workflows where the caller
needs to know when something completes.

**Implemented**: SSE-based subscriptions for individual actions, chains, and
groups with catch-up delivery of missed events, entity validation, and
stream UI integration.

---

## 3. Scheduling & Time-Based Features

### Delayed/Scheduled Actions — IMPLEMENTED
Accept actions with a `dispatch_at` timestamp. The gateway holds them in the
state backend and dispatches at the scheduled time. Covers use cases like
"send reminder email in 24 hours" or "retry this action at 3am during low
traffic."

**Implemented**: Actions accept a `dispatch_at` timestamp; the gateway persists
them in the state backend and a background poller dispatches due actions. Includes
at-most-once delivery guarantees, configurable grace period, and polyglot client
SDK support. See [Scheduled Actions](book/features/scheduled-actions.md) for full
docs.

### Cron-Based Rule Activation — IMPLEMENTED
Rules that only apply during certain time windows. Examples:
- Suppress non-critical alerts outside business hours
- Reroute to on-call provider during weekends
- Enable maintenance-mode suppression on a schedule

**Implemented**: The rule engine exposes a `time` map with fields `hour`,
`minute`, `second`, `day`, `month`, `year`, `weekday`, `weekday_num`, and
`timestamp`. Works in both YAML (`field: time.hour`) and CEL
(`time.hour >= 9 && time.hour < 17 && time.weekday_num <= 5`) frontends.
See [Time-Based Rules](book/features/time-based-rules.md) for full docs.

### Recurring Actions — IMPLEMENTED
Define an action that fires on a cron schedule. Turns Acteon into a
lightweight scheduler for recurring notifications (daily digests, weekly
reports, periodic health checks).

**Implemented**: API-only recurring action definitions with 5-field cron
expressions, IANA timezone support (via `croner` + `chrono-tz`), and a
background processor that dispatches occurrences through the full gateway
pipeline. At-most-once delivery per occurrence via distributed CAS claims.
No-backfill policy prevents dispatch storms after outages. Full CRUD API
(`POST`/`GET`/`PUT`/`DELETE /v1/recurring`), pause/resume lifecycle, and
Admin UI with list, create, and detail views. Configurable minimum interval
(default 60s). See [Recurring Actions](book/features/recurring-actions.md)
for full docs.

---

## 4. Provider Resilience

### Circuit Breaker — IMPLEMENTED
Track provider health and automatically open the circuit when failure rates
exceed a threshold. Route to fallback providers during outages. This is
different from rerouting rules (which are static/conditional) -- a circuit
breaker is dynamic, automatic, and based on real-time health.

States: Closed (normal) -> Open (failing, reject fast) -> Half-Open (probe).
Configurable per provider: failure threshold, recovery timeout, probe count.

**Implemented**: Distributed circuit breaker backed by the state store with
three states (Closed, Open, HalfOpen). Configurable per provider: failure
threshold, recovery timeout, success threshold, and probe timeout. Non-retryable
errors (validation, auth) are excluded from failure counts. Fallback provider
support when circuits open. API endpoints for inspecting and managing circuit
state. Distributed mutation lock prevents race conditions in multi-instance
deployments. See [Circuit Breaker](book/features/circuit-breaker.md) for full
docs.

### Provider Health Dashboard
Expose per-provider success rates, latency percentiles, and circuit breaker
status via the API (`GET /v1/providers/health`) and Prometheus metrics.

### Weighted/Percentage-Based Routing
Split traffic across providers by percentage (e.g., 90% SendGrid / 10%
Mailgun). Enables:
- Canary deployments for new providers
- Load balancing across equivalent providers
- Gradual migration from one provider to another

### Cost-Aware Routing
Assign cost-per-action to providers and route to minimize cost while
respecting SLAs and priority. Track spend per tenant/namespace. Enables
budget alerts and automatic downgrade to cheaper providers when budgets
are exhausted.

---

## 5. Enhanced Workflow Capabilities

### Conditional Chain Branching — IMPLEMENTED
Current chains are linear (step 1 -> step 2 -> step 3). Add conditional
branches: "if step 2 returns status X, go to step 3a; otherwise step 3b."
This turns chains into lightweight DAG workflows without requiring a full
workflow engine.

**Implemented**: Each chain step can define `branches` — an ordered list of
`BranchCondition` with `field`, `operator` (eq/neq/contains/exists), `value`,
and `target` (step name). Conditions are evaluated in order after step
completion; first match wins. A `default_next` fallback is used when no
condition matches, with sequential advancement as the final fallback.
`ChainState.execution_path` tracks the actual branch path taken. Cycle
detection validates configs at dispatch time. Supports branching on
`success`, `body.*`, and nested JSON paths. Fully backward-compatible — linear
chains work identically. Server config (TOML) and API responses include
branching fields. See `crates/simulation/examples/branch_chain_simulation.rs`
for demos.

### Parallel Chain Steps
Execute multiple steps concurrently within a chain, then join on all/any
completion. Fan-out/fan-in pattern for things like "notify all stakeholders
simultaneously, then proceed when all acknowledge."

### Chain Pause/Resume
Allow external systems to pause a running chain and resume it later. Useful
for human review steps that go beyond simple approve/reject, or for
integrating with external ticketing systems.

### Sub-Chains (Composable Workflows)
Invoke one chain from another as a step. Enables reusable workflow
components (e.g., a "notify-and-escalate" sub-chain used by multiple
parent chains).

---

## 6. Observability & Operations

### OpenTelemetry Distributed Tracing — IMPLEMENTED
Add W3C Trace Context propagation through the full pipeline: HTTP ingress ->
rule evaluation -> state operations -> provider execution -> audit write.
Lets users see Acteon actions in their existing tracing infrastructure
(Jaeger, Tempo, Zipkin). The design document already identifies this as a
planned feature.

**Implemented**: OTLP export (gRPC and HTTP) via `[telemetry]` config section
with configurable sampling, service name, and resource attributes. W3C Trace
Context propagation extracts `traceparent`/`tracestate` headers from incoming
requests, linking server-side spans to caller traces. The gateway pipeline is
fully instrumented with `#[instrument]` spans: `gateway.dispatch`,
`gateway.execute_action`, `gateway.llm_guardrail`, `gateway.handle_dedup`,
`gateway.handle_reroute`, `gateway.handle_state_machine`,
`gateway.handle_request_approval`, `gateway.handle_group`,
`gateway.handle_chain`, and `gateway.advance_chain`. Batch span processor
with graceful shutdown flush ensures no data loss during deployments.
See [Distributed Tracing](book/features/distributed-tracing.md) for full docs.

### Grafana Dashboard Templates
Ship pre-built Grafana dashboard JSON that visualizes the Prometheus metrics
already exported. Panels for: throughput, latency percentiles, rule match
distribution, provider health, error rates, per-tenant usage. Reduces
time-to-value significantly.

### Action Replay from Audit Trail — IMPLEMENTED
Replay failed or historical actions from the audit log. Invaluable for
incident response: "replay everything that was suppressed during the outage"
or "re-execute all actions that hit the dead letter queue in the last hour."

**Implemented**: Single replay via `POST /v1/audit/{action_id}/replay` and
bulk replay via `POST /v1/audit/replay` with full query filters (namespace,
tenant, provider, action_type, outcome, verdict, matched_rule, time range).
Replayed actions get new UUIDs and `replayed_from` metadata for provenance.
See [Action Replay](book/features/action-replay.md) for full docs.

### Dry-Run Mode
`POST /v1/dispatch?dry_run=true` evaluates rules and returns what *would*
happen without actually executing or recording state. Returns the matched
rule, verdict, and would-be provider. Essential for:
- Testing rule changes before deploying
- Debugging why an action was suppressed
- Building rule authoring tools

---

## 7. Developer Experience

### Admin Web UI
A web dashboard for: browsing/editing rules, viewing the audit trail,
managing pending approvals, monitoring provider health, and visualizing
chain progress and state machine transitions. Could be built with a
lightweight framework and served from the existing Axum server alongside
the API.

### Rule Testing CLI
`acteon-cli test-rules --rules ./rules/ --input action.json` evaluates
rules locally without a running server. Enables CI/CD validation of rule
changes before deployment. Output: matched rule, verdict, and evaluation
trace.

### ~~Rule Playground API~~ ✅ Done
`POST /v1/rules/evaluate` accepts an action and returns which rule matched
and why, without executing. Like dry-run but focused specifically on rule
debugging. Returns the full evaluation trace: which rules were checked,
which conditions passed/failed, and the final verdict.

### Terraform/Pulumi Provider
Manage rules, auth config, and provider configuration as
infrastructure-as-code. Especially valuable for teams managing multiple
Acteon environments (dev/staging/prod).

---

## 8. Data & Analytics

### Action Analytics API
Aggregated queries over the audit trail:
- Actions per tenant per hour/day
- Top N suppressed action types
- Average chain completion time
- Provider error rate trends
- Deduplication hit rate

Could expose via `GET /v1/analytics/...` and leverage ClickHouse's
analytical strengths when that audit backend is in use.

### Tenant Usage Quotas -- IMPLEMENTED
Hard limits on actions per tenant per billing period, with configurable
overage behavior:
- Block (HTTP 429)
- Warn (header + metric)
- Degrade (reduce to lower-priority provider)
- Notify (alert tenant admin)

**Implemented**: Per-tenant quota policies with four overage behaviors (Block,
Warn, Degrade, Notify). Quota check runs in the gateway dispatch pipeline
after lock acquisition but before rule evaluation. Epoch-aligned rolling
windows (Hourly/Daily/Weekly/Monthly/Custom) with counters stored in the
state backend. CRUD API at `/v1/quotas` with usage query endpoint.
Metrics: `quota_exceeded`, `quota_warned`, `quota_degraded` counters.
See [Tenant Quotas](book/features/tenant-quotas.md) for full docs.

### Data Retention Policies
Automatic cleanup of old audit records and state entries based on
configurable TTLs per tenant/namespace. Currently the system accumulates
data indefinitely. A background reaper process with configurable policies
would handle this.

---

## 9. Security & Compliance

### Payload Encryption at Rest
Encrypt action payloads before storing in state/audit backends. The crypto
crate already implements AES-256-GCM for config secrets; extending it to
payload data adds defense-in-depth for PII and sensitive business data.

### Field-Level Redaction in Audit
Configurable rules to redact sensitive fields (credit card numbers, SSNs,
API keys) before writing to audit backends. More granular than the existing
`store_payload` toggle, which is all-or-nothing.

Example config:
```yaml
audit:
  redact_fields:
    - "payload.credit_card"
    - "payload.ssn"
    - "metadata.api_key"
  redaction_strategy: "mask"  # or "hash", "remove"
```

### SOC2/HIPAA Audit Mode
Stricter audit settings for regulated environments:
- Synchronous audit writes (guaranteed delivery)
- Immutable records (no update/delete)
- Tamper-evident checksums (hash chain)
- Configurable via a single `compliance_mode` flag

### mTLS Between Components
Mutual TLS for server-to-backend and server-to-provider connections.
Configurable per connection with certificate rotation support.

---

## 10. Platform & Deployment

### WASM Rule Plugins
Let users write custom rule logic in any language that compiles to
WebAssembly, executed in a sandboxed runtime (Wasmtime or Wasmer). Enables
complex business logic that doesn't fit into declarative rules without
compromising security. The design document identifies this as a planned
feature.

### gRPC API
Alternative to REST for high-throughput internal service-to-service
communication. Protobuf definitions for Action, ActionOutcome, and all
API operations. Can coexist with the REST API on a different port.

### Multi-Region Active-Active
Conflict resolution strategies for state when running Acteon across
regions. Options:
- CRDT-based counters for throttling (convergent)
- Region-affinity for deduplication (partition by tenant)
- Async replication for audit (eventual consistency is acceptable)

### Embedded Mode / Library Usage
Use the gateway as an in-process library (no HTTP server) for applications
that want action processing without network overhead. The architecture
already supports this since `acteon-gateway` is a separate crate --
this feature is mostly about documentation, examples, and ensuring the
API surface is ergonomic for library consumers.

---

## Prioritized Recommendations

Ranked by impact-to-effort ratio:

| Priority | Feature | Effort | Impact | Status |
|----------|---------|--------|--------|--------|
| 1 | Generic Webhook Provider | Low | High | **DONE** |
| 2 | Dry-Run Mode | Low | High | **DONE** |
| 3 | Circuit Breaker | Medium | High | **DONE** |
| 4 | Delayed/Scheduled Actions | Medium | High | **DONE** |
| 5 | OpenTelemetry Tracing | Medium | High | **DONE** |
| 6 | Field-Level Audit Redaction | Low | Medium | **DONE** |
| 7 | Cron-Based Rule Activation | Low | Medium | **DONE** |
| 8 | Action Replay | Medium | Medium | **DONE** |
| 9 | WebSocket/SSE Stream | Medium | Medium | **DONE** |
| 10 | Conditional Chain Branching | Medium | Medium | **DONE** |
| 11 | Action Status Subscriptions | Low | Medium | **DONE** |
| 12 | Recurring Actions | Medium | Medium | **DONE** |
| 13 | Tenant Usage Quotas | Medium | Medium | **DONE** |
| 14 | Rule Playground API | Medium | Medium | **DONE** |
| 15 | MCP Server | Medium | Medium | **DONE** |
| 16 | Payload Encryption at Rest | Medium | High | Not started |
| 17 | Rule Testing CLI | Low-Med | High | Not started |
| 18 | Data Retention Policies | Low-Med | Medium | Not started |
| 19 | Provider Health Dashboard | Medium | Medium | Not started |
| 20 | Grafana Dashboard Templates | Low | Medium | Not started |
| 21 | Parallel Chain Steps | Large | Medium | Not started |
| 22 | Sub-Chains | Medium | Medium | Not started |
| 23 | Native Providers (Twilio, Teams, Discord) | Medium ea. | Medium | Not started |
| 24 | Weighted/Canary Routing | Medium | Medium | Not started |
| 25 | Kafka/RabbitMQ Producers | Medium | Medium | Not started |
| 26 | Cost-Aware Routing | Medium | Medium | Not started |
| 27 | Action Analytics API | Med-Large | Medium | Not started |
| 28 | SOC2/HIPAA Audit Mode | Med-Large | Medium | Not started |
| 29 | mTLS Support | Medium | Medium | Not started |
| 30 | gRPC Ingress | Large | Low-Med | Not started |
| 31 | WASM Rule Plugins | Large | Medium | Not started |

---

## Next Wave: Implementation Notes

> Added 2026-02-14. Detailed scoping for the next batch of features.

### P0 — Do Next

**16. Payload Encryption at Rest**
- `acteon-crypto` crate already has AES-256-GCM primitives for config secrets
- Extend to action payloads in state and audit backends
- Key management strategy needed: envelope encryption, optional KMS integration
- Transparent to providers — decrypt on read, encrypt on write

**17. Rule Testing CLI**
- Wraps existing rule playground / trace infrastructure in a CLI
- `acteon-cli test-rules --rules ./rules/ --fixtures tests/cases.yaml`
- Reads YAML test cases: input action + expected verdict/matched rule
- CI/CD friendly: exit code 0/1, machine-readable output option
- Low effort because trace engine (`evaluate_with_trace`) is already built

### P1 — High Value

**18. Data Retention Policies**
- Background reaper following `process_recurring_actions()` pattern
- Per-tenant TTL configs stored in state backend
- Sweeps audit and state backends on a configurable interval
- Respects SOC2/HIPAA audit mode if enabled (skip deletion)

**19. Provider Health Dashboard**
- New Admin UI page aggregating existing Prometheus metrics
- API endpoint `GET /v1/providers/health` returning per-provider stats
- Success rate, p50/p95/p99 latency, circuit breaker state, last error
- Refresh interval configurable in UI

**20. Grafana Dashboard Templates**
- Pre-built JSON dashboards under `deploy/grafana/`
- Panels: throughput, latency percentiles, rule match distribution, provider health, error rates, per-tenant usage
- Quick win for operators adopting Acteon

### P2 — Workflow Power

**21. Parallel Chain Steps (Fan-out / Fan-in)**
- New step kind: `parallel` with list of concurrent sub-steps
- Join semantics: `all` (wait for all) or `any` (first to complete)
- `execution_path` becomes a tree rather than linear list
- Partial failure handling: configurable (fail-fast vs best-effort)
- Timeout per parallel group

**22. Sub-Chains (Composable Workflows)**
- New chain step type referencing another `ChainConfig` by name
- Results from sub-chain available via `{{prev.*}}` in parent chain
- `validate()` must detect circular references across chain boundaries
- Promotes reusability: standard "escalation" or "notification" sub-chains

**23. Native Providers**
- Each provider is a new crate under `crates/providers/`
- Start with Twilio (highest demand), then Teams, then Discord
- Typed request/response structs, specialized error handling, rate limit awareness
- Generic Webhook remains the fallback for unlisted services

**24. Weighted / Canary Routing**
- New rule action or rule modifier: `weight: 90` on rule A, `weight: 10` on rule B
- Deterministic hashing (by action ID) or random distribution
- Supports canary provider rollouts and load balancing

### P3 — Advanced

**25. Kafka / RabbitMQ Producers**
- New provider crate(s) following existing provider pattern
- Config: broker list, topic/queue, serialization format (JSON, Avro, Protobuf)
- Enables Acteon as an event bridge

**26. Cost-Aware Routing**
- Cost metadata on providers; budget tracking per tenant
- Rule conditions that reference remaining budget
- Ties into quota system for enforcement

**27. Action Analytics API**
- `GET /v1/analytics/...` with time-bucketed aggregations
- Leverages ClickHouse audit backend for efficient aggregation
- Endpoints: top actions, suppression rates, chain completion times, provider error trends

**28. SOC2/HIPAA Audit Mode**
- Config toggle: `compliance_mode = "soc2"` or `"hipaa"`
- Synchronous audit writes, immutable records, hash-chaining
- Disables data retention deletion for covered records

**29. mTLS Support**
- Mutual TLS for Postgres, Redis, and provider egress connections
- Certificate rotation support
- Server already uses `axum` + `rustls`; extend to outbound

### P4 — Long-Term

**30. gRPC Ingress**
- Add `tonic` to server, define `.proto` schemas for Action/ActionOutcome/API
- Coexists with REST on a separate port
- Gateway dispatch pipeline is already transport-agnostic

**31. WASM Rule Plugins**
- Sandboxed Wasmtime/Wasmer runtime for user-supplied rule logic
- Plugin interface: receives action JSON, returns verdict
- Resource limits (memory, CPU time) per plugin invocation
- Biggest effort on the roadmap; long-term extensibility play
