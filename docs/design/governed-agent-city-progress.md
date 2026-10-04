# Governed-city implementation tracking

The full objective is the [governed-city design](governed-agent-city.md). This
file tracks implementation against that objective; a completed PR slice does
not complete a phase or establish the full vision's guarantees.

## First implementation slice: executor/control-plane boundary

Implemented on `feat/governed-executor-boundary`:

- Execution-only `executor` role and independent `OperationsManage` ceiling.
- Explicit inventory of all 193 registered HTTP operations, verified against
  the router in CI; unclassified protected routes fail closed.
- Production-router denial tests for all 78 operator operation entries,
  even with wildcard grants, for both executor and viewer credentials.
- Separate agent/conversation runtime access from registry management.
- Executor heartbeats restricted to the authenticated grant-bound agent.
- A2A RPC method allowlist prevents configuration mutation and future unknown
  methods from inheriting executor authority. Mixed batches are checked before
  executing any member, including notifications.
- Existing JWT sessions use current users, roles and grants after auth reload;
  removed users lose session access.
- Agent guide fixture and live verifier use executor credentials and verify
  denied management calls; authentication docs, all SDK READMEs and UI settings
  explain the boundary.

Evidence lives in `crates/server/tests/api_tests.rs`, role and route-permission
unit tests, `scripts/ci/route_permissions.py`, and `scripts/ci/agent_guide.py`.
These verify endpoint least privilege and existing execution paths. They do not
verify per-effect permits, revocation during a chain, closures or mesh execution.

## Remaining phase gates

| Phase | Current state | Next required evidence |
|---|---|---|
| 0: Inventory/coordinator | Exact resources, bounded CAS coordinator, multi-resource starts and atomic root reservations implemented; memory/Redis contracts pass | Complete boundary qualification, retention/emergency stop and backend failover proof |
| 1: Actors/context | Executor role, stable principals, signed root contexts and selected workflow propagation merged; shared server authentication guard in progress | Remaining deferred propagation, team/mandate lineage, credential enrollment, migration and rollback gates |
| 2: Permits/checkpoints | Internal current permits/credentials/configuration snapshots and durable direct-provider adapter merged; public server enforcement remains open | Qualified credential projection, authenticated scope stamps, real gateway effect coverage, provisioning/API/SDK/UI and recovery |
| 3: Closures/intervention | Existing agent lifecycle only | Generic serialized closures, durable intervention, drain/pause/cancel semantics and acknowledgments |
| 4: Autonomous mesh | Existing registry and submitted A2A tasks only | Real target resolution/invocation, attenuation, lineage, recovery and safe peer retry |
| 5: Production/federation | Not implemented | Verified backend and peer capability matrix, trust/revocation protocol and failure tests |

Next work should integrate trusted execution context into deferred paths and
complete coordinator/accounting prerequisites before per-effect enforcement. Do
not expose workflow/queue mutation as executor tooling merely by changing the
role table: these need retained authority and operation/resource checks first.

For every slice, record PR/review/merge and publication evidence here, keep
public guides synchronized with shipped behavior, and retain the full remaining
phase gates until their own runtime evidence passes.

## Coordinator substrate slice (merged, unintegrated)

The working `acteon-governance` crate implements bounded single-key CAS
coordination, incarnation/generation stamps, exact subject/resource restrictions,
per-attempt registration and settlement, and restriction-plus-control-event
persistence. It is not wired into gateway effects and does not establish permit,
shared-root budget, or public closure functionality.

Local evidence: eight memory contract tests pass, including both controlled race
orderings, lost acknowledgments, ambiguous settlement, active/retained-record
capacity, missing/unknown state, and stale-incarnation rejection. The same
sequential/race/response-loss contract passes through two independently connected
Redis clients using a unique test prefix. The integration workflow now explicitly
runs the real-Redis contract; PR #407 CI confirms it executed and passed.

Remaining Phase 0 work includes reservation protocol proof, canonical resource
identity, checked effect coverage, retention/emergency admission-stop design,
backend failover assumptions, and operational bounds. Context propagation and
per-retry enforcement remain Phase 1/2 integration gates. Neither a substrate
unit test nor a Redis test certifies the full city vision.


## Canonical resource identity slice (merged)

`ResourceRef` supplies exact validated kind/scope/ID identity and a versioned,
strict canonical encoding. Coordinator attempts and restrictions now store typed
references and reject foreign scope; format-1 prototype rows are not silently
migrated. See [resource identity ADR](resource-identity.md).

Targeted core/governance tests, the independent Redis contract, core OpenAPI
compilation, full workspace lint/tests/all-target checks, UI lint/build, and
strict documentation build pass locally. PR #408 merged at `aca3fd45` after current-head CI passed. Actual
resource resolution and per-effect enforcement are not implemented by this type.

The executor slice merged as PR #406 at `e9fc89f6`; its publication verification
is tracked separately from implementation. Coordinator PR #407 merged at `854643c5` after all current-head checks passed.
Its integration-job log confirms the independent Redis contract executed and
passed rather than being left ignored.

## Stable principal binding slice (merged)

Optional validated principal metadata is bound by operator-controlled auth
configuration, kept separate from credential names and secrets, and retained
in core Caller, durable admissions, and chain state. Bound idempotency digests
use the stable principal; original credential provenance is retained on replay.
JWT issuance pins the binding and subsequent validation refuses remapping an
existing session. Invalid/conflicting configuration reloads remain atomic.

`GET /v1/auth/identity` and typed Rust/Python/TypeScript/Go/Java methods inspect
only the current identity. The UI distinguishes credential and principal. The
checked operation catalog contains 194 routes. Agent-guide fixtures and the live
verifier include the explicit principal binding.

Targeted tests prove rotation/replay isolation, current-grant denial, metadata
forgery resistance, JWT remapping refusal, atomic reload failure, and original
principal retention across replacement-gateway chain handoff. Shared identity
fixtures exercise every SDK, including legacy null principal and async Python.
Local workspace validation passed 3,338 tests, lint and all-target checks;
all five SDK contract suites, Python lint/types, Node packaging, Java build,
UI lint/build, and strict docs build pass. The updated live agent guide passes
against the built server, including its principal-binding assertion. PR #409
merged at `f44b9e37` after adversarial review and all current-head CI checks passed.
Server bus-feature
lib/integration tests also pass locally.

This slice does not make Caller a trusted authority envelope or establish
principal lifecycle administration. Worker/workflow/schedule context propagation,
current deferred authority evaluation, coordinator-backed credential/principal
changes, permits, root reservations, and effect checkpoints remain required.
Existing audit caller/quota filters retain their credential-name semantics;
adding a principal binding does not silently reinterpret those controls.

## Trusted root context substrate (merged, integration in progress)

The internal context store captures versioned, HMAC-sealed root provenance and
exact accepted effect tuples. Replacement replicas recover only through trusted
storage with expected actor/execution/input bindings; unknown formats, tampering,
expired deadlines and changed authority incarnations fail closed. Stable-handle
capture reconciles lost acknowledgments and rejects authority broadening on
replay. Signing-key rotation retains old verification keys explicitly.

See the [context ADR](trusted-execution-context.md). PR #410 merged at `6cfcc318`
after adversarial review and all current-head CI checks passed. Workflow library
propagation is the next slice; this does not evaluate current permissions or
complete Phase 1. Child attenuation, durable propagation, legacy
migration, retained-context lifecycle and per-effect enforcement remain gates.
The city design and phase plan now include concrete storage, lifecycle, ownership
and acceptance contracts alongside the current implementation baseline.

Local evidence: eleven context contracts pass, plus one explicitly executed
independent-client Redis contract. Required workspace checks (3,349 passing tests), UI lint/build and
strict public docs build pass; PR #410's integration job executed the Redis
contract and passed. Runtime enforcement remains unintegrated.

Principal-binding publication: Deploy Documentation run `37176654460` passed
for merge `f44b9e37`; fetched authentication and agent-swarm pages contain the
identity endpoint and stable-principal/executor guidance.

## Workflow provenance propagation (merged)

Validated durable references now connect signed root context to workflow and
continuation-task records. An opt-in library profile verifies complete
scope/workflow/queue/input binding, accepted tuple and original actor at start,
enqueue, repair and poll-before-lease. Replacement/timer continuations preserve
the reference. Corrupted, stripped, expired or unconfigured provenance parks
delivery; observation and cancellation remain available. Child creation refuses
provenance loss pending attenuated derivation. See the [integration ADR](workflow-context-propagation.md).

The [agent workforce design](agent-workforce.md) now maps human teams, personal
agents, team agents, ownership, roster assignments and representation mandates
into the same authority model. These organizational/representation capabilities
are proposed, not supplied by the actual-principal context slice. The phased
plan includes their schema, enforcement, offboarding and real-scenario gates.

Public root provisioning, team/mandate context versions, remaining deferred
paths, child attenuation, current authority evaluation and effect checkpoints
remain required. This profile is provenance integrity, not permit enforcement.

Local workflow slice evidence: eight integration scenarios plus two core identity/
compatibility tests pass; the required full workspace has 3,359 passing tests.
Formatting/clippy/all-target checks, UI lint/build, strict public docs build and
design links pass. Interrupted task/index publication repairs the original chosen
continuation ID and reference. PR #411 merged at `9283dfef` after adversarial
review and all current-head CI checks passed. Team/mandate enforcement and
remaining Phase 1 gates are still open.

## Complete effect resources and atomic root accounting (merged)

The coordinator's format-3 contract registers complete exact resource sets and
root unit/concurrency reservations together. Immutable root allocations cannot
be enlarged by replay. Root-owner revocation, any resource closure, expiry and
remaining capacity constrain fresh starts. Uncertainty retains concurrency;
known settlement releases it once, and spent call units remain charged. Load
validates retained usage and refuses corrupt counters or earlier formats.

See the [accounting ADR](atomic-effect-reservations.md). Controlled memory and
independent Redis tests cover final-unit competition, closure-before-start,
start-before-closure, response loss and immutable replay. Byte-capacity failure
does not partially spend. These are internal coordinator contracts, not live
permit enforcement, lineage verification or aggregate team funding. Retention,
emergency admission stop, backend failover qualification and runtime wiring
remain gates.

Local evidence: ten reservation contracts pass, plus the explicitly executed
independent-client Redis reservation contract. Existing Redis coordinator and
trusted-context contracts also pass against format 3. Required workspace
format/clippy/tests/all-target checks, UI lint/build, strict public docs build
and changed design links pass. PR #412 merged at `3d0fe750` after adversarial
review and all current-head CI checks passed. Integration run `37182689055`
explicitly executed the Redis reservation contract: one passed, zero ignored.

## Actual provider attempt boundary (merged)

The executor now offers a trusted per-attempt admission/settlement interface.
Admission runs after semaphore and retry-delay waits, using the actual selected
provider identity. A new durable guard is required for each invocation. Unknown
outcomes, settlement failure and cancellation retain obligations; automatic
retries stop on uncertainty. An opt-in library requirement refuses ungated
legacy/attachment/batch calls. Gated exhaustion does not write authority-free
legacy DLQ entries. See the [attempt-boundary ADR](provider-attempt-gates.md).

Nine executor contracts exercise provider calls against the real memory
coordinator, including controlled revocation during backoff, closure during
semaphore wait, timeout, cancellation and settlement failure. No production
permit evaluator or gateway/server profile is installed yet. The remaining
lineage, permit publication, governed result retention and full effect coverage
gates remain open.

Local evidence: required workspace checks pass with 3,378 tests, alongside UI
lint/build, strict public docs build and changed design links. PR #413 merged at
`fb5f590d` after adversarial review and all current-head CI checks passed.
Run `37184900637`'s stable test job executed the nine attempt-gate contracts.

## Current execution permits (merged internal library slice)

Exact root-profile permit revisions and terminal revocation now share the
coordinator's CAS/generation and pending control history with effect starts.
Bounded trusted issuance prevents subject/effect/limit expansion. Sealed root
provenance binds accepted revisions; registration checks original and current
complete tuples, input digest, fresh clock and counters after every CAS conflict.
Later broadening cannot expand an admitted job; current narrowing or revocation
denies its next fresh effect. Load reconstructs current records from retained
history. See the [permit ADR](current-execution-permits.md).

Eleven memory contracts pass, including controlled revocation/start and narrowed
limit races, root-creation interruptions and time refresh after contention.
The same three permit race orderings pass through independent Redis clients.
Format 4 refuses earlier coordinator state; migration/rollback require explicit
review. Production executor adapters, server scope provisioning, governed result
retention, principal/grant publication, mandates, delegated/represented authority
and public API/SDK/UI remain open. This does not complete Phase 2.

Local evidence: required workspace checks pass with 3,389 tests. UI lint/build,
strict public docs build, changed links and all four explicitly executed Redis
governance contracts pass. PR #414 merged at `1f2737d6` after adversarial review and successful current-head
CI run `37187992748`.

## Durable provider execution (merged internal library slice)

The [durable provider adapter](durable-provider-governance.md) now connects root
permits, trusted contexts and atomic attempt reservations to the executor. Stable
root/ordinal IDs prevent competing workers from sending the same attempt.
Immutable result evidence is retained before format-5 settlement; recovery can
repair accounting and return the saved result without a second provider call.
Unknown outcomes retain capacity. Backoff survives restart and fresh authority
checks block revoked retries. Historical inspection does not authorize new effects.

Fourteen focused contracts pass, including a real loopback HTTP invocation; the
independent-client Redis contract passes separately with zero ignored. Required
workspace checks pass with 3,403 tests, UI lint/build, strict public documentation
build and changed design links. Local adversarial review tightened the retained
start digest binding and verifies valid altered results against evidence pins.
PR #415 merged at `49dc7863` after adversarial review and successful current-head
CI run `37194958453`. Its integration job explicitly executed the durable provider
Redis contract: one passed, zero ignored. This does not complete Phase 2:
server/gateway coverage, lifecycle publication, workforce mandates, delegated
lineage, public APIs/SDKs/UI and reconciliation remain open.

## Current credential authority (merged internal library slice)

[Credential-specific ceilings](current-credential-authority.md) now share the
coordinator's publication/revocation protocol and effect-start CAS. Signed
context format 2 binds the accepted credential revision. Original and current
complete effects, execution eligibility, validity and root limits constrain each
fresh attempt; two credentials for one actor cannot combine grants. A required
credential profile refuses actor-only contexts at the durable provider boundary.

Ten focused governance contracts and three controlled race orderings pass,
including the explicitly executed independent-client Redis contract. Provider
backoff/revocation and missing-credential profile tests exercise the executor
integration. Required workspace checks pass with 3,415 tests, UI lint/build, strict public
documentation build and changed design links. Focused Rust 1.88 Clippy and both
explicitly executed credential/provider Redis contracts pass. PR #416 merged at `aaff9523` after adversarial review and successful current-head
CI run `37199915869`. Its integration job explicitly executed the credential Redis
contract: one passed, zero ignored. Server auth
publication/resolution, public APIs/SDKs/UI, workforce mandates and delegated
lineage remain open; coordinator format 6/context format 2 require reviewed
migration and compatible readers.

## Credential configuration snapshots (merged internal primitive)

[Atomic configuration snapshots](credential-configuration-snapshots.md) prevent
partial per-credential reloads and stale replicas restoring old grants. One scope
CAS publishes every credential, retires omitted IDs, binds the source version and
full fingerprint, and advances generation/control history. Current equal versions
observe; conflicting or older versions fail. Ownership survives retirement and
prevents individual overwrite or cross-source adoption. Freshness references bind
the coordinator incarnation; disabled credentials can have empty deny-all ceilings.

Ten focused contracts and both controlled snapshot/start orderings pass, including
an explicitly executed independent-client Redis test with zero ignored. Required
workspace checks pass with 3,425 tests, UI lint/build, strict public documentation
build and changed design links. Focused Rust 1.88 Clippy passes. PR [#417](https://github.com/penserai/acteon/pull/417)
was adversarially reviewed at head `5dabb316` and merged as `6de26ad3` after all
current-head checks passed (CI run `37210220443`). The explicit configuration
Redis step passed one test with zero ignored in integration job `111459709491`.
Public source at the merge commit was verified separately; this design-only slice
is not a public-book deployment or crates release. Coordinator format 7 requires
reviewed migration/parking. Execution-scope credential projection, public
scope/API/SDK/UI surfaces and complete effect coverage remain open.


## Shared authentication authority (server slice in progress)

[Shared authentication authority](shared-authentication-authority.md) wires the
source-epoch coordinator into real server startup, auth file watching, login,
JWT validation, API-key lookup and middleware. It rejects stale replicas using
obsolete security tables, binds authenticated identities to private configuration
observations, and checks current principal disablement. The optional Redis mode
uses explicit versions, keyed fingerprints and a dedicated auth control scope.
Normal startup cannot recreate missing authority state.

Focused memory contracts pass (seven tests). Independent-client Redis HTTP and
actual binary startup/watcher contracts were explicitly run together: two passed,
zero ignored. Required full checks passed with 3,432 workspace tests, workspace
Clippy, all-target compilation, UI lint/build, strict docs, catalog/permission
checks and changed Markdown links. Focused server Clippy passes on stable and
Rust 1.88. Public authentication/configuration docs and sanitized UI settings cover
the configured mode. PR/merge/publication evidence remains pending. This source-only epoch has no credential execution projection and does
not enable per-effect permits; qualified effect resolution, credential enrollment,
root capture, scope mutation stamps and complete execution coverage remain next.
