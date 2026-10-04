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

| Phase | State after this slice | Next required evidence |
|---|---|---|
| 0: Inventory/coordinator | HTTP inventory implemented; effect inventory and unintegrated coordinator prototype in progress | Complete effect call-site mapping; multi-replica start/closure race and crash tests |
| 1: Actors/context | Execution-only role implemented; stable principals and durable context remain | Unforgeable context construction and propagation through every deferred path, migration and credential rotation tests |
| 2: Permits/checkpoints | Not implemented | Revocation and target reauthorization at each effect; atomic root reservations |
| 3: Closures/intervention | Existing agent lifecycle only | Generic serialized closures, durable intervention, drain/pause/cancel semantics and acknowledgments |
| 4: Autonomous mesh | Existing registry and submitted A2A tasks only | Real target resolution/invocation, attenuation, lineage, recovery and safe peer retry |
| 5: Production/federation | Not implemented | Verified backend and peer capability matrix, trust/revocation protocol and failure tests |

Next work should map the actual effect paths and prototype the coordinator
contract, then introduce stable principals and trusted execution context. Do
not expose workflow/queue mutation as executor tooling merely by changing the
role table: these need retained authority and operation/resource checks first.

For every slice, record PR/review/merge and publication evidence here, keep
public guides synchronized with shipped behavior, and retain the full remaining
phase gates until their own runtime evidence passes.

## Coordinator substrate slice (in progress)

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
runs the real-Redis contract; CI execution is still pending.

Remaining Phase 0 work includes reservation protocol proof, canonical resource
identity, checked effect coverage, retention/emergency admission-stop design,
backend failover assumptions, and operational bounds. Context propagation and
per-retry enforcement remain Phase 1/2 integration gates. Neither a substrate
unit test nor a Redis test certifies the full city vision.
