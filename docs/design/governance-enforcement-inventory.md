# Governance enforcement inventory

Status: source inventory for the next governed-city implementation slice.
Baseline: `4eb2dac99b414699c63c18d41702d1fb7455f13e`.

This extends the checked HTTP role inventory with execution call sites. None of
these rows establishes that permits or generic closures are enforced today.
The inventory identifies where the future trusted context and start checkpoint
must enter, survive deferral, and govern the actual attempt.

## Observed paths

| Path | Current source boundary | Observed provenance | Required checkpoint and propagation |
|---|---|---|---|
| External dispatch/batch | `api/dispatch.rs`; `Gateway::dispatch_pipeline` | Server identity becomes minimal core Caller | Create trusted context from current identity; persist admission; reject payload authority fields |
| Admitted dispatch/replay | `gateway/admission.rs` | Receipt retains original Action and Caller | Retain context in receipt; current permit state before attempt; preserve reconciliation state |
| Ordinary provider execution | `Gateway::execute_action` / `execute_provider`; `Executor::execute_inner` | Action, optional attachment DispatchContext; explicit library gate after waits, not yet provisioned by Gateway | Supply the production trusted gate, resolve complete provider resources and preserve context; library contract covers each retry after concurrency acquisition |
| Circuit-breaker fallback | `Gateway::execute_on_fallback` | Original Action plus separately selected target name | Check the actual target: action.provider can still name the original provider when the executor receives the fallback instance |
| Deduplicated provider execution | `Gateway::handle_dedup` | Action | Dedup reservation is not authority; check the actual effect before executor attempt |
| Direct chain provider step | `Gateway::advance_chain` / parallel advancement | ChainState retains audit Caller | Propagate full root context; child resource/call reservation for every step and retry |
| Full-pipeline chain dispatch | `Gateway::advance_chain` dispatch step path | Trusted ancestry labels and parent Caller | Preserve cryptographically/trust-bound context separate from user metadata; reauthorize child/fallback destinations |
| Subchain/fan-out | `Gateway::start_sub_chain` / `execute_parallel_group` | Parent IDs and audit Caller | Attenuate authority, share root reservations/deadline, record child lineage before execution |
| Compensation | Chain configuration and advancement compensation paths | Existing execution/step state | Independently authorized effect with explicit compensator authority; cancellation cannot authorize compensation automatically |
| Worker queue handoff | `Gateway::enqueue_worker_task`; `task_queue/recovery.rs` | WorkerTask queue, chain/workflow references, lease tokens; checked root reference for profile-managed workflow tasks | Extend context to standalone/chain workers; verify worker claim authority and original root authority at mediated effects |
| Worker settlement | `task_queue/handoff.rs` | Terminal record is a CAS-backed result outbox | Keep context through terminal handoff; idempotent parent continuation and reservation settlement |
| Workflow continuation | `gateway/workflow.rs`; `workflow_context.rs` | Optional checked root reference in workflow/tasks; library profile verifies scope/input/actor at enqueue, repair and poll | Provision trusted roots at public entrypoints; add represented-party/mandate lineage, attenuated child context and current permit/effect checkpoints |
| Delayed dispatch | `gateway/scheduled.rs` | Stored Action; dispatch_inner currently receives Caller=None | Scheduled record needs original context; time passage or scheduler identity cannot grant authority |
| Recurring firing | `background/workers/recurring.rs`; server main recurring consumer | Recurring event and stored configuration; consumer uses dispatch_precounted_action | Persist schedule owner/context; recheck current authority each firing; define migration and ownership transfer |
| Group flush | `background/workers/group_flush.rs`; server main group consumer | Synthesized Action; dispatch_precounted_action drops caller | Require compatible contributor contexts or explicit aggregator authority; account for resources and retain contributors |
| Timeout notification | `background/workers/timeout.rs`; server main timeout consumer | Synthesized Action; dispatch(..., None) | Explicit service principal/permit for this derived notification; retain causal provenance |
| Human approval execution | `Gateway::execute_approval_inner` | Stored approval Action and signed URL contract | Human decision does not replace original actor authority; checkpoint after decision and before effect |
| Approval notification/retry | `Gateway::handle_request_approval` / `retry_approval_notification` | Direct executor call with synthesized notification | Explicit notifier service authority; bind reviewer destination; never give the agent approval capabilities |
| Enrichment | `gateway/enrichment.rs::apply_enrichments`; ResourceLookup | Resolved parameters and provider lookup trait | Authorize lookup resource before external request; policy/enrichment work may spend budget before final provider execution |
| LLM guardrail/inference | `Gateway::apply_llm_guardrail`; `llm/http.rs`, `llm/typed.rs`, `llm/provider.rs` | LLM request/model config; no common execution authority | Explicit model resource and inference operation; current authority and bounded reservation before real model call |
| Embedding/rule preview | `api/embeddings.rs`; `api/rules.rs`; `embedding/http.rs` | Role/scoped request; inference may happen during evaluation | Preview/dry-run must explicitly distinguish simulation from billable inference; authorize/meter actual calls |
| Bus direct publication | `api/bus.rs::publish` | Bus grants; reserved headers protected | Trusted context reference outside arbitrary headers; topic/resource admission and publish start |
| Bus agent/conversation/tool/stream messages | `api/bus.rs` produce paths | Scoped grants, sender/participant identity and server-stamped headers | Preserve sender and root authority for delivery; do not derive authority from message headers or content |
| Bus deadletter/approval publication | `api/bus.rs` deadletter and approve paths | Subscription or approval state | Delivery/recovery operation needs its own authority and intent; approval cannot override current closure |
| Subscription/session delivery | `api/bus_sessions.rs`; bus backend subscribe/receipt APIs | Subscription, consumer group, session lease and receipts | Current consumer/route/resource authority on claims and governed handoff; already delivered payload cannot be retracted |
| Managed stage processing/replay | `bus/stage.rs`; `api/bus_stages.rs` | Checkpoints, generation/owner token, control/replay intents | Stage service principal/context and effect checkpoints; preserve existing audited halt/replay controls |
| A2A task submission | `api/a2a.rs::method_message_send` | Scoped task/history; no peer runtime invoked | Retain authenticated context; distinguish acceptance from runtime execution; target binding must be explicit |
| A2A push delivery | `api/a2a_push_worker.rs` HTTP send | Stored notification config and delivery state | Explicit notifier authority and approved endpoint binding; actual outbound send/retry checkpoint |
| Swarm execution | `swarm-provider/provider.rs`; swarm runtime adapters | Provider action; separate audit/quota/memory/hook clients | Root context through runtime adapter tools; sandbox/credential isolation for operations outside Acteon mediation |
| Outbound peer A2A | Not implemented in baseline | No remote task mapping/handoff | Governed delegation receipt, attenuated context, start lease and safe peer retry/reconciliation |

Source paths in this table are relative to `crates/server/src` for API/server
references and `crates/gateway/src` for Gateway references. Model/embedding,
bus and swarm paths refer to their corresponding crates.

## Findings that change the implementation approach

1. A gateway-level check before `executor.execute` is insufficient. The executor
   retries internally after delays and takes a concurrency semaphore before
   starting work. Authorization must be refreshed at the actual attempt boundary.
2. Fallback authorization must use the selected provider instance/name, not just
   the Action's original provider field. The current fallback executor receives
   the original Action.
3. Core Caller and provider DispatchContext are both insufficient authority
   containers: Caller has credential provenance and optional stable principal;
   DispatchContext only holds resolved
   attachments. Introduce a distinct trusted execution context.
4. Delayed, recurring, grouped and timeout work have paths with no Caller. They
   need an explicit migration/service-authority model; inserting the current
   server administrator identity would amplify authority.
5. Approval and notification work includes direct executor calls. Routing the
   main action through policy is not enough to govern those effects.
6. Inference, enrichment, previews, bus publication and A2A webhook delivery have
   independent network/effect paths. The provider dispatcher alone cannot cover
   the platform.
7. Worker handoff already has the right durability pattern: terminal state and
   pending delivery share a CAS record. Reuse it for authority-aware settlement
   and intervention rather than create a separate best-effort event.
8. Direct library users and provider implementations remain trusted integration
   boundaries. Strict mode must require governed adapters; network and credential
   isolation prevents an untrusted runtime from bypassing the platform entirely.

## Storage evidence and coordinator prototype contract

StateStore exposes atomic check-and-set, versioned reads and single-key CAS. It
has no generic multi-key transaction. Existing DynamoDB versioned reads request
strong consistency; backend correctness still needs an end-to-end coordinator
contract test, including failure and replica behavior.

Prototype a bounded, non-expiring per-tenant coordination record containing the
current authority generation, closure/revocation restrictions, active start
registrations, and durable pending reconciliation intents. Authority mutation
and attempt start must update that record through CAS. Avoid deleting or expiring
it, which would permit version/generation ABA.

A start first evaluates current referenced authority and records its generation.
Its registration CAS must still observe that generation and the current closure
restrictions. Any permit/grant/principal mutation that affects eligibility must
be serialized through the coordinator before it is acknowledged effective.
A separately updated permit object is not sufficient: define the authoritative
restriction and pending-object-update intent in the coordinator so a crash
cannot expose an allow after revocation.

Registration is the start linearization point. If registration precedes closure,
the effect is in flight even if its network send follows later. Closure cannot
promise to fence an arbitrary remote socket/provider. An expired or abandoned
start registration remains unresolved until reconciled; it cannot be silently
evicted to release budget or authorize replay.

Bound record size and active attempts explicitly. Admission fails with capacity
exhausted rather than truncating authority or forgetting in-flight effects.
Root budgets may use a separate authoritative root ledger, but registration and
reservation need a durable, recoverable state machine. An independent decrement
plus lease insert is not an atomic shared-budget guarantee.

## Required executable proof before integration

- Two coordinator instances share a memory StateStore; controlled barriers prove
  start-before-close and close-before-start outcomes.
- The coordinator uses the configured `StateStore`. The same tests run against
  independently connected instances of each supported durable backend
  (PostgreSQL, Redis and DynamoDB), qualifying their CAS contracts separately.
- A crash after restriction persistence but before secondary permit/closure
  object write leaves the restriction effective and the intent repairable.
- Response loss after a successful registration returns/reconciles the same
  attempt using a stable ID; it never authorizes a fresh effect automatically.
- Concurrent starts and shared reservations cannot exceed configured capacity.
- Revocation between executor retries refuses the next attempt.
- Recovery and rollback never synthesize broader authority for records without
  trusted provenance.

The prototype must prove these contracts before strict enforcement is exposed
through a configuration flag. The source inventory then becomes a checked
coverage fixture with integration tests for each mediated effect class.
