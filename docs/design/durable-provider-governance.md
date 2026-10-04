# Durable governed provider execution

**Status:** library implementation merged in PR #415. Server-wide enforcement
and the remaining phase gates are open.

## Decision

`acteon_executor::governed::GovernedProviderExecutor` connects current root permits,
trusted execution contexts, atomic reservations and provider attempt gates. It
retains the operation and provider evidence for recovery. This is a reusable
execution primitive for deterministic operations and agent tools. Server-wide
scope enforcement and workforce mandates remain separate integration work.

A trusted host constructs `BoundProvider` from an immutable provider instance,
endpoint resource/version, action type, definition revision and all additional
protected resources. The binding names the actual selected provider, including
when the original Action names another provider. Dynamic endpoint selection or
internal provider retries require their own qualified mediation before using
this boundary. A configuration revision is a host assertion that must actually
identify that configuration; it cannot detect an unreported endpoint change.

## Durable operation and attempt identity

Before sending, the adapter retains the original Action, context reference,
selected permit revisions, binding and bounded retry settings. The semantic
input digest excludes delivery ID, creation time, trace context and transport
signatures. A redelivery can have a new transport identity while observing the
same operation; it cannot replace the retained payload or selected authority.

Each ordinal derives its UUID from the root execution UUID. Competing workers
therefore register the same attempt, rather than allocating different IDs. Only
`StartRegistration::New` supplies the guard that can invoke the provider.
`Existing` is observation. Losing the registration acknowledgment can leave an
in-flight reservation even if no network call happened; recovery must not infer
that another send is safe.

Before each new attempt, the gate verifies the context for execution and checks
original/current permits and fresh time through the coordinator. Each known-safe
retry spends another root unit. Stored rejection evidence retains its retry due
time across restart. A replacement waits for that time and reevaluates authority;
revocation or expiry during backoff blocks the next invocation.

## Evidence and accounting

The guard saves immutable result evidence before settling the root reservation.
Coordinator format 5 pins the scoped evidence ID and plaintext SHA-256 digest
through the same CAS that changes attempt status and releases active capacity.
Repeated settlement releases capacity once; a changed evidence reference is a
conflict. Recovery validates actor, complete resources, reservation, canonical
permitted-attempt digest, token, binding and saved result before repairing a
missing settlement. It returns the original result without invoking the provider.

| Observed condition | Recovery behavior |
|---|---|
| Provider success and retained evidence | Settle once and return the saved result |
| Reviewed rejection with no external effect | Settle; retry only within the pinned policy and current authority |
| Timeout or unqualified provider error | Retain capacity and report reconciliation required |
| Cancellation before evidence retention | Keep the attempt in flight; no automatic replay |
| Evidence write fails before commit | Keep the attempt unresolved, even if the provider returned success |
| Evidence commit or settlement acknowledgment is lost | Observe authoritative records and repair from saved evidence |
| Evidence or binding changes | Refuse recovery; do not send again |

Generic connection errors do not prove that a remote operation failed. The
default failure contract treats all provider errors as uncertain. A privileged,
versioned `ProviderFailureContract` can classify a specific error as known
rejected only when the provider contract proves no external effect. Timeout
remains uncertain regardless of that classifier. Uncertain evidence is immutable;
qualified manual resolution needs a separate reconciliation protocol, not an
overwrite or retry toggle.

Results remain inspectable by their actual owner after permit revocation and
context expiry. Historical context verification proves provenance, not current
permission to start an effect. Registration still checks expiry and incarnation.
There is no public cross-owner operator inspection endpoint in this slice.

## Storage and compatibility

Operation and evidence records do not expire. Each is bounded to 2 MiB; retry
policies allow at most 32 attempts. Optional `PayloadEncryptor` encrypts retained
Action and result bodies. The digest pins decoded plaintext bytes rather than a
reserialized response whose map ordering could change. Host storage access and
context signing keys are privileged dependencies.

Format 5 refuses earlier coordinator formats. There is no automatic in-place
migration or safe rollback to an older writer. Drain or park work and perform an
explicit reviewed migration before cutover. Never delete unresolved attempts or
bootstrap a replacement incarnation to recover capacity.

Finite coordinator capacity can prevent settlement even after evidence was
saved. Recovery retains the evidence and refuses duplicate sends; safe history
retention, capacity qualification and emergency admission stop are still required
for production deployment. Missing evidence is never interpreted as proof of no
effect. This boundary promises mediated attempt identity, not exactly-once
external business effects or provider-wide socket fencing.

## Integration boundary

The API accepts a verified authenticated actor from its trusted host. It does
not authenticate credentials, evaluate gateway rules, implement approvals or
create a public enforce scope. Templates and attachments are refused in this
initial direct-provider profile. It does not use the legacy Action-only DLQ.

Remaining work includes server provisioning, current grant/principal publication,
workforce representation and mandates, child attenuation, every deferred and
auxiliary effect path, qualified reconciliation, API/SDK/UI surfaces and a real
workforce scenario. Existing generic executor methods remain compatibility paths
outside this explicitly constructed adapter; they cannot be advertised as an
enforced scope.

## Executable evidence

`crates/executor/tests/governed_provider.rs` covers duplicate workers, cancellation,
ambiguous errors, result/settlement write interruptions, registration acknowledgment
loss, input/actor/configuration conflicts, digest pins, encryption, historical
inspection and expiry, known-safe retries and durable backoff with revocation.
A loopback HTTP service records an actual invocation and its unchanged payload;
a reconstructed executor returns its saved result without a second network call.

An explicitly executed Redis contract uses independent connections and independently
constructed coordinators/context stores: a second worker observes the first
worker's registration and then its settled result. It uses a unique Redis prefix
and deletes only its own records. CI runs this contract separately so an ignored
backend test cannot masquerade as executed evidence.

```sh
cargo test -p acteon-executor --test governed_provider
ACTEON_GOVERNANCE_REDIS_URL=redis://127.0.0.1:6379 \
  cargo test -p acteon-executor --test governed_provider \
  independent_redis_workers_observe_one_provider_attempt -- --ignored
```

These tests establish the exercised library contract. They do not establish
server coverage, Redis failover safety or completed workforce governance.

The [current credential authority follow-up](current-credential-authority.md)
extends this historical format-5 slice to coordinator format 6/context format 2.
Credentialed roots are checked automatically at the permit checkpoint. Hosts can
require that profile explicitly; public server enforcement remains open.
