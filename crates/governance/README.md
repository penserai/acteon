# acteon-governance

Coordination substrate for the [governed-city design](../../docs/design/governed-agent-city.md).

This crate serializes effect-start registrations and authority restrictions through
one bounded, non-expiring per-tenant StateStore CAS record. It is not yet connected
to gateway execution. It implements internal exact root-profile permits, but
principal administration, delegated authority and public closure APIs remain open.

## Contract

A trusted adapter evaluates authority against an `AuthorityStamp` and submits that
stamp when registering a stable attempt ID. `New` is the start linearization point.
`Existing` is observation/reconciliation only and never authorizes another send.
A changed generation requires reevaluation for a new attempt. A different
incarnation rejects even an existing-ID lookup from the old context.

A restriction and its pending control event are persisted together. A failed or
lost acknowledgment must be reconciled by reading authoritative state, not by
assuming the mutation failed. Secondary audit/index delivery can acknowledge the
event without removing the restriction.

If restriction wins before registration, the new attempt is refused. If
registration wins, that attempt is already in flight, even when its response or
network send occurs later. Registration cannot fence an arbitrary external socket.
Every subsequent step or retry requires a new evaluated registration.

Settlement requires the recorded attempt token. Uncertain attempts retain active
capacity. Settled attempts cannot return to uncertain or in-flight state. This
crate tracks coordination status, not external business outcome.

## Integration prerequisites

Only trusted deployment bootstrap may initialize missing state. Runtime adapters
use `connect`; disappearance or incompatible formats fail closed. Never delete or
expire the coordinator to reopen a resource. A trusted disaster-recovery bootstrap
creates a new incarnation and requires fresh authority evaluation.

Every eligibility-changing principal, grant, or permit update must eventually
participate in this coordinator protocol. A permit stored independently and
revoked without changing this authority state cannot establish strict revocation.
Resource restrictions use validated exact `ResourceRef` identities and must
match the coordinator namespace and tenant. Complete authority evaluation and
authenticated management remain required integration work. These internal
restriction toggles do not yet represent overlapping public closure lifecycles.

Independent adapters cannot safely share an attempt ID unless subject, resource,
and semantic request digest agree. The adapter must compute the digest and must
not accept a caller's assertion of trusted authority. This crate is a trusted
library boundary, not a credential-authenticating service.

## Bounds

Admission preserves reserved record and byte headroom for control changes, but
history remains finite. Reaching capacity refuses new work or the requested
mutation; callers must not report an unpersisted restriction as active. Unlimited
operator control, safe archival/compaction, emergency admission stop, aggregate
team/funding allocations, and scalable indexes are still open design work. Unresolved attempts must
never be evicted to release capacity.

Single-key CAS correctness and storage durability/failover assumptions are backend
requirements. Tests against a running Redis prove the exercised contract, not
correctness under every Redis deployment or failover configuration.

## Complete effects and atomic root budgets

`register_attempt` accepts up to 16 distinct exact resources, a stable attempt ID,
evaluated authority stamp and optional positive-unit root reservation. All
resource restrictions, actor/root-owner revocation, root deadline, remaining
units and concurrency are checked before one CAS persists both reservation and
attempt. Any affected resource blocks the complete operation.

`create_root_budget` captures an immutable allocation supplied by a trusted
evaluator. Replaying creation cannot increase its limits. Attempts across
descendants share its ID; retries spend new units. Unknown outcomes retain
concurrency, and known settlement releases it once without refunding spent units.
Counters are verified against retained attempt records on load. Roots count
toward bounded capacity while preserving control headroom.

This is not authentication, lineage verification, aggregate team funding or
runtime enforcement. A future enforce profile must derive root IDs/units from
verified context and require reservations. The compatibility `register_start`
helper is explicitly unmetered and cannot be a strict-mode fallback. See the
[accounting ADR](../../docs/design/atomic-effect-reservations.md).

```sh
ACTEON_GOVERNANCE_REDIS_URL=redis://127.0.0.1:6379 \
  cargo test -p acteon-governance --test reservations \
  independent_redis_root_reservations_pass_the_contract -- --ignored
```

## Current execution permits

The `permit` module publishes immutable-ID current revisions under a trusted
issuance ceiling and revokes them through the same authority CAS. Current policy
is reconstructed from retained control history on load. Terminal revocation
cannot be undone by republishing the same ID.

`TrustedContextStore::capture_permitted_root` verifies current subject, complete
effects, selected revisions and bounds before sealing provenance and allocating
its root budget. `register_permitted_attempt` binds the actual input digest and
reevaluates original/current permits and root counters on every CAS retry, with
a refreshed trusted clock. Selected permits intersect; they never assemble a
cross-product of unrelated permissions. Existing registrations remain observation.

These are privileged host APIs, not credential authentication or a public enforce
profile. The executor now has a directly mediated [durable provider adapter](../../docs/design/durable-provider-governance.md)
merged in PR #415. Grants/mandates, represented lineage, server enforce
profiles and API/SDK/UI provisioning remain open. See the [permit ADR](../../docs/design/current-execution-permits.md).

```sh
ACTEON_GOVERNANCE_REDIS_URL=redis://127.0.0.1:6379 \
  cargo test -p acteon-governance --test permits \
  independent_redis_current_permits_pass_the_contract -- --ignored
```

## Trusted durable root context

`context::TrustedContextStore` captures a trusted adapter's authenticated root
identity, original credential provenance, exact accepted operation/resource
tuples, ceiling revision, input digest, deadline and evaluated authority stamp.
It persists a versioned HMAC-SHA256 sealed record and returns an opaque reference.
Recovery on another replica verifies the signature, domain/scope, expected
execution/actor/input binding, deadline and coordinator incarnation before
constructing a non-deserializable `VerifiedExecutionContext`.

This is a privileged library boundary. The deployment supplies strong signing
keys and retains verification keys during rotation. Never expose root capture or
key material to models; do not translate client metadata directly into admission
facts. A handle is not a bearer credential, and its trusted host must check work
ownership independently. A holder of signing keys can issue context records.

Allocate the handle before capture and retain it across retries. Lost write
acknowledgments are reconciled through the same handle. Replays observe the
original facts and reject changed ceilings, deadlines, credentials or input.
Capture uses an independently evaluated authority stamp; stale fresh captures
are refused. Later generation changes do not invalidate provenance, but require
current effect evaluation. A new incarnation refuses old contexts.

Verified provenance is **not effect authorization**. Current principal/permit
evaluation, atomic reservations and start checkpoints remain mandatory future
integration. This slice does not wire contexts into gateway/deferred records or
implement child delegation. Exact tuples cannot be mixed across entries.
Records are bounded to 64 KiB, 128 effect tuples and 16 resources per tuple.
They do not expire automatically: retained-work cleanup and key retirement need
an explicit lifecycle policy before production integration. No public API or SDK
surface is introduced by this internal substrate.

```sh
cargo test -p acteon-governance --test context
ACTEON_GOVERNANCE_REDIS_URL=redis://127.0.0.1:6379 \
  cargo test -p acteon-governance --test context \
  independent_redis_context_capture_and_recovery -- --ignored
```

## Verification

```sh
cargo test -p acteon-governance
ACTEON_GOVERNANCE_REDIS_URL=redis://127.0.0.1:6379 \
  cargo test -p acteon-governance --test coordinator \
  independent_redis_connections_pass_the_same_contract -- --ignored
```

The Redis contract uses a unique prefix and deletes only its own coordinator key.
It exercises independent client connections, both race orderings, response loss,
replay, revocation, and durable pending control events.


## Resource format compatibility

Coordinator format 7 stores complete exact resource sets, root accounting,
immutable result evidence references and
current permits, credential-specific ceilings and configuration heads. Formats 1–6
are refused.
No automatic deletion/recreation or permissive migration is performed. The earlier
substrate is not wired into live gateway effects, but any standalone adopter must
stop admission and explicitly review/migrate retained records before upgrade.
Unknown/uncertain attempts retain their reconciliation obligations. Rolling back
to an older reader is unsupported for format-7 state. Signed context format 2 adds credential
references, but recovery still requires a compatible coordinator.

## Retained result evidence

Coordinator format 5 adds immutable digest-pinned attempt evidence references.
Trusted adapters retain evidence before `settle_with_evidence` atomically pins
its reference and releases capacity. Repeated settlement does not refund units
or release capacity twice. Earlier formats fail closed; migration and rollback
require explicit review. Evidence itself is stored by the qualified adapter.

## Current credential ceilings

The [credential authority primitive](../../docs/design/current-credential-authority.md)
binds a job to one authenticated credential's accepted and current exact ceiling.
Publication, narrowing, disablement and terminal revocation share the effect-start
CAS. `capture_credentialed_root` seals its revision; every permitted attempt checks
it automatically. Required credential mode on the durable provider adapter refuses
actor-only contexts. Server authentication/resolution and reload publication remain
integration work; this is not a public enforce profile.

## Atomic credential configurations

[Configuration snapshots](../../docs/design/credential-configuration-snapshots.md)
publish a complete scope projection with a monotonic source revision and trusted
security fingerprint. Credential changes, omitted-ID retirement, source ownership
and pending control history share one CAS. Equal/current snapshots observe;
older or conflicting snapshots cannot restore grants. Source-owned credentials
cannot be overwritten individually. Freshness references do not establish auth
or effect authority. Server startup/reload and authenticated-request binding are
required host integration, not enabled by this primitive alone.
