# acteon-governance

Coordination substrate for the [governed-city design](../../docs/design/governed-agent-city.md).

This crate serializes effect-start registrations and authority restrictions through
one bounded, non-expiring per-tenant StateStore CAS record. It is not yet connected
to gateway execution and does not implement execution permits, principal
administration, delegated authority, or a public closure API.

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
The current exact-string restrictions are internal prototype inputs; canonical
resource references and complete authority evaluation remain required integration
work.

Independent adapters cannot safely share an attempt ID unless subject, resource,
and semantic request digest agree. The adapter must compute the digest and must
not accept a caller's assertion of trusted authority. This crate is a trusted
library boundary, not a credential-authenticating service.

## Bounds

Admission preserves reserved record and byte headroom for control changes, but
history remains finite. Reaching capacity refuses new work or the requested
mutation; callers must not report an unpersisted restriction as active. Unlimited
operator control, safe archival/compaction, emergency admission stop, shared-root
budgets, and scalable indexes are still open design work. Unresolved attempts must
never be evicted to release capacity.

Single-key CAS correctness and storage durability/failover assumptions are backend
requirements. Tests against a running Redis prove the exercised contract, not
correctness under every Redis deployment or failover configuration.

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
