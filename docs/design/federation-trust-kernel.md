# Federation trust kernel

**Status:** governance substrate, deployment bindings, authenticated trust
management and inert inbound admission implemented; execution handoff remains
gated.

Acteon's local agent mesh uses server-held authority in one administrative
domain. Federation cannot extend that design by serializing a local context or
by trusting a remote agent card. A foreign domain may authenticate an assertion;
the receiving domain still decides which actor, recipient, service binding,
effects, limits and delegation depth it will import.

The governance crate establishes that boundary. The server now exposes an
authenticated inbound admission endpoint and bounded trust-management API. The
endpoint retains imported authority but deliberately starts no task or effect,
so this milestone does not claim complete interoperability with another
implementation.

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

Every imported effect uses `register_federated_attempt`. Current trust,
revocation, target status, expiry, exact effect attenuation and root accounting
are checked inside the same coordinator CAS that registers the effect. Therefore:

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

`POST /a2a/{namespace}/{tenant}/federation/envelopes:import` requires current
private authentication as the configured peer. The runtime verifies the signed
envelope and binds it to the exact prepared local service recipient, skill,
agent resource, binding digest, ingress effect and direct effects before calling
the coordinator. Its typed receipt always reports `execution_started: false`.
The coordinator may fund the imported root, but the server creates no task,
accepted execution context, effect attempt or external call.

Rust, Python, TypeScript, Go and Java expose the same typed publish, revoke and
import operations. A shared fixture pins the wire shape, while the server test
proves operator credentials cannot impersonate a peer, a configured peer can
import, import starts no execution, and terminal revocation blocks a later
envelope.

## Remaining integration gates

The next slice must derive the recipient's local execution context from the
imported record and recheck current peer identity, trust, target status,
closures, expiry and effect bounds at the same durable transition that dispatches
work. It must never accept those authority fields from a dispatch request.

Later federation gates include outbound delivery, credential exchange,
capability negotiation, remote card and key rotation, response-loss recovery,
operator-visible expiry and reconciliation, load/chaos evidence, and conformance
against named external implementations. Until those pass, the cross-domain
surface is an admission boundary rather than a complete federated A2A mesh.
