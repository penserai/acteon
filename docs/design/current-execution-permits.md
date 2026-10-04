# Current execution permits and atomic admission

**Status:** internal root-profile policy merged in PR #414. The durable provider
adapter is the next library slice; server provisioning and public management
surfaces remain open.

## Decision

Store exact execution permits in the same scoped coordinator as effect attempts,
root accounting and restrictions. Publishing or revoking a permit advances the
authority generation and persists its pending control event in one CAS. A later
attempt cannot use a separately cached permit object after that change.

An `ExecutionPermit` binds a stable ID/revision, actual principal, complete exact
operation/resource tuples, validity and integer per-root ceilings. Effects are
matched as whole tuples. Missing permission denies; no wildcard expansion,
membership union or cross-product of unrelated resource/operation lists exists.
Selected permits intersect: each must cover the complete requested effect.

Publication requires the expected current revision, evaluated authority stamp
and an independently evaluated trusted issuance ceiling. The ceiling bounds
subjects, effects, validity and units/concurrency. It is not deserializable
authority. Management authentication and current issuer grant evaluation remain
host responsibilities; an administrative role or JSON field cannot supply them.
The unbounded generic change method rejects permit publication. Subject retargeting
is refused, and revoked permit IDs cannot be reactivated. New issuance uses a
new ID so an old accepted lineage does not silently regain authority.

## Accepted provenance and current permission

Canonical `permits-v1` revision tags bind selected IDs and accepted revisions in
the existing HMAC-sealed context's accepted-ceiling field. Reference order is
irrelevant; empty, duplicate or invalid references fail. This adds a recognized
root profile without treating arbitrary context metadata as a permit.

`capture_permitted_root` verifies current selected revisions, actual subject,
accepted complete effects, validity and requested root limits before sealing the
context and creating its immutable allocation. The handle/execution ID and root
ID remain stable across retries. These are two recoverable writes: interruption
can leave an inert context with no root allocation, never permission to send.
Lost allocation acknowledgments observe the same immutable budget. A changed
authority generation or incompatible accepted admission parks creation pending
explicit review instead of synthesizing broader work.

`register_permitted_attempt` accepts a verified root context, its selected
references, complete effect, actual semantic input digest and trusted host clock.
The root ID derives from verified execution identity. Input digest must equal the
admitted root binding. Replays bind the operation, canonical resource set, input,
selected lineage and units; observing an existing attempt never authorizes a send.

Fresh attempts check both the accepted original revisions and current records.
Root allocations cannot exceed original permit limits; current narrowing further
bounds remaining units/concurrency, effects and validity. Later broadening cannot
expand the accepted job. A revoked selected permit denies the next fresh effect.
Current actor/root revocation and any complete-resource closure also apply.

Permit-limit evaluation repeats after every CAS conflict, alongside current root
counters. A generation-only check is insufficient because budget writes do not
change authority generation. Two contenders cannot spend a narrowed permit's last
unit even if the root's original allocation is larger. The trusted clock is
sampled again after storage/CAS contention; a stale caller timestamp is not the
fresh permit profile's time source. Clock accuracy and storage latency remain
qualified host/backend assumptions, not a remote socket cancellation guarantee.

## History, compatibility and retained obligations

Coordinator format 4 adds current permit records. Load reconstructs them from
retained publication/revocation history and rejects mismatches, skipped revisions,
retargeting, duplicate authority generations and post-revocation publication.
Permits/history count against bounded record and byte capacity; issuance preserves
control headroom. Control event acknowledgment does not remove effective policy.

Formats 1–3 are refused explicitly. Standalone adopters stop admission and review
retained in-flight/uncertain obligations before any migration. No deletion,
recreation, automatic conversion or old-reader rollback is supported. Signed
context format remains unchanged; recovery still needs the compatible coordinator.
Safe archival must preserve accepted permit history and consumed-unit summaries.
Finite control history and emergency admission-stop qualification remain open.

## Scope and next integrations

This is direct actual-actor root authority. It does not implement team mandates,
represented-party lineage, child attenuation, aggregate funding, schema/definition
constraints or federated permits. Integer ceilings are per root, not a global
monetary spending limit. Host adapters compute and validate semantic digests from
real work; caller-supplied labels cannot establish that binding.

No gateway/provider path uses this profile yet. The next slice must provision the
trusted production gate at executor boundaries, retain result/reconciliation
evidence and refuse unsupported effect classes in an enforced scope. Existing
unmetered and ungated compatibility helpers cannot be enforce-mode fallbacks.
Principal/grant/mandate changes still need authoritative publication. Public
administration APIs, all five SDKs, UI and public book documentation ship with
that supported surface rather than advertising an incomplete enforcement switch.

## Evidence

Contracts cover bounded issuance and its bypass refusal, current narrowing,
original budget/input binding, complete-tuple intersection, canonical replay,
terminal revocation, expiry, publication response loss, corrupt retained policy,
interrupted root creation and refreshed deadlines after CAS contention. Controlled
barriers prove revocation-before-start, start-before-revocation and narrowed-limit
competition. The same three race orderings run through independent Redis clients;
CI explicitly executes this contract.

The subsequent [durable provider adapter](durable-provider-governance.md) uses
coordinator format 5, adding immutable attempt evidence references. Format 4
above describes this permit slice historically; old readers/writers cannot be
mixed with the newer adapter without a reviewed migration.
