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

Remote agent work now has a state-backend journal for at-most-once cancellation.
The cancellation path rechecks current permits, closures, source service
binding, onward-agent grant, registry card and exact target binding. Ambiguous
delivery is retained as uncertain and reconciled only by safe task observation;
it is never automatically resent. The public route and all five SDKs preserve
unsupported, rejection, uncertainty, and observed terminal finality as separate
outcomes.
These verify endpoint least privilege and existing execution paths. Those first-slice checks do not
verify per-effect permits, revocation during a chain, closures or mesh execution.

## Remaining phase gates

| Phase | Current state | Next required evidence |
|---|---|---|
| 0: Inventory/coordinator | Exact resources, bounded CAS coordinator, multi-resource starts, root reservations and reviewed scope cutover implemented against configured state backends; memory/Redis/PostgreSQL/DynamoDB substrate contracts | Broader effect qualification, operational failover and emergency-stop proof |
| 1: Actors/context | Executor role, stable principals, credential enrollment, signed root contexts, shared authentication and private scope projection merged | Remaining deferred propagation, teams, memberships, mandates and delegated lineage |
| 2: Permits/checkpoints | Current permits/credentials/configuration snapshots, qualified static-webhook adapter and common gateway mediation merged; standalone runtime and authenticated governance management with SDK/UI merged | Broader effect coverage, workforce representation, deferred execution and recovery |
| 3: Closures/intervention | Serialized resource restrictions, durable control events and authenticated public close/reopen/revocation | Overlapping named closures, intervention recovery, drain/pause/cancel semantics and acknowledgments |
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


## Shared authentication authority (server slice merged)

[Shared authentication authority](shared-authentication-authority.md) wires the
source-epoch coordinator into real server startup, auth file watching, login,
JWT validation, API-key lookup and middleware. It rejects stale replicas using
obsolete security tables, binds authenticated identities to private configuration
observations, and checks current principal disablement. The optional mode uses the configured `StateStore`, explicit versions, keyed
fingerprints and a dedicated auth control scope.
Normal startup cannot recreate missing authority state.

Focused memory contracts pass (seven tests). Independent-client Redis HTTP and
actual binary startup/watcher contracts were explicitly run together: two passed,
zero ignored. Required full checks passed with 3,432 workspace tests, workspace
Clippy, all-target compilation, UI lint/build, strict docs, catalog/permission
checks and changed Markdown links. Focused server Clippy passes on stable and
Rust 1.88. Public authentication/configuration docs and sanitized UI settings cover
the configured mode. This source-only epoch has no credential execution projection and does
not enable per-effect permits; qualified effect resolution, credential enrollment,
root capture, scope mutation stamps and complete execution coverage remain next.


The follow-up removes the Redis-only startup gate: auth authority uses the same
configured `Arc<dyn StateStore>` as the gateway. Independent-client and actual
binary startup/watcher contracts now pass for PostgreSQL and DynamoDB Local as
well as Redis. The memory binary test passed with the Redis feature disabled.
Updated full checks passed with 3,433 workspace tests, Clippy, all-target
compilation, UI lint/build and strict documentation. Focused stable/Rust 1.88
Clippy covers PostgreSQL and DynamoDB features. PR #418 merged at `e7f1c5f` after exact-head review and successful CI run
`37219034763`, whose integration job executed all four backend contracts.
Documentation deployment `37220928565` succeeded and the published authentication
page includes the configured-backend guidance. These
contracts qualify auth-source behavior, not backend failover or all execution
classes. The memory backend remains process-local and ephemeral.


## Logical credential enrollment (server slice in progress)

Authentication can resolve an optional logical `authority_id` from the actual
API key lookup or signed JWT. Middleware carries a privately constructed binding
alongside the caller and shared configuration observation. IDs require stable
principals. API key rotation may reuse an ID only with identical complete policy;
conflicting grants, roles, principals or authentication methods fail validation
before publication or table replacement. JWT sessions pin the ID and cannot
silently adopt a replacement enrollment. The identity endpoint, all five SDKs
and UI expose the optional ID as inspection metadata.

Actual HTTP contracts exercise private key bindings and forged headers, atomic
rotation rejection, JWT enrollment migration and shared version conflicts. This
slice does not publish execution-scope credential ceilings, prevent an operator
from manually reusing an ID after removal, or enable per-effect permits. The next
integration must retain exact configuration/credential references and validate
terminal enrollment lifecycle through the execution coordinator.

Local validation passes: 3,438 workspace tests (zero failures; nine backend
contracts remain explicitly ignored in the default command), workspace Clippy,
all-target compilation, all five SDK suites, UI lint/build, strict documentation
and route/catalog checks. PR review, CI and publication evidence follow once
available. Backend storage is unchanged from the configured-state integration.


## Qualified execution-scope publication (host building block in progress)

[Execution-scope publication](execution-scope-credential-publication.md) implements
an immutable catalog tied to actual selected provider instances. Qualification
adds a protected binding-version route, so accepted roots cannot silently adopt
new destinations or failure contracts. Auxiliary action resources do not become
primary callable routes. A scope projector resolves each enrollment separately,
uses independent publication ceilings and fixed deadlines, and publishes through
the configured coordinator/state backend. Omitted credentials retire terminally.

The auth provider publishes all declared scope snapshots before the control
epoch and local table replacement, binds the canonical scope manifest to that
epoch, and retains the original scope references in privately constructed
middleware evidence. Same-version mismatches and stale bindings are rejected.
Real HTTP contracts cover result recovery, private binding freshness, lost
acknowledgments and partial two-scope publication followed by identical retry.

Adversarial review also rejects execution projection into the authentication
control scope before writes and isolates unrelated actors in a shared auth file
without expanding the scope publisher's independently authorized subjects.

Required local checks pass: 3,448 workspace tests, zero failures, nine backend
contracts ignored by the default command; workspace Clippy, all-target check,
UI lint/build, strict docs and catalog/route checks. PR #420 passed final-head CI,
merged, and its GitHub source was verified. This host building block has not installed production scope
configuration/startup, root capture or gateway effect enforcement; configured
backend projection contracts, public controls/SDKs/UI and workforce lineage
remain required. It does not close the complete execution or city phases.

## Static webhook qualification

Server startup constructs static webhooks through the same factory that can
produce their protected route bindings. The private keyed revision covers the
actual destination, canonical headers, network policy, loaded TLS snapshot,
timeout and transport behavior. Bindings carry their version resource before
catalog insertion. Redirects, proxies and transport retries are disabled; URL
query credentials are stripped from transport failure diagnostics.

Five focused contracts cover actual POST delivery, instance provenance,
configuration changes, canonical inputs, refused redirects and query secrecy
on transport failure. The workspace suite passes with 3,456 tests, zero failures
and nine default backend exclusions. Production scope declarations, catalog
installation, original-binding root admission and gateway effect enforcement
remain the next delivery gate. Other adapters and workforce lineage remain
required for complete platform coverage.

## Production execution authority (merged as PR #423)

The working implementation prepares execution scopes from actual registered
providers, publishes a fingerprint of the independently declared policy, and
admits roots from the original private authentication proof with explicit
permits. Durable admission pins the first identity, input, deadline and budget
across retries and partial-write recovery. Scope purpose is permanently reserved
through the configured state backend; reviewed `scope-upgrade` previews and CAS
application preserve prior accounting and uncertain attempts.

All selected-provider gateway calls now use one execution interface, including
reroutes, fallbacks and approval notifications. Its strict governed adapter
retains credential-required durable executors, checks actual provider instance
identity and requires explicit host-created invocation authority. The server
can produce this authority from private authentication and durable admission.
Refusals have no legacy execution fallback. Uncertain outcomes keep their
reservations and require reconciliation rather than automatic resend.

`dispatch_with_execution_admission` now carries a borrowed private admission
adapter explicitly to the final selected work. Real webhook delivery covers
direct, modified, deduplicated, throttled, rerouted and fallback actions. The
entry point refuses the legacy executor, and unauthenticated requests are
refused before operational state changes. Requests missing permits cannot
consume deduplication or throttle slots before a valid retry. Approval
notifications/retries receive no inherited request authority. Tests also cover
forged metadata, changed work under one operation key and revocation on replay.

Focused contracts verify actor/input/instance substitution refusal, credential
revocation before a send, completed replay without another send, and uncertain
work without resend. Server admission and cutover contracts have run against
memory, Redis, PostgreSQL and DynamoDB. PR #423 established library admission and gateway mediation. Its backend
contracts alone did not establish public HTTP dispatch coverage. The standalone
server integration below now supplies public immediate dispatch; deferred and
delegated work, broader adapters and workforce management remain outstanding.

## Standalone server runtime integration (merged as PR #424)

`feat/server-execution-authority` installs the qualified execution mediator,
credential projection and explicit deployment permits into the standalone
server. Authority coordination, signed contexts, root budgets, replay bindings
and provider records all use the same configured `StateStore`; this is not a
Redis-only subsystem. The runtime accepts only private middleware authentication
and exact explicit permit revisions for selected provider work.

Current local runtime evidence: `execution_http` passes with memory and no Redis
feature, and with PostgreSQL and no Redis feature. The PostgreSQL test restarts
the actual server and verifies that replay returns the saved outcome with no
second network send. Both also verify unauthenticated denial, missing permits,
changed-input replay conflicts, an unissued-permit refusal followed by a valid
retry of the same action ID, and batch execution. CI now explicitly schedules
these HTTP contracts. PR #424 merged at
`681431408fc28382ac4a3efe1086c1ff58537866` after adversarial review of exact head
`d61d7bba577e4e3818199e4a38a7533c8d9dec32` and all 28 final-head checks completed
(25 success, three expected skips). The merged tree matches the reviewed head.
Integration run `37290238915` executed the memory HTTP suite and all four
PostgreSQL-enabled HTTP tests, including restart replay and invalid-startup
publication refusal. Deploy Documentation run `37293384388` succeeded for that
merge; an actual HTTP fetch of the public execution-permits page verified its
permit header, PostgreSQL/DynamoDB guidance, SDK method and signing-key instructions.

Replay markers are reserved after successful final-work admission. The initial
HTTP conflict check reads an existing marker without creating one, so an invalid
permit cannot poison a subsequent authorized retry. Admission or replay storage
failures prevent provider execution. Batch scope verification follows the size
and grant checks to bound backend work.

All five SDKs now expose typed explicit permit options for single and batch
dispatch, with shared wire-fixture tests and HTTP refusal preservation. The UI
accepts permit IDs and revisions separately; configuration exposes an enabled
flag without authority details or secrets.

Open work remains: governed durable
receipt responses, deferred execution, approvals, shared child funding and peer
invocation must retain private authority and lineage. Teams, memberships,
personal agents, team representatives, mandates and public governance management
remain required. The selected-provider integration does not establish complete
coverage of auxiliary effects or the whole workforce vision.

Startup preflights local authentication tables before installing authority;
explicit permits publish only after authentication scope projections and rule
loading succeed. The PostgreSQL HTTP test also verifies that a failed startup
with invalid credentials leaves both control and execution scopes absent.

## Evaluated control-write boundary (merged)

A bounded, independently evaluated control ceiling now guards intervention
writes and their idempotent replay within the coordinator CAS loop. Current
scope stamps, actor revocation, complete policy resources/subjects and host time
are checked before accepting a change. This supports closures/reopening and
permit/credential/subject revocation without a Redis dependency or schema change.
See the [control-write ADR](evaluated-governance-control.md).

This is a prerequisite for public management adapters, not a public governance
API or closure lifecycle. Private authenticated management projection, server
routes, SDK/UI integration, overlapping named closures, intervention recovery,
teams/mandates, deferred authority and autonomous mesh remain required.

Local evaluated-control evidence: all twelve targeted tests pass, including explicitly
executed independent PostgreSQL and Redis contracts. Execution management refuses
unclaimed and authentication-control scopes. Public adapters remain unimplemented.
Required local checks pass: 3,503 workspace tests, zero failures and twelve
explicit default exclusions; all twelve targeted control tests pass with backend
exclusions enabled. Formatting, workspace Clippy, all-target compilation,
UI lint/build and strict documentation build also pass. PR [#425](https://github.com/penserai/acteon/pull/425) merged as
`95595c81f4428a3184a50d671a73056671effb2b` after adversarial review of
`6d952df6e93a02bdd4e5b361e639df87d0caf4b3` and successful final-head CI
`37301063111`. Its integration job `111733626575` explicitly ran all twelve
control tests, including independent PostgreSQL and Redis contracts. The merge
tree matches the reviewed tree. Documentation deployment `37303111920` succeeded
for that merge; the public execution-permit guide returned HTTP 200 with its
permit header and title verified. This does not complete the city objective.


## Authenticated governance management (merged and published)

PR #426 adds authenticated scope inspection, bounded permit issuance,
and resource close/reopen plus permit/credential/subject revocation through three
public HTTP operations. Independent deployment managers constrain each actor's
subjects, qualified routes, issuance bounds and intervention rights. Private
middleware authentication and current authority are rechecked; caller labels
and wire payloads cannot create management authority. State uses the configured
`StateStore`, including memory without the Redis feature and PostgreSQL restart
coverage.

All five SDKs have typed methods and shared wire/refusal contracts; the operator
UI exposes scope inspection, issuance and reasoned closure/reopening/revocation.
The governed-city guide and runnable simulation use a real server, native Python
SDK and HTTP webhook receiver: two authorized sends, zero unauthorized sends
across replay, closure, reopening, permit revocation and credential offboarding.
No model or autonomous peer invocation is claimed by this scenario.

Local validation passes: 3,510 workspace tests across 100 suites with zero
failures (twelve backend-dependent tests excluded by the default invocation),
six explicitly enabled PostgreSQL HTTP tests, both evaluated-publication replay
and CAS-expiry tests, nine neural-observability example tests, all five SDK
suites, desktop/mobile governance browser checks, workspace Clippy, all-target
compilation, UI lint/build and strict documentation build. The CI workflow now
runs the real governed-city scenario and rejects guide/configuration drift.
PR [#426](https://github.com/penserai/acteon/pull/426) merged as
`de655dadb6f243883bb426fb5d9044a923af2087` after adversarial self-review of
`66078bbcf34131bc8dc81577f66bda27368828d2`. All 25 final-head checks succeeded;
three expected checks were skipped. Merge and reviewed trees match. CI
`37318088861`, integration job `111789821803`, explicitly ran the current private
management tests, the no-Redis city simulation and six PostgreSQL HTTP tests.
The downloaded city artifact confirms two authorized HTTP sends and zero
unauthorized sends. Documentation deployment `37321725077` succeeded for the
merge commit; both public governance management and governed-city guides returned
HTTP 200 with their endpoint, configuration, SDK and result markers verified.
Review/publication evidence is recorded in the PR.
Independent overlapping closure records, pause/drain/cancel acknowledgments,
deferred context propagation, shared child funding, teams/memberships/mandates,
personal and team agents, and autonomous registry-backed A2A invocation remain
required to complete the objective.

## Workforce authority integration (working branch, not published)

The `feat/workforce-authority` branch adds versioned teams, direct memberships,
agent ownership, duty assignments and representation mandates to the configured
state backend's coordinator. Team identities remain descriptive organizational
references, not shared authentication principals. Ownership and roster membership
alone issue no execution permits. Represented permits and their mandate bindings
are published together through one coordinator CAS.

Signed contexts retain the actual actor, authenticated initiator, represented
party, exact job class, mandate revision and explicit ownership/membership/
assignment dependencies. Provider admission resolves required representation
from permit history and the actual prepared route; payload fields cannot select
another initiator, team, job class or mandate. This first provider adapter uses
the qualified route's exact action type as job class. Delegated initiators still
require a separate verified handoff protocol.

The coordinator format advances from 8 to 9 through explicit reviewed cutover,
preserving incarnation, history and accounting. Signed format-2 actor contexts
remain readable without gaining representation; new contexts use format 3.
Startup does not silently add workforce authority to legacy records.

Current targeted evidence includes independent PostgreSQL and Redis clients,
membership offboarding while standing team work continues, ownership transfer,
team disbanding, management expiry during CAS conflict, lost acknowledgments,
offboarding before effect registration, exact job/initiator checks and input-bound
proofs. Authenticated server admission retains one original context on replay
and denies the next effect after mandate revocation without spending budget.
Revoking a human also denies jobs with an explicit ownership or membership
dependency on that human, without disabling independent standing team work.
Manager ceilings independently bound job classes as well as teams, principals,
effects, validity and spending.

Local broad regression evidence: 3,521 workspace tests across 101 suites passed
(fourteen backend-dependent tests excluded by that invocation), all-target
compilation, UI lint/build and strict documentation build. Following the final
human-dependency revocation fix, workspace Clippy and all 26 explicitly enabled
workforce/context tests passed. Nine server preparation tests passed with
PostgreSQL enabled and Redis disabled, including independent-client authenticated
workforce context recovery and the existing reviewed-cutover/admission contract.
These are working-tree results, not final-head CI or publication evidence.

The working tree now also includes authenticated public workforce management,
independent deployment bounds, all five client SDKs, the operator UI and a real
HTTP workforce guide/simulation. Final-head checks, adversarial review and
publication remain required. Registry integration is a later phase. Shared team/descendant funding, deferred lineage,
autonomous A2A invocation, overlapping closures and federation qualification
remain gates for the full objective. These core tests do not establish those
remaining guarantees.


### Working-tree workforce management and real HTTP qualification

The public slice now includes independent workforce deployment bounds, scoped
authenticated inspection and mutation routes, ten typed management changes,
all five native SDKs and an operator UI. The shared fixture exercises every
change variant; real server contracts separately establish authorization.

Admission now intersects the deployment root ceiling, all selected exact permit
ceilings and the required mandate. The server test admits a one-call permit
under a two-call mandate and five-call deployment ceiling, preserves its original
context and limits on retry, and refuses work after mandate revocation. A focused
coordinator test also checks concurrency and deadline intersection, wrong actor,
wrong effects, expiration, revocation and absence of root allocation by the
projection itself. Current actor revocation also prevents issuance of a new
represented permit without publishing any binding or authority event.

The new `examples/agent-workforce/run.py` uses typed SDK management and dispatch
against an actual server and HTTP receiver. Its run produced eight authorized
webhook deliveries and zero unauthorized deliveries. A retry adds no delivery;
removing Maya's Reliability membership stops human and personal-agent work while
an active Release membership cannot replace it. Standing investigator and service
mandates continue independently. A resource closure, mandate revocation and team
disbandment each stop the expected subsequent calls. It makes no model calls and
is not autonomous mesh qualification. CI now runs both public city scenarios and
uploads their evidence.

Desktop and mobile browser tests also check that a represented permit fits both
mandate and manager budgets, and that an unavailable mutation is retried manually
with its original exact request and change ID. Full Python, TypeScript, Go and
Java SDK tests, UI lint/build, workspace Clippy and strict documentation build
passed locally. The final broad Rust run passed 3,532 tests across 101 suites
with fourteen backend-dependent tests excluded by that invocation; 27 separately
enabled governance/backend tests and 22 real server tests passed, the latter with
PostgreSQL enabled and Redis disabled. All-target compilation passed. Reviewed-head
CI, PR merge and public publication still require verification; this paragraph
does not claim shipping.


### Descendant execution foundations — in progress, uncommitted

The descendant branch now implements signed child provenance and immutable
parent-to-root budget links in the configured `StateStore`. Same-actor children
retain the original initiator, representation and pinned dependencies. Every
registered effect charges the leaf and all ancestors in one coordinator CAS;
settlement releases concurrency once while uncertainty retains it. Current
permit, credential and mandate limits apply across sibling spending.

The coordinator wire protocol advances to 10 through the explicit reviewed
cutover. A protocol 9 fixture with workforce state, live uncertain accounting
and an earlier 7→9 history entry preserves those records through 9→10 migration.
Signed context format 4 retains readers for existing root formats 2 and 3.

Local governance validation passed 120 tests across 11 suites, with twelve
backend-dependent tests excluded by the default invocation. This includes lost
acknowledgments at admission, context persistence and ledger allocation; signed
ancestry/input/scope tampering; branch exhaustion; sibling concurrency;
exactly-once concurrency release; offboarding; bounded depth/count; and clock
resampling after a same-generation CAS conflict. Workspace Clippy and all-target
compilation passed. Separate PostgreSQL and Redis descendant contracts were
attempted but both connections were refused by this session's network policy
(`Operation not permitted`); they remain unverified.

This is a foundation, not a shipped workflow feature. Qualified plan admission,
actual step input binding, persisted workflow/chain references, restart and
wait handling, SDK/UI surfaces, a real durable-backend scenario, full release
checks, adversarial review, final-head CI and publication remain required.
Cross-principal autonomous A2A and explicit team funding retain their separate
implementation gates. No public child body supplies an actor or payer.

### Qualified plans and durable worker provenance — in progress, uncommitted

Complete chain qualification now binds original semantic input, pinned
configurations and actual provider revisions/footprints, including parallel,
sub-chain and cancellation routes. Signed children retain chain closure
restrictions at the same CAS that admits provider effects and charges ancestors.
The strict credential-requiring provider mediator accepts the private planned
child adapter; ordinary dispatch origins cannot reuse it.

The new `PlanHandoffStore` persists job provenance and logical child attempt
identities through the configured `StateStore`, with deployment payload
encryption support. Replacement workers requalify pinned definitions and verify
the original signed root. Lost job/call write acknowledgements and competing
replicas preserve identities; changed inputs, limits, route revisions and
corrupted definitions are refused. The tests exercise actual in-process provider
calls under the recovered root, with closure between calls and shared spending.
This is not evidence of a real HTTP chain or autonomous A2A mesh.

Authenticated plan-root admission, chain-engine job ownership and worker wiring,
instance-cancellation fencing, lifecycle retention, SDK/UI inspection/control,
a real durable-backend multi-step scenario and release verification remain
required. Independent PostgreSQL and Redis clients have the same encrypted
handoff test contract; those externally connected cases remain unverified in
this network-restricted session. Redis is an optional StateStore implementation,
not a required backing service for these features.

Validation for this checkpoint passed 183 governance/executor tests across 16
suites, with fifteen external-backend cases ignored and one real HTTP test
explicitly filtered. The latest targeted handoff run passed six tests, including
expiry observation without renewed child admission and encrypted restart, with
the two external-backend handoff cases ignored. Workspace Clippy, current
all-target compilation, formatting and diff checks passed. Full workspace
runtime tests and release checks have not been completed for this branch.

### Authenticated plan-root admission — in progress, uncommitted

Independent deployment declarations now bound concrete chains and their eligible
principals; deployment permits explicitly name chains as well as provider
routes. Credential projection requires a dispatch-capable role and matching
scope for each declared principal. Complete plans require both chain starts and
all qualified provider effects. Provider grants alone do not issue chain rights.
Canonical chain/subject bounds participate in the security policy fingerprint;
empty declarations preserve the provider-only fingerprint shape.

The server's private plan-root path shares the existing authenticated provider
root's current limit attenuation, representation and credential admission. The
runtime derives root admission identity from the authoritative job UUID and
persists the qualified plan using the configured state backend and optional
payload encryption before returning it for handoff. Replay retains the original
root; changed candidate keys cannot reset its allocation. Changed inputs,
undeclared sub-chains and revoked credentials refuse fresh admission, while
historical recovery preserves the original evidence.

The chain engine still needs to establish trusted work-record ownership and use
this root/child path at its actual provider boundary. Chain-aware management and
workforce job-class declaration surfaces, per-instance cancellation fencing,
retention, public SDK/UI/guide coverage, a real durable-backend scenario and the
remaining full-city phases stay open. This checkpoint does not claim a shipped
protected workflow or autonomous A2A mesh.

Current server validation passed eight execution-preparation tests with
PostgreSQL support compiled, five external-backend cases ignored and the real
network-listener test explicitly filtered. The plan contract uses actual private
authentication middleware, deployment permit publication, original-root replay,
changed candidate/input rejection, role/scope denial, replacement-runtime
recovery and credential revocation. Both PostgreSQL and Redis have independent
client variants of the same contract, but their connections remain unverified.
Workspace Clippy and all-target compilation passed; full release validation and
publication remain open. A separate server build directory preserves the user's
main server executable; the dependency's supported local Swagger asset setting
allows compilation from the existing cached archive without network access.


## Connected provider steps in the chain engine (unreleased)

The governed chain engine now captures a credentialed plan root and persists its
handoff before publishing root work. Sequential provider steps and flat parallel
provider groups use planned child admission at the existing provider boundary.
The execution driver retains circuit breakers, metrics and provider attempt
registration; a refusal never falls back to an ungated call. State, contexts,
child identities and provider evidence use the configured `StateStore`.

Workers recover the admitted definitions from the persisted plan rather than
adopting a live registry edit. Logical step identities remain stable across a
failed result-projection write, allowing an already completed provider result to
be recovered without another provider invocation or budget charge. Closures of
the enclosing chain are checked at each actual provider start. Missing handoff
records and modified original inputs cannot borrow authority from chain labels.

Five actual-engine tests pass with in-process providers: lost projection plus a
registry edit, closure between steps, modified work input, missing provenance,
and parallel calls charged to one root. Server compilation and production
Clippy pass. These tests establish local behavior, not distributed backend parity.
The real HTTP provider regression cannot bind a socket in this restricted
session (`Operation not permitted`); it remains an explicit verification gate.

Remaining delivery includes parking ambiguous/in-flight receipts, qualified sub-chain and nested parallel
execution, cancellation fencing and independently authorized cleanup, deferred
worker adapters, chain-aware management/workforce declarations, public surfaces,
independent backend qualification and reviewed release. Unsupported execution
profiles are refused before root work is admitted; that temporary restriction
does not reduce the city platform's intended scope.


### Historical receipt recovery after authority stops (unreleased)

A separate observation boundary now reads a retained logical call without
allocating work, verifies signed child provenance and inspects the exact
qualified provider receipt. Completed evidence can repair a chain result after
credential revocation or deadline expiry. Missing evidence still requires fresh
child admission; unavailable or conflicting evidence refuses without an
execution fallback. Inspection may reconcile already recorded settlement, but
it never invokes a provider or registers a new attempt.

Eight actual-engine tests pass, including completed-result recovery after expiry
and revocation and refusal of a corrupted retained-call input. Recovery asserts
that no new root, child budget link or attempt is created and that the following
step remains denied. The corrupted-record test advances the injected clock past
the abandoned chain lock lease before retrying. Current governance/executor
regression: 183 passed, 15 external-backend tests ignored and one socket-bound
HTTP test filtered. Production Clippy passes for governance, executor, gateway
and server. The earlier pre-observation regression passed 776 tests; it is not
represented as verification of later changes.

Receipt observation does not yet provide a durable parked/reconciliation state
in the chain engine. In-flight or uncertain work is refused, and implementing
that lifecycle without inventing a fresh attempt identity remains a delivery
requirement, alongside cancellation fencing, sub-chains and the other release
gates above.


Current gateway regression also passes 473 tests across the library, provider
mediation, chain fencing and chain recovery suites. Final targeted verification
passes the qualified-plan contracts and all eight governed engine tests after
removing encoding from the observation path. The final production lint check
passes for the four affected platform crates. These are scoped local checks;
full release validation and external backend qualification remain open.


## Durable provider receipt parking (unreleased)

Pending provider work now has a first-class `ProviderPending` outcome with the
original execution ID, attempt count and typed receipt state. Sequential and
flat parallel provider steps persist `waiting_provider` and an observable
provider wait before returning. Polling preserves the original logical attempt;
uncertain work does not become a failed step, consume a new allocation or lose
its retained charge when the workflow deadline passes.

Parallel groups retain known sibling results and inspect started receipts after
a group timeout. Governed bounded batches drain their started calls; `any` and
fail-fast decisions prevent later batches from starting. A decided group that
still has uncertain work polls only its retained pending branches. New status
filters, chain wait details, SDK outcome decoding and UI visibility accompany
the engine state. Generic state-backend storage remains authoritative.

Current engine evidence: thirteen tests pass, including uncertain work across
restart/expiry, in-flight completion after replacement and revocation, a mixed
parallel group, an `any` winner with an uncertain loser, and group timeout.
Provider executor regression passes twenty tests (one external backend ignored
and one socket-bound HTTP test filtered). Release checks and backend parity are
still required. An operator-facing reconciliation protocol must separately
establish trustworthy external evidence; polling cannot invent that evidence.


Final local parking verification: 1,360 tests pass across core, governance,
executor and gateway (15 external-backend tests ignored; the socket-bound HTTP
receipt test filtered). Workspace production Clippy, all-target compilation,
strict public docs build and UI lint/build pass. Node model decoding passes 51
tests and type checking; Python and Go pending-receipt cases pass. All Java SDK
sources compile at the Java 21 language level and 12 outcome-decoder JUnit tests
pass using the cached JDK/dependencies directly. Gradle itself cannot start its
socket-based file-lock service in this restricted session; that build remains a
separate release check. No commit, PR, merge or publication is recorded here.


Execution-instance cancellation fencing (unreleased): governed chain cancellation
commits a permanent root/descendant budget fence through the configured StateStore
coordinator before chain status projection. Child admission and effect registration
check the entire budget ancestry; known settlement is still permitted. Pending
provider wait metadata and uncertainty charges survive cancellation. Generic
resource controllers cannot use the host-only cancellation operation. Outstanding
work must be reconciled from independently trusted evidence; cancellation is not
completion evidence. This change does not close the remaining autonomous A2A,
workforce management, deferred adapter, or trusted reconciliation gaps.

Cancellation adversarial review found and removed a dependency on the current
provider catalog. The restriction-only handoff reader verifies retained signed
root/input/permit provenance without qualifying or granting any new effect, so
provider removal and original-permit expiry cannot disable cancellation. Final
focused verification passed 49 tests across chain execution, qualified plans,
controller bounds and descendant accounting; six external-backend checks remain
ignored. Workspace Clippy, all-target compilation, UI lint/build and strict docs
build passed. The exact full workspace test command aborted in the unmodified
socket-level Kafka mock transport suite (librdkafka assertion, SIGABRT), so the
full test/release gate remains incomplete. No publication is claimed.


Trusted finality reconciliation (unreleased): provider hosts can install a local
verifier for independent finality receipts. The HMAC implementation binds signed
context/action/attempt/nonce/binding identity with dedicated issuer keys; proof
size, schema and signature are bounded and checked. Finality is a source contract
covering all effects and future deliveries, not a signature on an empty lookup.
No provider is invoked and no expired work permission is borrowed for verification.
The coordinator pins a separate immutable attestation, original evidence and prior
unresolved state, preserving spent units and releasing concurrency once. Accepted
history is observable without reusing verifier keys. A candidate awaiting ledger
acceptance is reverified; known ordinary evidence takes precedence over it.

The actual chain engine has a reconciliation recovery test: an ambiguous metrics
call parks; independent finality resolves that exact receipt; the same logical
step repairs without another send and the next logs step obtains fresh admission.
Management/proof-ingress endpoints, qualified external issuer/probe transports,
and SDK/UI management controls remain required platform work. Historical
binding-independent reads are implemented in the later checkpoint below. No globally permissive force-settlement endpoint was introduced.


## Configured-backend provider contracts (unreleased)

The provider multi-client contract now takes two `Arc<dyn StateStore>` clients,
with memory, Redis, PostgreSQL and DynamoDB entry points. PostgreSQL and DynamoDB
CI steps explicitly run the otherwise ignored tests against their disposable
services. Each fixture isolates its state using a generated prefix or table; it
does not flush a shared backend. The contract observes one started provider call
from another client, permanently cancels the root, accepts independently signed
finality after expiry, checks exact replay and recovery without a verifier, drains
the late worker, and verifies one send, one spent unit and zero active attempts.

Fresh starts also atomically seal the original operation envelope. Reads validate
that retained digest, including encrypted envelopes, so delivery IDs excluded
from semantic input equivalence cannot be rewritten after execution starts.
Legacy starts are not retroactively sealed. At this checkpoint, the separate
receipt-history reader was pending and the absent module declaration was removed.
The historical-reader checkpoint below implements that interface without a live
provider binding.

Local validation and external-backend execution must be recorded separately;
adding CI steps does not prove those services have passed on the final branch.


The existing qualified-plan handoff contract now has explicit Redis/PostgreSQL
CI steps and a DynamoDB Local wrapper using independent clients and encrypted
handoffs. The broad shared contract exercises reconstruction, pinned definitions,
child contexts, shared budgets and tamper refusal without backend-specific
execution code. Persistent-backend runtime outcomes still await CI execution.


## Historical provider receipt access (unreleased)

`HistoricalProviderStore` is a read-only host primitive over the configured
`StateStore`, scope coordinator and retained signed contexts. It has no provider,
registry, executor, clock or verifier. Lookup by execution UUID or original
context verifies authenticated ownership, scope, original input, permit selection,
coordinator incarnation, attempt footprint and original-operation seals. Accepted
original results and reconciliation records must match ledger-pinned digests.
Uncommitted proofs and unacknowledged known bodies remain pending; the reader
never repairs state or releases charges. Missing pinned records fail closed.

Its projection preserves original and finality evidence separately, reports an
inherited permanent cancellation fence, and distinguishes sealed, legacy and
unstarted work. Legacy records do not expose unsealed delivery/retry
metadata; binding metadata requires a whole-operation seal or pinned result/finality, and unsealed retry settings cannot hide later protocol attempts. The
existing shared provider and qualified-plan contracts now exercise this reader
through their backend fixtures, including parent cancellation and encrypted
record recovery. The new API is a Rust host integration; authenticated management
HTTP routes, SDK methods and UI views still require dedicated adapters and review.


## Authenticated historical management boundary (unreleased)

The provider history management route now reads the configured `StateStore`
through the independent historical reader. An explicit, default-denied
`can_read_history` deployment permission is bounded by the manager's subject
allowlist and the original private authentication proof. Current management
scope, policy, credential and authority are checked before and after observation.
Read-only managers may omit live route grants and cannot intervene or issue
permits merely because they can read evidence. Omitted false permissions retain
the existing policy serialization. The response has dedicated public Core types
and a typed Rust client; all five generated operation catalogs include the route.

Full history-only deployments with zero live routes, the remaining dedicated SDK
models/helpers, UI access and proof-ingress management remain required. This
checkpoint does not certify external backend runtime tests or publication.


## Provider retirement with authenticated history (unreleased)

Execution scopes now support an explicitly validated `history_only` mode. It
connects to existing state with retained reviewed effects, context keys and
optional payload encryption, and needs no live provider registrations. The
normal catalog/projector/mediator constructors still reject empty executable
installations; separate history construction produces an empty catalog,
execution-disabled projected credentials and a mediator that refuses every
provider call. History-only declarations prohibit bootstrap, routes, chains,
deployment permits, permit issuance, intervention and workforce management.
Only explicit history readers with bounded subject allowlists can be installed.

The server contract persists a real signed operation with admission interrupted
before any network send, retires the actual registration, advances the security
revision, denies the old proof and verifies the retained receipt through the
production HTTP router. It proves that an out-of-allowlist execution is hidden,
current credentials carry no executable effects, retained authority cannot invoke
the removed provider, and receipt reads change no accounting or authority state.
Additional tests cover invalid history-only declarations and refusal to create
missing state. These are local memory/in-process checks; they do not certify
external backend execution or publication. The earlier checkpoint's zero-route
startup gap is closed; dedicated non-Rust history SDK helpers, UI access,
revocation-during-read tests and qualified proof-ingress remain required.


The Governance UI now exposes receipt inspection only when the returned
management bounds include `can_read_history`. Its query is keyed by canonical
scope and provider execution UUID. It resets selection on scope changes, hides
cached results on read errors, and renders original and accepted reconciliation
evidence separately. It contains no provider invocation, retry or settlement
control. UI lint and production compilation pass; browser smoke coverage still
requires an environment able to bind a preview server.

The retirement contract also keeps a production-style credential-requiring
worker alive across cutover and confirms it cannot start an attempt using the
old adapter and context. Its admission is refused before a send, with unchanged
accounting. Historical projections expose the signed participant identity,
which the UI displays without deriving an actor from action labels.


## Typed retained history across SDKs (unreleased)

All five SDKs now expose dedicated provider history helpers and typed responses;
Python supports both synchronous and asynchronous clients. Every nested outcome
uses the existing dispatch decoder, while original evidence and accepted
reconciliation remain separate. The shared fixture covers all five receipt states,
nullable unstarted metadata and binding, zero timestamps, signed participant
attribution, cancellation fences, and an original failed observation followed by
an accepted executed resolution. Rust checks that the fixture round-trips through
the public DTO. TypeScript, Python and Go transport tests assert the scoped,
authenticated GET; Java tests use the production JSON mapper.

This closes the preceding checkpoints' dedicated-client gap. It does not close
revocation-during-read race coverage, qualified proof-ingress management, live
browser verification, or final-head CI, adversarial review and publication.


Local validation for this checkpoint: 11 TypeScript history/governance tests,
28 Python governance/platform/pending-provider tests plus three subtests, Go's
in-memory-transport history and capability contracts, Java's production-mapper
history and capability contracts, and the Rust public-wire fixture test pass.
Node type checking, lint and production build, Python scoped lint, UI lint/build,
strict public docs, and the 200-operation permission/catalog checks pass. The
full Node suite reports 199 passes and three existing bus/SSE failures caused by
socket binding being denied (`EPERM`). Java's new HTTP history contract compiles
but must run in network-capable CI. These limitations remain release gates.


## Authority changes during retained-history reads (unreleased)

A backend-agnostic read barrier now extends the existing `FaultStore` test adapter.
It pauses `get` or `get_versioned` before or after the backend observation and
signals that exact cut. This is test infrastructure, not a runtime retry policy.
Production-router contracts pause a real signed prepared receipt, then apply role
offboarding, execution credential or subject revocation, authentication-source
disablement, authentication-only epoch rotation without refreshing the execution
projection, resource closure, or management expiry before resuming the read.
They compare both execution and authentication authority records before/after
the resumed read to check that reads do not mutate authority or accounting.
Unavailable authentication authority fails with HTTP 503 without recreation;
expiry takes precedence over both corrupt evidence and a missing auth authority. An unchanged read returns
its receipt; a new read after a resource closure can still inspect retained work.

The tests reproduced two defects before fixes: corrupted evidence plus a newly
expired reader returned a storage error before rechecking access, and a reader
disabled at the original authentication authority received the receipt through an
otherwise-current execution projection. History errors are now held until final
management revalidation. Host-created authentication proofs retain their original
trusted authority coordinator and verify its original epoch and current subject
eligibility; management reads check both source and execution observations.
No wire value chooses the authority, and an obsolete proof is never refreshed to
new permissions. These are independent authoritative checks, not a multi-record
atomicity guarantee or a substitute for execution-scope start fencing.

Qualified finality-proof ingress, external-backend and final-head CI, browser
verification, adversarial review, merge and publication remain outstanding, along
with the later delegation, registry-driven A2A, funding and federation phases.


The final local management suite passes ten tests, including one production-router
contract with eleven explicitly ordered read cases. Both original defects have
retained failing-before-fix logs. The shared read-barrier suite passes the four
`get`/`get_versioned` before/after combinations and a dropped-controller contract.
Provider persistence/reconciliation and qualified-plan regression suites pass
with service-dependent tests explicitly ignored and the known socket-binding
HTTP test filtered. This is local evidence; external backend runs and the full
release gate are still required.


## Evaluated reconciliation authority (unreleased)

The coordinator now offers `reconcile_attempt_evaluated` for independently
permitted acceptance of finality evidence. Trusted host inputs bound the operator,
affected subject IDs, complete registered resource footprint and validity window.
The host must first verify the original signed operation's full principal identity:
legacy coordinator attempts contain subject IDs, not principal kinds. These inputs
are not deserializable request authority and do not install a proof verifier.

The authorization check runs before identical-proof replay and on every settlement
CAS retry. A closure invalidates an earlier evaluation; a freshly authorized operator
may still accept evidence about the earlier effect while the resource stays closed.
This performs no provider send, creates no attempt, preserves original evidence and
spent units, and releases concurrency once. The privileged library reconciliation
entrypoint remains available for trusted adapters and must not be used as a public
management authorization boundary.

Five local memory-backed contracts pass: full subject/resource bounds, exact-proof
replay and expiry, closure during CAS, expiry after a version-only CAS conflict,
revoked-operator replay after lost acknowledgment, and invalid bounds/token refusal.
The accounting assertions check retained capacity after refusal and unchanged spent
units after successful settlement. Targeted Clippy passes with warnings denied.
This uses the configured StateStore abstraction; it does not establish external
backend qualification for this new API.

The next required integration is a guarded executor path that does not implicitly
repair or accept staged evidence before management authorization, followed by
host-installed qualified verifiers, an independent default-denied management
capability, server proof ingress, SDK/UI support and an end-to-end simulation.
These remain outstanding together with the broader workforce implementation phases
and final-head review, CI, merge and publication.


## Guarded executor finality acceptance (unreleased)

`GovernedProviderExecutor` now exposes independently evaluated correlation and
finality-acceptance methods. Correlation validates the signed original owner,
complete retained attempt history, installed provider binding and current management
bounds using read-only history. It never acknowledges an interrupted result or
accepts a staged proof. Acceptance verifies the exact latest attempt and qualified
local proof, refuses a known ordinary result even when its ledger acknowledgment is
pending, and settles through the evaluated coordinator CAS.

Management-staged resolution records carry an explicit durable authorization
requirement. Ordinary receipt observation and trusted legacy reconciliation do not
adopt these candidates. A lost proof-write acknowledgment or a closure that wins
before settlement leaves a staged proof without releasing capacity. Acceptance can
resume under freshly evaluated operator authority. Accepted-proof replay checks
operator authority before observing the retained final result.

The coordinator can atomically pin a verified original uncertain result whose
acknowledgment was interrupted, together with the separate finality link. It checks
the expected original pin, never replaces an existing pin, preserves spent units,
and releases concurrency once. No intermediate privileged acknowledgment is needed.
The stored proof's `resolved_at_ms` is its verification/staging time; an operator
acceptance audit record and acceptance timestamp remain required for public ingress.

Six executor contracts exercise closure-before-settlement, restart with a staged
management proof, original result acknowledgment interrupted at a controlled store
barrier, proof-write and settlement acknowledgment loss and operator revocation, known success or
rejection precedence, and invalid proof or incomplete operator bounds. They assert
one original provider invocation and no reconciliation invocation. The tests use
reserved execution scopes as production does; unclaimed legacy scopes cannot mint
new evaluated management authority.

Public ingress remains outstanding: qualified verifier installation and lifecycle,
an independent default-denied management permission, original authentication-source
revalidation, operator acceptance audit, HTTP and SDK/UI surfaces, and end-to-end
simulation. Retired binding/settings reconciliation also needs explicit host
qualification rather than constructing a current driver for unrelated retained work.
External backend qualification, all final-head release gates and later workforce
phases remain open.


## Atomic reconciliation acceptance audit (unreleased)

Evaluated reconciliation now commits the accepting operator's full principal,
authority incarnation/generation and authorization decision time with the finality
link, original evidence pin and concurrency release in one configured-StateStore
CAS. Failed writes retain no attribution; lost acknowledgments retain the committed
record. Replay checks current operator authority but preserves the original actor,
original authority stamp and original acceptance time. Historical actor revocation
does not erase the audit record or invalidate retained evidence.

Recovery rejects orphan attribution, foreign incarnations, impossible generation
ordering and negative timestamps. Management-marked accepted proofs require an
operator acceptance record; ordinary privileged adapter settlements retain no
invented actor. Both the live executor projection and the independent read-only
history projection expose the optional acceptance record. The Rust public DTO,
Python, TypeScript, Go and Java SDKs preserve it, including legacy omission, and
the UI separates acceptance from proof-recording time.

Contracts cover acceptance CAS failure and acknowledgment loss, another authorized
operator's replay without rewritten attribution, corrupted audit metadata, and
read-only recovery with plaintext and encrypted retained provider evidence. The
shared SDK fixture includes both unattributed adapter settlement and attributed
operator settlement, with distinct proof and acceptance times.

Qualified verifier lifecycle, retired binding/settings support, the independently
permissioned public proof-ingress API, original authentication-source revalidation,
SDK/UI commands and end-to-end simulation remain outstanding. The broader
workforce phases and final-head release gates remain active.


## Provider-independent reconciliation store (unreleased)

`ProviderReconciliationStore` now provides the same evaluated correlation and
finality acceptance protocol for both live drivers and retired provider bindings.
The store has no provider object, action executor or dispatch grant. Trusted host
configuration qualifies verifiers against exact immutable binding digests retained
before retirement. Archived acceptance requires complete original operation seals,
valid signed ownership and the latest registered attempt. It revalidates the
second operation read and current attempt set rather than trusting an earlier
history projection. Current live drivers additionally require their original
binding and settings to match.

Removing an installed qualification prevents new acceptance. A changed verifier
can authenticate new receipts for its binding, but cannot overwrite a staged
receipt with another proof or revision. Retaining the original verifier allows
freshly authorized acceptance of that staged receipt. Accepted exact-proof replay
uses the pinned evidence and original audit without requiring the old signing key;
current operator authority is still required. Independent history reads need no
verifier installation.

Contracts cover releasing every provider reference before archived acceptance,
plaintext and encrypted records, accepted replay after verifier/key rotation,
wrong binding and owner, rejection of an unaccepted old revision, a lost staging
acknowledgment followed by verifier replacement and restoration, and refusal of
unsealed legacy records. Existing closure, authority expiry, known-result
precedence, acknowledgment-loss and replay contracts now use this shared store.

Server host qualification and lifecycle configuration, independently permissioned
HTTP proof ingress with original authentication-source revalidation, SDK/UI
commands and end-to-end simulation remain outstanding. This library change does
not qualify an external source merely because its signature is valid. Broader
workforce phases and final-head review, CI, merge and publication remain active.


## Trusted server reconciliation boundary (unreleased)

Server embeddings can now install bounded, immutable verifier qualifications for
exact binding digests in declared scopes. Installation consumes the runtime before
serving requests, rejects empty, malformed, duplicate and undeclared installations,
and does not mutate authoritative state. There is no request-controlled verifier
selection and no automatically qualified CLI HMAC source.

`can_reconcile` is independent of history, permit issuance and intervention. It
is omitted when false, defaults to denied across all five SDKs and requires a
nonempty, unique, scope-local `reconciliation_resources` declaration. History-only
scopes retain their read-only meaning and reject this write grant. Signed full
principal ownership and complete resource bounds are checked before correlation
or acceptance. Missing host qualification fails closed.

A generic asynchronous `ReconciliationAuthorityGuard` runs before staging and on
each coordinator settlement CAS attempt, including replay and retries after a
version-only conflict. The server guard revalidates the original private source,
deployment policy, execution authority stamp and current management lifetime.
Source and execution authority are separate records; these checks do not create a
multi-record transaction. Immediate atomic closure/revocation fences remain the
execution coordinator's generation-checked CAS. A stronger source-to-scope cutover
protocol remains a required integration concern rather than an inferred guarantee.

HTTP transport, CLI source qualification/lifecycle configuration, SDK/UI command
surfaces and production-boundary finality/source-race simulations remain next.
The full workforce phases and final-head release gates remain active.


## Reconciliation HTTP and typed SDK transport (unreleased)

The protected production router now exposes independently permissioned correlation
and acceptance endpoints under each retained execution/attempt. Scope comes from
explicit namespace/tenant query parameters; signed ownership comes from retained
state and the manager's full-principal allowlist. Requests cannot select an actor
or verifier. The checked role inventory assigns both routes OperationsManage;
`can_reconcile`, exact resource bounds and qualified host installation remain
independent handler requirements. Sensitive responses disable caching.

Acceptance takes one opaque base64 proof, bounded to 64 KiB after decoding, and
returns a typed provider receipt. Malformed, oversized and unknown-field evidence
cannot create a staged proof or settle an attempt. Exact accepted replay retains
original attribution, spent budget and single concurrency release. Source
offboarding during a controlled ownership-read barrier refuses correlation and
acceptance without exposing correlation or writing reconciliation state. These
contracts use real in-process production authentication/router handlers and a
properly sealed abandoned registration; they do not invoke a provider or claim a
live-network simulation.

Rust, Python (sync/async), TypeScript, Go and Java now expose typed correlation and
acceptance helpers. Generated finite-operation catalogs and the role inventory
cover both new routes. Shared wire fixtures exercise original correlation, exact
opaque request bytes and completed/no-effect nested outcomes. Client commands do
not automatically retry acceptance.

CLI source qualification and lifecycle installation, stronger atomic
source-to-execution cutover, UI acceptance commands, external finality-source
qualification and a live end-to-end simulation remain outstanding, along with
broader workforce phases and final-head review/CI/merge/publication gates.


### Standard server finality-source configuration (unreleased)

The server now resolves bounded `reconciliation_sources` declarations and dedicated
hex-encoded environment keys before authority publication. Each declaration names
a scope, original immutable binding digest, operator-reviewed source contract and
verifier revision. Resolution uses no state access and installation retains the
same configured StateStore. Duplicate bindings, conflicting source revisions or
key references, authority-key aliases (including hex case changes), missing keys,
undeclared scopes and read-only scopes are refused. Empty configuration retains
default-denied reconciliation. Qualified HTTP test fixtures now install their
verifiers through this configuration path.

This supplies startup configuration and restart-based key replacement. It does
not establish external source qualification by itself, provide a durable external
finality journal, dynamically revoke trust roots across replicas or atomically
fence authentication-source changes against execution commits. Archived write-only
scope mode, UI acceptance controls, autonomous cross-participant delegation and
registry-driven A2A remain part of the full city objective. Current changes are
local, unmerged and unpublished.


### Retained finality management without live providers (unreleased)

The explicit `reconciliation_only` deployment mode connects to an existing scope,
requires retained reviewed effect bounds and installs an empty provider catalog.
It uses observation-only credential projection, yielding no executable effects.
Evidence managers can hold independent history and reconciliation grants; live
routes, chains, deployment permits, bootstrap, permit issuance, intervention and
workforce mutation are rejected. The policy fingerprint distinguishes this mode
from read-only `history_only`; changing modes requires a new shared authentication
revision. `history_only` retains its original strict read-only semantics.

The transition contract retains a real sealed unresolved attempt, replaces the
live runtime with an empty-registry runtime on the same StateStore, republishes
private authentication, accepts an independently signed no-effect proof and
replays without changing the original acceptance. It also checks old-proof
refusal, disabled executable credentials, permit and intervention refusal,
concurrency release and absence of provider result records. This is an in-process
production-router contract, not a live external finality-source qualification.

Cross-replica trust-root cutover, atomic authentication-source fencing, UI finality
acceptance, a qualified external journal, autonomous registry-driven A2A and
cross-principal delegation remain required for the full city objective. The
current feature batch is still local, unmerged and unpublished.


## Published descendants and finality management

PR #428 merged on October 6, 2026 as `565a22a20ed19a22569089d959ac95be66488a72`.
The reviewed head passed 25 checks with three intentional skips. Provider and
qualified-handoff contracts ran successfully on Redis, PostgreSQL and DynamoDB.
The documentation deployment succeeded and all seven changed public articles
matched the reviewed strict build. Earlier unreleased sections above are retained
as implementation history; the scope of PR #428 is now published.

## Phase 4: approved registry candidates (in progress)

`ApprovedPeerRegistry` reads the existing individual agent/card records through
StateStore under operator-approved card/skill/endpoint/actor/effect bindings.
`discover_delegation_eligibility` checks independently credentialed source and
recipient contexts against one coordinator snapshot. Candidate discovery is
read-only, bounded and advisory. Changes to cards require renewed qualification;
backend errors and stale authority abort rather than return partial candidates.

Memory contracts and independent Redis clients exercise authority refusal,
registry redirection, resolver identity substitution, stale liveness and controlled
read-barrier expiry. This slice supplies host integration, not an HTTP route or
an A2A invocation. Cross-principal child admission, shared sponsorship, runtime
binding, transport qualification and actual peer invocation remain required for
the autonomous mesh phase. All later workforce/federation phases remain active.


## Approved discovery release and cross-principal admission (in progress)

PR #430 merged reviewed head `482c8d5e9f0a0e36fac4f656d93f8b574fb14050` as
`d290a9f126ac021d549043a17557500f1aefc7f3` on October 6, 2026. All 28 final-head
checks completed: 25 passed and three intentionally skipped. The integration
job's first attempt had an intermittent existing scenario replay divergence;
the exact local feature-enabled CLI suite, nine preserved local replays and one
unchanged-head integration retry passed. The original failure remains retained
and its cause is unproven. Documentation deployment 37503322846 succeeded for the
merge commit; the public governance article returned HTTP 200 and matched the
reviewed strict-build article hash.

The next branch implements explicit service delegation grants, initial signed
root grant selection, independently authenticated cross-principal child input
acceptance and shared sponsor ancestry. Source ingress authority is separate
from recipient direct effects. Nested service intent cannot expand the accepted
root's footprint; budget allocation pins recipient context identity. Grant
publication/terminal retirement and effect starts share the configured StateStore
coordinator boundary. This work remains unmerged pending complete verification,
registry service-plan integration and adversarial review.

Public API/SDK/UI surfaces, real agent-specific runtime and A2A transport,
intervention acknowledgments, production/federation qualification and the other
remaining city/workforce gates are still required. This is platform authority and
accounting work, not scenario-specific code, and it does not complete the city
vision.

### PR #431 service discovery integration (in progress)

The committed delegation foundation is `c3c36317` on PR #431. The next change
integrates approved service plans into registry discovery: source ingress and
initially accepted grant intent are checked before private recipient resolution,
while recipient private operations are checked independently after registry
reads. The binding digest pins complete intent and direct-operation selection.
Contracts cover fresh recipient acceptance and shared sponsorship, no caller
permission borrowing, grant retirement during resolution, delayed expiry,
offboarding, unaccepted grants, and narrowed current intent. This remains an
unmerged draft pending final checks and adversarial review; durable A2A runtime
and transport work remains in the implementation plan.


### Service delegation published; durable runtime adapter in progress

PR #431 merged on October 6, 2026 as
`e12def7624596ef75c9d9efd6dbad40f92063c23`. Its reviewed head completed all
28 checks: 25 passed and three intentionally skipped. Documentation deployment
37521925732 succeeded; the public governance article returned HTTP 200 and its
normalized content matched the reviewed strict build. Earlier draft entries
above record implementation history.

The next platform slice connects accepted recipient contexts to the existing
governed provider executor and task engine through `AgentProviderRuntime`.
Twelve focused contracts cover actual provider invocations, shared sponsorship,
restart, lost acknowledgments, concurrent resumes, ambiguous outcomes, forged
terminal projections, revocation, and acceptance while a sender holds the sole
concurrent slot. Admission does not reserve execution capacity; actual starts
still enforce the shared limit atomically. This slice remains under verification
and does not expose an HTTP/A2A runtime or new SDK wire APIs. Qualified network
handoff, intervention acknowledgments, other runtime families, and the remaining
city/workforce phases remain required.


#### Runtime adversarial review corrections

PR #432 adds the provider runtime adapter. Review identified copied acceptance
keys, substituted task projection identities, unverified terminal artifacts,
and stale reaping after removal of projection metadata. Recovery now binds the
requested ID and validates projection identity at every governed CAS retry;
terminal artifacts are repaired from qualified execution evidence. The reaper
checks the durable acceptance journal as well as the projection marker.
Fifteen ordinary contracts and one independent-client Redis contract exercise
these boundaries, including lost acceptance acknowledgment and both known and
uncertain outcomes. Redis uses an isolated UUID prefix and the contract is wired
into CI. This verifies this adapter on memory and Redis; it does not establish
network A2A or all-backend qualification.

### Durable runtime released; authenticated service admission underway

PR #432 merged on October 6, 2026 as
`91674147d449b332f754e3f2db89705cbf475822`. All 28 reviewed-head checks
completed (25 successful, three intentionally skipped). Documentation deployment
37532453477 succeeded for that merge; the public governance article matched the
reviewed strict build. This supersedes the earlier in-progress release status.

Draft PR #433 implements configured individual-agent service admission. Deployment
publication first authenticates every configured recipient against its own
principal, scoped credential policy, and qualified operation. Missing, wrong,
or unqualified recipient credentials prevent listener startup. A source's
`agent.<id>/invoke` grant does not authorize the recipient's private provider.
The individual REST endpoint durably accepts and replays the same message with
no provider effect start; actual starts remain governed executor work.

Review found that tenant-level A2A endpoints could otherwise access governed
service projections. The legacy adapter and push-config storage now check the
durable acceptance identity. Isolation also covers damaged display metadata,
substituted display IDs, and missing projections. Real-server contracts exercise
legacy get, cancel, continuation, event subscription, and callback configuration.
Generated route catalogs include the new operation in all five SDKs; native
service lifecycle helpers and UI still depend on the completed wire contract.

This is admission progress, not the Phase 4 completion gate. Required work remains:
requester-isolated observation/control, a durable driver and restart recovery,
registry revision fencing, qualified outbound transport, cancellation ambiguity,
and a real peer lifecycle. The complete city/workforce objective remains active.


### Requester observation and server driver checkpoint

The PR #433 branch adds authenticated requester observation and a configurable
server driver over the existing durable acceptance journal. Observation checks
source principal, credential identity, authentication method, and, for agent
callers, the exact source context returned in the admission response header.
GET requests cannot start provider work. Known completion receipts can restore a
missing task projection and artifacts atomically without another provider call.

The driver schedules eligible acceptances through the existing governed runtime,
with bounded concurrency and fair cursors. Completed, in-flight, and uncertain
receipts are retained rather than automatically resent. The state backend scan
API still collects whole scopes, so this does not claim bounded storage scanning
or production scale qualification. Cancellation, typed wire errors, native SDK
header retention, UI, registry revision fencing, and the qualified outbound peer
lifecycle remain before release. PR #433 remains a draft.


### Typed service failures and actual response-loss recovery

The PR #433 branch now preserves typed failure categories from authentication,
scope evaluation, context admission, and the durable runtime through the HTTP
boundary. Storage failure no longer claims missing work, and invalid messages no
longer look like accepted-input conflicts. Requester ownership failures still
conceal foreign work. Error bodies contain stable public codes, and service task
responses prohibit caching.

Three independently connected Redis contracts passed locally: queued work and
known artifacts survive server restart; a fully received HTTP operation whose
response is lost remains uncertain across restart without resend or capacity
release; and an unreadable task projection returns 503 and recovers after repair
without a provider start. These contracts are now included in CI. This proves
these inbound adapter behaviors on Redis, rather than outbound A2A lifecycle or
all-backend qualification. The full city/workforce objective remains active;
requester cancellation, native SDK/header integration, UI, registry revision
fencing, and qualified outbound peer lifecycle remain required.


### Native agent-service receipt lifecycle integration

All five SDKs now have dedicated service acceptance and observation helpers;
Python covers sync and async clients. Receipts retain response provenance in
host state separately from mutable task data. Observation uses the original
route and task identity with per-request headers. Missing provenance cannot be
filled from model metadata, and the service helpers do not retry or follow
redirects. Default Rust and Python transports also refuse automatic redirects;
custom transports must retain the documented constraints.

The agent detail UI now accepts governed service work and observes retained jobs
using the current browser identity. An explicit retry preserves its exact message
ID and content. A new request requires an explicit user action. Context is kept
out of the rendered task data. Configured CORS origins can read receipt/version
headers; the real server contract checks that a reference without the original
requester credential remains denied.

This completes acceptance/observation integration, not cancellation or complete
peer lifecycle. Registry revision fencing, qualified outbound A2A, unsupported
and response-lost cancellation, host tools, production backend qualification and
broader workforce/aggregate funding requirements remain active.


### Original-requester stop boundary

An accepted service requester can now restrict future starts for its recipient
execution subtree through the configured StateStore coordinator. The original
private credential and signed per-job source context bind this control to its
accepted task; another job or recipient cannot substitute authority. Concurrent
and repeated stops share one durable control event. Recovery scheduling skips
stopped jobs, while observation continues to preserve provider evidence.

Stop is distinct from a provider abort acknowledgement. A task with an uncertain
external effect stays working and keeps its source and recipient reservations;
a delivered operation may still complete after the restriction. This checkpoint
adds the safe restriction boundary. Provider abort/reconciliation, native stop
SDK/UI helpers, all-backend qualification, registry fencing, qualified outbound
A2A, and the full workforce objective remain required. PR #433 remains draft.


### Receipt-aware stop SDK and browser controls

Every native SDK now stops the original job using its retained host receipt,
without deriving authority from mutable task data or sharing job headers.
Helpers require the exact original task identity and a true restriction
acknowledgement; failed HTTP responses, redirects, false flags and foreign task
identities cannot return a successful stop receipt. Python covers sync and async.

The browser's per-job **Stop future starts** action retains the same target and
context on an explicit retry. It acknowledges only the durable future-start
restriction and keeps actual provider status visible. A later completed result
preserves the local stopped indication rather than replacing it with Cancelled.
Desktop and mobile contracts cover this path and keep source contexts out of the
rendered page. Receipt retention remains local to the open view.

Provider abort/reconciliation, response-lost peer cancellation, registry revision
fencing, qualified outbound A2A, all-backend/scale qualification, and the complete
city/workforce requirements remain active. This completes native future-start
stop integration, not the complete peer lifecycle or Phase 4 release gate.


### Registry authority foundation (integration in progress)

The coordinator now retains independently approved agent revisions and exact
skill binding digests through the configured StateStore. Publication requires a
trusted host approval and the evaluated authority stamp; ordinary registry
metadata cannot publish qualification. Retirement and delegated effect starts
share the coordinator CAS. A retired or replaced qualified binding blocks new
starts, while known completion and uncertain accounting remain observable.
Previously published digests cannot be reused to revive old accepted work.

This advances the persisted authority protocol to 11. Existing protocol 10
scopes require the explicit reviewed upgrade, preserving their incarnation,
funded child budget links, opaque execution contexts and known or uncertain
receipts. Startup does not migrate authority automatically.

This is a kernel checkpoint. Mandatory qualification of configured mesh
services, fencing registry mutation routes before updating their projections,
and revision-aware service enrollment remain required integration work.
Abstract trusted delegation can still operate without a registry record; this
checkpoint does not establish registry fencing for every deployed service.
Phase 4 and the full governed-city objective remain open.


### Mandatory configured-service registry enrollment (integration in progress)

Configured agent services publish their independently qualified registry record
before deployment grants. Their explicit `registry_revision` defaults to 1 and
is bound into the complete approved service digest through an enclosing route
resource. Requalification requires a new epoch and newly reviewed credentials,
permits and grant revisions wherever their approved footprint changes. Replaying
a retired or replaced epoch refuses startup rather than reopening it.

Admission requires the exact current qualification before allocating source
work. The retirement contract uses a real server and an independent Redis
coordinator: new requests are denied without new roots, accepted history remains
readable, and a restart under the retired declaration refuses to listen.

Registry mutation-route fencing, retained historical bindings after replacement,
qualified outbound transport, provider abort/reconciliation and the remaining
city/workforce phase gates remain required. This completes configured-service
enrollment, not the full registry or peer lifecycle.


### Registry metadata mutation control effects (integration in progress)

The coordinator now stages a digest-pinned agent or card mutation, including
its expected metadata version, before delivering the separate projection write.
Staging retires an existing qualification; an intent for an agent that is not
yet qualified also blocks new qualification. Competing pending intents for the
same agent are rejected. The same authority CAS gates delegated starts and
qualification publication.

Delivery requires current independently bounded operator authority and creates
one durable control-effect receipt. Creation uses StateStore check-and-set,
replacement uses compare-and-swap, and removal uses compare-and-delete. A known
version conflict certifies no effect without overwriting newer metadata. A
successful acknowledgement and matching projection complete the fence. Known
receipts recover interrupted completion without another write. In-flight or
uncertain delivery retains the fence and capacity; matching metadata alone,
including an absent deleted row, cannot certify finality. Secondary delivery
acknowledgements cannot clear mutation intents. Control authority can remove a
closed agent's metadata without reopening agent execution.

Focused contracts cover paused writes and independent concurrent workers,
qualification exclusion, original operator/resource bounds, changed inputs,
registration and completion acknowledgement loss, uncertain replacement and
deletion, stale metadata versions, forged completion and explicit legacy
cutover. An independently connected live Redis race runs under an isolated UUID
prefix and is included in CI.

Authority protocol 12 has an explicit reviewed cutover from protocols 10 and 11.
The real delegated runtime contract preserves funded source/recipient budget
links, opaque context references, registry qualifications and known or uncertain
receipts under both source versions, with one provider invocation and no resend.

This is the mutation protocol prerequisite. Registry HTTP mutation handlers,
operator agent-management bounds, receipt-aware SDK/UI recovery and qualified
reconciliation for ambiguous metadata writes remain required integrations.
At this checkpoint, retained service bindings across replacement, outbound A2A,
provider abort and the complete city/workforce phases remained open.


### Governed registry operator and backend completion

The registry mutation prerequisite is now integrated through the public HTTP
boundary with exact manager agent bounds. Governed scopes fence the six legacy
agent/card writers. Approved discovery reads the actual card, verifies the pinned
digest and current qualification, and treats the separate presence hint as
advisory. Agent and card projections keep separate backend versions and share a
64 KiB discovery/write limit; older larger records remain inspectable and
removable.

Rust, Python, TypeScript, Go and Java expose typed inspect/mutate helpers with
correlated receipts, redirect refusal and no automatic retry. The Governance UI
adds reviewed update/removal, validates responses, journals the exact request
before send, restores it after reload, prevents duplicate activation and requires
reinspection before a new intent. Desktop and mobile contracts cover redirect,
response-loss, false-completion and exact-replay paths.

A shared registry lifecycle contract now passes on memory and independently
connected Redis, PostgreSQL and DynamoDB stores. It exercises create, a paused
concurrent replacement, restart, replay without another projection write,
delete/recreate, restart again and requalification only after known completion. The
production backend variants use isolated storage and run in CI.

This closes the governed registry mutation/operator/backend building block.
Provider abort/reconciliation and qualified outbound A2A remain required for the
full peer lifecycle and city objective.


### Retained service bindings across replacement

Execution scopes can now retain up to 128 exact prior agent-service bindings in
addition to their active services. Each host declaration reconstructs the old
card, registry epoch, agent principal, endpoint, route and provider operation and
must match its pinned 64-character binding digest. Preparation refuses forged
digests, duplicate epochs, nonhistorical revisions and provider substitutions.

Retained bindings install runtimes for accepted-work recovery only. They are
excluded from new admission bounds, permits, registry qualification and grant
publication. Their ingress effects remain in the publisher ceiling so a newer
authentication configuration can withdraw the former credential projection.
Replacement uses a new registry epoch, source permit revision, authentication
authority revision and delegation-grant ID; immutable grant IDs cannot be
retargeted.

Task observation, future-start stop, the recovery driver and exact message replay
route by the binding digest sealed in the durable acceptance. Read-only signed
root/child admission inspection correlates a response-lost replay without
restoring contexts or budgets. Current authentication, original credential ID
and method, source lineage, task identity, binding and input digest are all
verified before the original receipt is returned. A changed payload conflicts,
and a fresh message uses only the active binding. Exact replays and task controls
continue after all active declarations are removed; fresh sends are refused.

A real server replacement contract on Redis accepts work under revision 1,
restarts with revision 2 plus the retained binding, observes, stops and exactly
replays original tasks, executes new work once under the new digest, removes the
active service while preserving exact replay, and repeats old observations after
another restart. A superseded queued task cannot borrow the new authority; when
current checks refuse its first provider start, it stays retained without a
provider call.

This closes historical service binding retention. Governed parent-context
handoff, qualified outbound A2A transport, and provider abort/reconciliation
remain required for the complete peer lifecycle.


### Durable provider-abort delivery foundation

The governed executor now exposes a provider-independent abort adapter contract
for work already fenced by the coordinator. It records the exact reconciliation
attempt and adapter revision in the configured StateStore before making one
external call. Unsupported adapters report restriction only. Any crash, timeout,
error, or lost response remains uncertain and is not resent automatically. The
host-controlled execution timeout also bounds adapter delivery.

An adapter cannot settle work by claiming success. It must return proof accepted
by the exact binding's existing finality verifier. Acteon persists that proof
before reconciliation, replays only the idempotent settlement CAS after restart,
and retains the proof digest in the abort receipt. Focused contracts prove abort
is refused before restriction, qualified no-effect finality wins over a late
provider return, unsupported capability stays distinct, and a crash during the
adapter call produces one invocation and an uncertain receipt after restart.
The retained-proof recovery contract also proves that a verifier unavailable at
delivery time can finish settlement after restart without resending the abort.

Agent-service stop now exposes this distinction as optional `provider_abort`
state. Rust, Python sync/async, TypeScript, Go, Java, and the browser validate and
render restriction-only, uncertain, and reconciled states while preserving the
task's actual provider evidence.

This closes the generic durable abort-delivery and client lifecycle building
block. Concrete adapters must still qualify external attempt mapping and
finality against every supported production backend. Qualified outbound A2A
remains required for the complete peer lifecycle.

### Governed parent-context handoff

The individual-agent admission surface now accepts a paired opaque execution
context and explicit permit references. It recovers sealed parent authority from
the configured state backend and rechecks authenticated principal binding,
permit revisions, registry state, delegation grants, closures, and budgets before
creating a child. It rejects duplicate or malformed contexts, either header on
its own, and empty permit sets.

Rust, Python sync/async, TypeScript, Go, and Java expose typed parent invocation
helpers backed by one cross-language fixture. Context stays in host state and is
never inferred from model messages or task metadata. This closes the public
authority-handoff contract needed by outbound peer invocation. Qualified durable
transport, delivery recovery, and remote finality remain the next platform gap.

### Durable qualified peer-transport foundation

The Rust host now has a backend-neutral durable peer-send journal over the
configured `StateStore`. It rechecks the approved live registry binding and
current source delegation authority, persists the complete send intent before
network delivery, uses a CAS claim for one sender, and retains the exact remote
task/source mapping only after validating it. Repeated or concurrent submission
observes the same stable record; the same message ID with different content
conflicts.

Timeouts, adapter errors, malformed successes, crash-visible registered or
delivering records, and settlement loss remain uncertain. They are never
reported as rejection and are not resent by a normal retry. Explicit redelivery
is available only for an exact host adapter that declares a reviewed idempotent
submission contract, and it rechecks registry and authority before its CAS claim.
At-most-once peers cannot use that path.

Focused contracts cover concurrency, changed input, corrupt retained mappings,
grant retirement, registry suspension, malformed acceptance, ambiguity, and
qualified idempotent recovery. See
[Durable qualified peer transport](durable-peer-transport.md).

This closes the generic outbound intent, claim, ambiguity, and remote-acceptance
mapping foundation. The first concrete adapter now uses the guarded outbound
client, exact host credential, disabled redirects, bounded response reads, and
native parent-context headers. It accepts only a typed task and source mapping;
redirects, 5xx, malformed responses, and transport failures remain uncertain.
The remote lifecycle bridge, a real two-server restart scenario, and
durable-backend qualification are still required before the autonomous mesh is
complete.

### Installed governed peer handoff

Configured agent services now declare exact `onward_agents`. Preparation walks
that bounded graph, seals the transitive provider intent and immediate peer
ingress effects, and rejects an edge without the target's grant and a matching
source permit. Accepted service children retain only those exact onward grant
references. This closes the authority gap where provider-only intent could not
authorize the target's full `agent.invoke` effect.

The server installs an approved registry and one durable guarded transport for
each configured source/target edge. It resolves the source credential from host
configuration, pins the target binding and replay capability into the adapter
revision, and shares the configured state backend, coordinator, clock and
encryption boundary. `AgentPeerInvocation` is a trusted non-deserializable host
input: the host injects source identity and opaque context while model-selected
data is limited to the installed target, skill and message. Submit, observe and
qualified idempotent replay recover and verify the current source service context
before reaching the journal.

The protected peer-send route resolves that trusted invocation from an accepted
source task and the calling agent's current private credential. The request body
contains only the message; source context, permits, endpoint, binding and
credentials cannot be deserialized from model output. Rust, Python sync/async,
TypeScript, Go and Java expose typed one-shot helpers and validate the versioned
accepted, rejected or uncertain receipt without automatic retry.

Accepted sends can now be refreshed by stable submission ID. The runtime
repeats source authentication, permit, grant, binding and registry checks,
derives the remote task URL from the exact qualified REST endpoint, performs a
single guarded read and compare-and-swap journals only valid forward task
progress. Remote failure preserves the last accepted snapshot. All five clients
expose the refresh operation without accepting authority fields.

Focused preparation and strict Clippy checks cover call-graph digest changes,
missing target grants/permits and the compiled runtime integration. The remaining
mesh work is event-cursor projection, input/auth responses, cancellation, a
two-server lost-response/restart scenario and production-backend qualification.

### Governed peer lifecycle release and native cancellation bridge

PR #433 merged as `238594fd59c4086a59312a72f41dcd3c975758ae` on October 8,
2026. The release includes authenticated individual-agent services, durable
driver recovery, requester-isolated observation and stop, retained historical
bindings, provider-abort evidence, parent-context handoff, safe peer discovery,
and guarded durable peer send, refresh, and cancellation tools. The public A2A
documentation deployment completed successfully after the merge.

The next lifecycle increment connects durable peer cancellation to the target
Acteon service's native `/stop` route. A successful stop with a nonterminal task
is retained as `restricted`: the target has durably fenced future provider
starts, while an already-running external effect may still be active. This state
is distinct from ambiguous `uncertain`, definitive refusal, and terminal
`reconciled`. Repeated explicit cancellation observes the exact task and may
promote a restricted record to reconciled finality without delivering a second
remote stop. Rust, Python, TypeScript, Go, and Java validate the same strict wire
contract.

The same increment adds a real two-server HTTPS contract over the configured
Redis `StateStore`. Two independently launched replicas use one deterministic
deployment policy, discover the reviewed peer, create a governed child on the
remote server, persist a restriction through the native stop route, restart the
source server, and recover the identical cancellation receipt without starting
either provider. The contract also proves peer adapters use the deployment's
outbound TLS trust configuration and that services with qualified
`onward_agents` install successfully at runtime.

The Phase 4 completion gate remains open. A two-server response-lost fault
injection contract and production-backend qualification beyond Redis still
remain. Event cursor projection and structured input/auth challenge responses
remain later mesh capabilities rather than prerequisites for truthful
cancellation.

### Post-commit peer cancellation response loss

The real two-server HTTPS contract now routes the qualified peer through a TLS
fault boundary. The target receives its authenticated native stop, durably
fences future starts, and returns `future_starts_blocked: true`. Only after the
proxy has read that committed response does it terminate the response body, so
the source must persist `uncertain` rather than invent a restriction
acknowledgment.

The contract then restarts the source server over the same Redis `StateStore`
and repeats the explicit cancel operation. Recovery retains the same stable
cancellation ID, sends no second stop, and performs exactly one read-only task
observation. Because the target task is still nonterminal and ordinary task
observation cannot prove the lost restriction acknowledgment, the durable
result truthfully remains `uncertain`. The target commit, one stop delivery,
one observation, zero provider starts, and source restart are all asserted in
the same executable boundary test.

The response-lost/restart requirement of the Phase 4 gate is now covered. The
remaining prerequisite is production-state-backend qualification beyond Redis.
Event cursor projection and structured input/auth challenge responses remain
later mesh capabilities.

### PostgreSQL governed-peer lifecycle qualification

The complete two-server HTTPS peer lifecycle now runs through the generic
`StateStore` contract on PostgreSQL as well as Redis. Two independently launched
servers share an isolated table prefix, publish the reviewed cards, discover the
approved peer, create the governed child, deliver the target's native stop,
restart the source process, and recover the exact durable restricted receipt
without another stop or provider start. The PostgreSQL fixture uses an
independent client for registry projection and removes its four isolated state
tables after the contract.

CI builds this contract with `--no-default-features --features postgres`, which
proves the lifecycle does not compile or pass by falling back to Redis. Redis
continues to run the normal lifecycle and the post-commit response-loss variant.

This closes the production-backend prerequisite and the Phase 4 local governed
A2A mesh completion gate. Event cursor projection and structured input/auth
challenge responses remain planned mesh extensions; federation and the later
city phases remain open.
