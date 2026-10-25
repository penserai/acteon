# Federation trust kernel

**Status:** governance substrate, deployment bindings, authenticated trust
management, inert inbound admission, authenticated agent dispatch/replay and
durable provider execution are implemented.

Acteon's local agent mesh uses server-held authority in one administrative
domain. Federation cannot extend that design by serializing a local context or
by trusting a remote agent card. A foreign domain may authenticate an assertion;
the receiving domain still decides which actor, recipient, service binding,
effects, limits and delegation depth it will import.

The governance crate establishes that boundary. The server exposes bounded
trust management, inert inbound admission, and a separate authenticated
dispatch endpoint. Admission retains authority without starting work. Dispatch
binds one signed invocation to one exact prepared message and local service.

## Contract

A local operator publishes a versioned `FederationTrust` through an independent
non-deserializable issuance ceiling. One trust record binds:

- a terminal trust ID, foreign domain, local audience and Ed25519 key ID;
- the exact public verification key, validity interval and minimum foreign
  revocation epoch;
- maximum clock skew and maximum revocation staleness;
- reviewed actor, recipient, agent binding, skill, ingress effect, executable
  effects, root limits and delegation-depth ceilings.

The private signing key never enters the coordinator. A foreign
`FederationEnvelope` is schema closed and signs all authority-bearing fields.
Its audience and key ID must match the current local trust. Its expiry must fit
inside the configured revocation-freshness window, and its proposed effects,
limits and depth may only narrow one exact reviewed shape.

This kernel admits direct imported effects. It retains a depth ceiling for the
runtime integration, but exposes no cross-domain child-derivation operation yet.
Generic local attempt and descendant paths cannot spend or extend a federated
root.

Acceptance verifies the signature and local policy, then atomically records the
exact signed evidence and creates a bounded local root budget in the configured
`StateStore`. Reusing an envelope ID with different signed bytes conflicts.
Retained signed evidence is reverified during coordinator reconstruction, so an
edited imported effect, recipient, limit or binding makes the authority record
unreadable instead of becoming permission.

Every imported effect uses `register_federated_attempt` with a
`SignedFederationInvocation`. The foreign signer binds the exact imported
envelope digest, a replay nonce, the complete typed A2A message, and a short
validity interval. The request carries the same message separately for ordinary
A2A handling; Acteon requires semantic equality before persistence. The signed
peer never has to reproduce receiver-private provider names or action types.
The trusted local adapter independently prepares that message under the exact
service binding and seals its provider-input digest into execution evidence. Current
trust, invocation signature, revocation, target status, expiry, exact effect
attenuation and root accounting are checked inside the same coordinator CAS
that registers each local delivery attempt. Therefore:

- if trust revocation commits first, the new effect is denied without spending;
- if the effect commits first, it is an honest in-flight attempt and the later
  revocation blocks every subsequent effect;
- replay of the already registered attempt remains observation and does not
  cause another external effect;
- uncertainty retains the same root concurrency, as it does for local work.

Trust revocation is terminal for a trust ID. Key rotation or a renewed
relationship uses a new revision before revocation, or a new trust ID after
revocation. Revisions cannot retarget the foreign domain or local audience.
Existing imports stop authorizing new effects when their accepted trust revision
is no longer current.

## Provider execution

`GovernedProviderExecutor::execute_federated` is the host-side execution
boundary for an exact signed invocation. It derives deterministic local context
and execution IDs from the signed bytes, persists the complete immutable
operation before delivery, and seals that operation digest into every start
record. The adapter independently hashes the prepared `Action`; a signed digest
cannot authorize a different provider payload.

The existing provider lifecycle then applies without a federation-specific
side channel: qualified provider bindings, timeouts, known-rejection retries,
one unit per attempt, uncertainty, settlement evidence, reconciliation, abort
and historical inspection all use the configured `StateStore`. Retry ordinals
produce distinct deterministic federation attempt IDs while retaining the same
signed input. An exact completed replay reads retained evidence without checking
current trust or calling the provider again. Prepared work and retries re-enter
the coordinator, where current trust and budget are checked before delivery.

This adapter receives an already authenticated local target from its host. It
does not authenticate a network peer. The server route authenticates the
configured peer on every dispatch or recovery request and rechecks that
credential immediately before a possible start. The coordinator independently
checks signed trust and budget in its start CAS. A background worker never
resumes federated work from stored labels.

## Freshness and partitions

The issuer signs its authority observation time and monotonic revocation epoch.
The receiver accepts it only for its locally configured bounded interval. This
defines the maximum exposure to a foreign revocation; it is not an assertion of
instant global revocation.

After the freshness or envelope deadline, Acteon fails closed. A partition does
not extend the window, reuse cached discovery as authority, or convert missing
remote evidence into success. Already registered effects retain their truthful
known or uncertain status and can be observed or reconciled under the existing
effect lifecycle.

Local closures, target-principal revocation, root cancellation and resource
limits remain independent restrictions. A valid foreign signature cannot bypass
them.

## Protocol and storage

Coordinator protocol 13 adds retained federation trust and import records.
Protocol 12 scopes require an explicit reviewed cutover. Startup never inserts
empty federation state into an older authority record. The cutover preserves the
incarnation, prior history, budgets, starts, registry qualifications and
accounting.

The generic contract is exercised in memory and by independently connected
Redis, PostgreSQL and DynamoDB clients. The backend contract publishes trust on
one replica, imports and funds an envelope on another, starts an exact effect on
the first, revokes trust on the second, and verifies a later start is denied.

## Server integration

Each deployment scope may now configure one local audience, exact trust
revisions and a set of foreign-domain-to-authentication-principal mappings.
Preparation rejects cross-scope resources, mismatched audience/domain pairs,
duplicate peers and unknown trust IDs. An independent manager grant names the
only trust IDs that caller may publish or terminally revoke. Public request
fields can select a configured declaration but cannot expand it.

`POST /a2a/federation/envelopes:import` requires current
private authentication as the configured peer. The runtime verifies the signed
envelope and binds it to the exact prepared local service recipient, skill,
agent resource, binding digest, ingress effect and direct effects before calling
the coordinator. Its typed receipt always reports `execution_started: false`.
The coordinator may fund the imported root, but the server creates no task,
accepted execution context, effect attempt or external call.

`POST /a2a/federation/agents/{agent}/v1/message:send`
accepts a `SignedFederationInvocation` and its exact A2A message. For new work,
the peer credential, retained import, agent ID, current service binding, signed
message equality, trust, closures and budget must all pass before provider delivery.
The durable acceptance stores the signed invocation, and only the authenticated
federation resume path can run it. Generic recovery skips these acceptances.
Concurrent or repeated delivery of the same signed input converges on the same
task and provider evidence. After revocation, an exact completed replay remains
observable to the same configured peer without another provider call; new work
and unfinished sends fail current authority checks.

Rust, Python, TypeScript, Go and Java expose the same typed publish, revoke,
import and federated-agent invocation operations. Wire tests pin the exact path,
body and task identity. Runtime tests prove unsigned recovery cannot resume
federated work, a completed replay does not call the provider again, and
revocation blocks new execution while preserving exact completed observation.

## Remaining integration gates

Peer authentication and coordinator effect registration live in separate state
scopes. The route narrows their race by checking the peer immediately before
dispatch, while the coordinator atomically rechecks signed execution authority
with effect registration. A future credential projection that shares the
coordinator transaction could close that last cross-scope interval.

Further federation gates include outbound delivery, credential exchange,
capability negotiation, remote card and key rotation, response-loss recovery,
operator-visible expiry and reconciliation, load/chaos evidence, and conformance
against named external implementations. Until those pass, Acteon does not claim
complete cross-domain A2A interoperability.
