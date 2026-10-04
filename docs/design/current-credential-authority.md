# Current credential authority at effect starts

**Status:** internal library implementation on the working branch; local checks
passed, review and CI pending.

## Problem and decision

A stable principal can authenticate through several credentials with different
grants. Preserving the principal and permit is insufficient if recovery combines
those credentials or ignores a grant/role change made after admission. Local auth
file reloads also cannot establish a shared revocation boundary across replicas.

Publish a versioned, exact `CredentialAuthority` in the same coordinator as
permits, restrictions, root counters and effect starts. Its `ExecutionPermit`
ceiling supplies subject, complete operation/resource tuples, validity and root
limits. Additional fields identify the authentication method and whether that
credential may execute. Credential IDs contain no tokens or secrets. The ceiling
is specific to one credential, never the union of an actor's credentials.

A privileged host resolves authenticated grants against qualified actual provider
and endpoint identities. It publishes only reviewed complete effects within its
issuance ceiling. Registry cards, payloads, models and SDK labels cannot publish
or select trusted credential authority. The library does not authenticate its
publisher or provide an auth-file-to-effect resolver.

## Admission and recovery

`TrustedContextStore::capture_credentialed_root` accepts the independently
verified credential ID/method, selected current revision, original permit
references and root limits. It verifies the exact current credential revision,
actor, execution eligibility, effects, time and budgets before sealing provenance
and creating the root budget. Context format 2 retains the credential reference
inside the signed payload. Changing the reference requires a new reviewed root;
redelivery or recovery cannot replace it with a more privileged credential.

Every `register_permitted_attempt` for such a context checks both the accepted
historical ceiling and its current record on each CAS retry, alongside permits
and root accounting. Current grants can narrow a job; later broadening cannot
expand its original ceiling. Current call/concurrency limits apply to the shared
root counters. Exact complete tuples prevent a cross-product of unrelated grants.

`GovernedProviderExecutor::require_credential_authority()` refuses actor-only
contexts before retaining or executing work. Credentialed contexts always check
their bound authority, even without this required-profile flag. Actor-only
library compatibility APIs remain explicitly available for trusted integrations;
they are not a credential-enforced server scope or a strict-profile fallback.

| Change | Effect on admitted work |
|---|---|
| Grant removes an operation/resource | Next affected fresh attempt denied |
| Execution role disabled | Next fresh attempt under that credential denied |
| Current call/concurrency cap reduced | Next registration checks the narrowed cap against root usage |
| Credential revoked | Future effects under it denied; ID cannot be republished |
| Another credential for the actor remains valid | Its independently admitted jobs remain eligible |
| Principal revoked | Future starts across that principal's credentials denied |
| Credential grants broadened | Existing jobs retain their accepted ceiling |
| Provider attempt already registered | Remains in flight; mutation does not erase or undo it |

Subject and authentication method cannot be retargeted under an existing ID.
Credential secret rotation can preserve that ID when the host verifies the same
identity; rotation does not implicitly reassign work. Transport token expiry and
job authority lifetime are separate host policies. Admission must bound the job
by whatever credential lifetime the host promises; the library checks the
published ceiling's expiry and the sealed job deadline at effect starts.

## Publication and auth integration obligations

Publication uses expected revisions and an evaluated authority stamp. The record,
generation and pending control event commit in one CAS. Replays observe the
original event; a changed publisher, definition or reason conflicts. Current
credential records are reconstructed from retained control history on load.
Tampering, skipped revisions and retargeting fail closed. Generic `change` cannot
bypass the bounded publication entrypoint. Terminal revocation uses the same CAS.

Server startup and reload must publish resolved ceilings before acknowledging
them as effective for a governed scope. Each replica must use authoritative
records rather than a cached watcher view at effect starts. A failed or lost
publication acknowledgment requires observation/reconciliation, not rollback by
recreating state. Several scope coordinators are not one transaction: partial
publication must park or reconcile affected scopes and report its actual status.
A single global success response cannot hide those partial results.

Hosts still authenticate each API request and authorize continuation, observation
and management independently. An actor ownership check alone is not a complete
public read/resume authorization policy. Public enforce profiles must require
credentialed capture and refuse unsupported effect paths; they must not expose
legacy capture as an alternate route.

## Compatibility and remaining work

Coordinator format 6 adds credentials; context format 2 adds signed credential
references. Earlier formats are refused. Existing actor-only records require
reviewed migration or parking; no authority may be inferred from whichever
credential currently names that actor. Older readers/writers cannot participate
in a rolling deployment that drops these fields.

This slice supplies the shared authority primitive and connects it to the durable
provider boundary. Server startup/reload publication, credential resolution,
authenticated APIs, all SDKs/UI, complete effect coverage, workforce mandates and
delegated lineage remain open. Finite history retention, emergency admission stop
and backend failover qualification remain independent storage prerequisites.

## Executable contracts

`crates/governance/tests/credentials.rs` covers credential separation, narrowing,
disablement, original/current limits, immutable identity/method, terminal
revocation, bounded publication, history corruption and signed-context recovery
after key rotation, plus publication acknowledgment loss/replay. Controlled races exercise both revocation/start orders and
competition for the last unit. An explicit independent-client Redis contract
repeats those three orderings using a unique prefix and deletes only its own keys.

Executor contracts invoke the actual provider method, revoke its credential
during persisted backoff, and prove no retry occurs. Required credential mode
refuses an actor-only context with no retained operation and no invocation.

```sh
cargo test -p acteon-governance --test credentials
ACTEON_GOVERNANCE_REDIS_URL=redis://127.0.0.1:6379 \
  cargo test -p acteon-governance --test credentials \
  independent_redis_credential_authority_passes_the_contract -- --ignored
cargo test -p acteon-executor --test governed_provider
```
