# Atomic credential configuration snapshots

**Status:** internal coordination primitive on the working branch; required local
checks pass, PR review and CI pending.

## Problem

A server can reload several credential grants at once. Independent per-credential
updates expose partial policy, and a replica with an older file can restore grants
if it simply reads the latest revision and republishes its old definitions. An
acknowledged auth reload needs a shared configuration identity and version.

## Decision

`CredentialConfiguration` represents the complete credential projection for one
trusted source within one namespace/tenant coordinator. It contains a source ID,
monotonic revision, an opaque configuration fingerprint and exact credential
ceilings. Each credential ceiling revision equals the source revision. A newer
revision may skip unapplied versions; `expected_revision` still names the actual
current source head, and publication requires an independently evaluated stamp.

The host must compute the fingerprint from all security-relevant configuration,
including credential rotation, principal bindings, roles and grants. Equal resolved
execution effects alone cannot detect a change from Operator to Executor. Use a
keyed fingerprint when inputs contain secrets; do not publish raw secrets or an
unkeyed digest that enables guessing them. This primitive accepts that trusted
fingerprint; the production auth parser/fingerprinter remains integration work.

The coordinator canonicalizes credential, effect and resource ordering and hashes
the full snapshot. Reordering equivalent definitions preserves the binding. A
same-version change to either the definitions or fingerprint conflicts.

## Atomic publication

`publish_credential_configuration` checks the issuer's bounded ceiling and writes
all of these through one CAS:

- current credential definitions and revisions;
- terminal retirement of omitted credentials;
- the source head and retained ownership of credential IDs;
- the authority generation and pending control event.

An empty snapshot retires the source's credentials. Removal still requires
management authority over its previous subjects/effects; empty input cannot
bypass the publication ceiling. Disabled credentials can have an empty execution
ceiling. Enabled credentials and ordinary execution permits require nonempty,
complete approved effects. A disabled ceiling does not authorize a root.

Source ownership is retained after retirement. Another source cannot adopt those
IDs, an individual publication cannot overwrite source-owned credentials, and a
retired credential cannot be reactivated. Re-enrollment uses a new credential
identity; it need not change the person's stable principal. Subject and method
cannot be retargeted while an ID remains active. Individual terminal revocation
remains available as a restrictive overlay; a later snapshot cannot undo it.

Source rename is creation of another authority source, not automatic retirement
of the old one. A reviewed source migration must retire the old source explicitly
and account for its jobs and credential identities. Multiple sources are separate
trusted authorities, never an implicit union for one execution.

## Replicas, acknowledgments and freshness

| Condition | Outcome |
|---|---|
| Newer version and current expected revision/stamp | Publish the whole scope projection atomically |
| Current version with the same canonical digest | Observe its original event without another generation change |
| Current version with different content/fingerprint | Conflict; no mutation |
| Older version, even if previously published | Stale; do not reinstall old tables |
| Any invalid or unauthorized entry | Entire publication refused |
| Response lost before/after commit | Read authoritative state and reconcile the same snapshot |
| Old coordinator incarnation | Reject the reference and require reviewed recovery |

`CredentialConfigurationReference` binds source, revision, digest and incarnation.
`verify_credential_configuration` compares a reference derived from the host's
own trusted configuration and returns the stamp from that same snapshot. It
establishes freshness, not authentication, credential eligibility or permission
for an operation. Hosts must independently check actual credential/principal
revocation and request permissions. Caller-supplied references cannot establish
which configuration the host actually authenticated against.

Current-stamp mutation and effect registration still supply the serialization
boundary. A freshness check alone is not permanent authority: changes can occur
after it. Effects continue through current permit/credential registration, while
management mutations must use the stamp from their complete authority evaluation.
Previously registered attempts remain in flight; a snapshot does not undo an
external effect, discard uncertainty or refund root units.

## Required server integration

The next host slice must:

1. Validate/decrypt the auth file and resolve stable credential identities,
   including explicit re-enrollment identities and ambiguous duplicate names.
2. Produce a keyed security fingerprint and a monotonic configuration revision.
3. Resolve complete actual effect tuples from qualified provider definitions;
   preserve each credential's grants independently.
4. Publish each configured scope before exposing new tables. Several scopes are
   not one transaction; partial publication needs explicit status/reconciliation.
5. Bind authentication results to the exact accepted snapshot/credential revision.
   Check authoritative freshness and eligibility on governed requests; an older
   replica cannot silently substitute its local tables or refresh a caller into
   broader authority.
6. Preserve that binding through root capture and deferred work, then check current
   authority at every supported effect. Public enforce scopes require credentialed
   capture and refuse unsupported paths.

An auth watcher acknowledgment, table swap or optional callback alone does not
complete these gates. Public APIs, SDKs/UI, workforce mandates and delegation
remain open. This library primitive does not enable a server enforcement mode.

## Storage and compatibility

Coordinator format 7 adds configuration heads and full snapshot control events;
signed context format 2 is unchanged. Earlier coordinator formats are refused.
Migration/parking and compatible readers are required; deleting state or replacing
its incarnation cannot settle old effects or safely reclaim reservations.

Each snapshot is bounded to 128 credentials and the issuer ceiling to the existing
16 subject references. Record/byte capacity can impose tighter limits. Retired IDs
and prior events remain retained. Publication preserves control headroom and fails
without mutation at capacity; unlimited reloads, safe archival, emergency admission
stop and failover qualification remain separate storage work.

## Executable evidence

`crates/governance/tests/configurations.rs` covers atomic narrowing/retirement,
stale and conflicting versions, canonical order, skipped unapplied versions,
ownership, overwrite/retirement bypasses, all-or-nothing invalid entries,
acknowledgment loss, history corruption, incarnation binding, competing replicas
and empty disabled ceilings. Existing root capture and permit registration
exercise historical credential lookup through snapshot events.

Controlled races cover snapshot-before-start and start-before-snapshot. Both
credential rows change together; subsequent affected attempts are denied while
an already registered attempt remains honestly in flight. An explicit independent
Redis-client test repeats both orderings with a unique prefix and deletes only its
fixture records. CI runs it separately so ignored tests do not count as evidence.

```sh
cargo test -p acteon-governance --test configurations
ACTEON_GOVERNANCE_REDIS_URL=redis://127.0.0.1:6379 \
  cargo test -p acteon-governance --test configurations \
  independent_redis_configuration_snapshots_pass_the_contract -- --ignored
```
