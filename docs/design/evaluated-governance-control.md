# Evaluated governance control writes

## Decision

A public intervention adapter must use `AuthorityCoordinator::change_evaluated`
after authenticating the actual credential and evaluating its current management
policy. The trusted host supplies an independent `ControlChangeCeiling` and the
exact scope `AuthorityStamp` observed during that evaluation. The coordinator
checks the stamp, current actor revocation, absolute validity, and the complete
requested control target inside the same load/CAS loop that persists the
restriction and its pending control event.

The configured `StateStore` owns this state. Memory, PostgreSQL and Redis run
the same behavioral contracts; Redis is not an architectural requirement.
No persisted schema change or migration is introduced by this entrypoint.

## Bounds and authentication

The ceiling names one actual actor, a bounded set of exact typed principals,
and a bounded set of exact scoped resources. It cannot be deserialized from a
public request. Closing/reopening requires an included resource; subject
revocation requires an included principal ID. Permit and credential revocation
require their current typed subject and **every** resource in their current
policy to fit the ceiling. Expected policy revision remains mandatory.
Only permanently reserved execution scopes accept this entrypoint; unclaimed
and authentication-control scopes refuse it. Intervention authority does not
confer publication authority. An expired
permit can still be revoked by a currently authorized operator.

The host must verify its private authentication evidence against the current
scope configuration and derive management bounds from independent deployment
policy. A dispatch grant, execution permit, caller-provided actor, or possession
of a snapshot is insufficient. This internal entrypoint does not authenticate
anyone and is not yet a public management API. Current endpoint role checks
alone are not an adequate adapter for it.

## Races and replay

A changed scope generation or incarnation fails with `StaleAuthority`, including
on an idempotent replay. The host must perform fresh authentication and
management evaluation; it must not attach a newly read stamp to an old decision.
Every CAS retry resamples the trusted host clock. An outbox acknowledgment can
change the storage version without changing the authority generation, so a
retry still has to check time and actor status.

A lost acknowledgment may leave the requested restriction and pending event
committed. Fresh, still-authorized evaluation followed by identical replay
returns the original event without another write. A replay after reopening
observes the earlier closure event and does not close the resource again.
Changed actor, reason or requested change under the same ID conflicts.
Revoked actors and expired management authority cannot retrieve a successful
replay through this entrypoint.

The old `change` method remains a privileged host/bootstrap primitive for
existing trusted integration code. Public adapters must use the evaluated
entrypoint. Neither method implies remote cancellation, compensation, drain,
overlapping named closure records, or that already admitted effects stopped.
Time is sampled after each storage read; expiry during the backend write itself
is subject to the same trusted-clock observation boundary as effect admission.

## Verification and next integration

Memory plus independently connected PostgreSQL and Redis clients exercise lost
acknowledgments, current-authority replay, reopen/replay behavior, actor
revocation winning before CAS, and expiry during a CAS conflict with unchanged
authority generation. Policy tests exercise complete resource and subject
bounds, missing policies, wrong revisions, foreign scopes, duplicate bounds
and validity.

Next integrate private authenticated management projection, authenticated
permit publication/revocation and closure endpoints, typed SDKs, UI controls,
and a live server scenario proving that an operator closure prevents a new
provider invocation. Keep all management operations unavailable to executors.
Named overlapping closures and intervention reconciliation remain distinct
Phase 3 requirements.
