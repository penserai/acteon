# Atomic effect registration and shared root reservations

**Status:** internal coordinator contract on the working branch; runtime permit
evaluation and effect wiring remain open.

## Decision

Use the existing per-scope coordinator CAS to persist an effect's complete exact
resource set and reserve its root units/concurrency in one write. Registering a
provider and endpoint separately, or decrementing a counter before inserting an
attempt, would leave unsafe race and crash states.

`AttemptRequest` supplies a stable attempt ID, actual subject, semantic digest,
evaluated authority stamp, up to 16 distinct exact resources, trusted current
time and an optional root reservation. Resource order is irrelevant; missing,
duplicate, foreign-scope or oversized sets are refused. Any closed resource
blocks the complete attempt without allocating units.

Root allocations have an immutable owner, integer unit limit, concurrency limit
and deadline. Trusted adapters derive these limits from authorized root permits;
the coordinator does not authenticate callers or prove delegated lineage. The
root owner is an additional revocation ancestor, while complete principal,
membership, mandate and permit ancestry remains evaluator responsibility.

## Accounting and replay

New registration spends positive units and consumes one concurrency slot in the
same CAS as its attempt record. Retries and descendants use fresh attempt IDs
and share the root ID. Only one concurrent contender can spend the last units.
Limits and counters are reloaded after CAS contention.

An existing attempt is observation only. Its resource set, subject, digest,
root and unit count must match; it cannot authorize another send or replenish
limits. Root creation uses the same stable root ID and refuses changed owner or
limits. Lost creation/start acknowledgments are recovered by observation.

Uncertain attempts retain concurrency. Known durable settlement releases the
slot once, including after a lost settlement acknowledgment. Spent call units
are never refunded. Registration that fails byte/record capacity or restriction
checks spends nothing. These are conservative authorized-attempt units, not
retrospective billing or a strict monetary cap.

Root concurrency counts registered unsettled effect attempts. It is not a count
of all agents/jobs, and it cannot stop an already registered remote socket. The
adapter must define known settlement for its effect class; a timeout alone is
not settlement. An existing-ID response never grants a second external attempt.

## Linearization and integrity

Authority change and start/reservation share the coordinator CAS. If a resource
closure wins, reevaluation refuses the attempt and no root capacity is consumed.
If registration wins, it is in flight even if its acknowledgment/network send
follows closure. Both orderings are tested with controlled before/after barriers.

Load verifies exact resource sets, valid attempt incarnations/tokens, root
references and counters reconstructed from all retained attempts. Corrupted
usage, missing roots, future authority generations, duplicate persisted resources
and nil authority/token identities fail closed. Roots count toward record/byte
capacity and cannot consume reserved control-plane headroom.

## Compatibility and remaining gates

Coordinator format 3 replaces format 2's single resource with a complete set and
adds root accounting. Formats 1 and 2 are refused, not silently migrated or
deleted/recreated. Standalone adopters must stop admission and explicitly review
old records, preserving in-flight/uncertain obligations. Rollback to older
readers is unsupported for format-3 state. Signed context format is unchanged;
its coordinator dependency still refuses incompatible authority storage.

The legacy one-resource `register_start` delegates to an explicitly unmetered
attempt. A future enforce profile must require its verified root reservation;
it cannot fall back to this compatibility helper. Root IDs/units/time must come
from trusted context and evaluator output, not a model's assertions.

No gateway provider/worker effect uses this accounting yet. Current permission
publication, complete class-specific resource resolution, per-retry checkpoint
wiring and public API/SDK/UI integration remain gates. Aggregate team funding
and allocations across coordinators require separate proof; this is one scoped
root ledger. Retention must preserve consumed-unit summaries before archiving
settled attempts. There is no automatic eviction, unlimited control capacity or
backend failover certification. Emergency admission-stop and production capacity
qualification remain open.

## Evidence

Memory contracts cover shared units, concurrency/uncertainty, complete-resource
closure, root-owner revocation, deadlines, immutable replay, invalid resources,
missing roots, byte-capacity rollback, root/start/settlement response loss,
counter corruption and controlled final-unit/closure races. An independent-client
Redis contract runs the accounting lifecycle and all three controlled race
orderings. CI explicitly executes it; passing sequential fake-store tests alone
does not establish the shared-backend contract.
