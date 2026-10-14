# Federate independently operated agent domains

An agent mesh becomes a trust boundary as soon as one agent can ask a peer in
another deployment to act. Acteon makes that boundary explicit: the source
domain can choose a peer, but only the destination can decide which signed
authority it will import and which local effect that authority may reach.

This guide follows the production qualification scenario used in CI. It starts
two real Acteon servers with separate authentication, authority configuration,
signing keys, transport credentials and state namespaces. The source uses Redis
and the destination uses PostgreSQL in the mixed-backend run. No state database
or internal authority key is shared. The one cross-domain credential is the
explicit peer transport credential: the source holds its value and the
destination authenticates its stored hash to a peer principal.

```mermaid
sequenceDiagram
    participant SA as Source agent
    participant S as Source Acteon / Redis
    participant D as Destination Acteon / PostgreSQL
    participant P as Qualified provider

    SA->>S: Export message under local permit
    S->>S: Persist signed envelope and invocation
    S->>D: Fetch signed capability card
    D-->>S: Audience-bound capabilities
    S->>D: Import attenuated authority
    D->>D: Verify current trust and persist root
    S->>D: Invoke exact governed agent
    D->>P: Execute qualified effect
    P-->>D: Success
    D--xS: Response deliberately lost
    Note over S,D: Both servers restart
    S->>D: Explicit idempotent replay
    D-->>S: Original task
    Note over D: Trust is revoked
    S->>D: Fresh export
    D-->>S: Rejected; provider is not called
```

## What each domain owns

The source deployment owns the originating agent, its local task, a
`federation.export` permit, the outbound profile and the signing key. The
profile fixes the remote origin and audience, destination capability key,
transport authentication, accepted trust revision, exact remote agent binding,
effect ceiling, unit budget and maximum delegation depth.

The destination owns its audience, peer-to-principal authentication binding,
accepted source signing key, import trust, local agent service, recipient
credential and provider permit. Publishing a registry card does not grant any
of these rights. The destination imports only the intersection of the signed
request and its current trust record.

Keep these inputs under separate operational control:

| Source domain | Destination domain |
|---|---|
| Envelope signing secret | Pinned source verification key |
| Peer transport credential | Peer authentication principal |
| Destination capability public key | Capability signing secret |
| Local export permit and budget | Import trust and local provider permit |
| Durable export journal | Imported root, task and effect evidence |

Use [host-owned secret custody](../features/secret-custody.md) for durable A2A
callback credentials. Federation signer and peer credential variables are read
by the deployment while the immutable export epoch is installed; they are not
accepted from an agent message or retained in delivery evidence.

## The qualification scenario

The contract performs the following checks against real HTTP listeners:

1. The destination operator publishes the exact reviewed trust revision.
2. The source creates a governed local task and selects its configured export
   profile.
3. The source fetches and verifies the destination's signed capability card.
4. Acteon persists deterministic signed bytes before crossing the network.
5. The destination imports the attenuated root and calls the qualified provider
   exactly once.
6. A proxy discards the successful invocation response. The source records
   `invoke_uncertain`; an ordinary repeat performs no network call.
7. Both Acteon processes restart. Explicit replay returns the retained remote
   task, settles source accounting and leaves the provider count at one.
8. Direct task observation with insufficient provenance returns 404, avoiding
   cross-principal existence disclosure.
9. The destination revokes the source trust. A new export settles as
   `rejected`, and the provider count remains one.

The first CI run uses isolated Redis prefixes and additionally drives the
public A2A 1.0 surface with the official Python `a2a-sdk` 1.2.2. The second run
uses Redis for the source and PostgreSQL for the destination. Both execute the
same backend-neutral contract.

## Run it

Start Redis and PostgreSQL, then point the test at disposable databases:

```bash
export ACTEON_GOVERNANCE_REDIS_URL=redis://127.0.0.1:6379
export DATABASE_URL=postgres://acteon:acteon@127.0.0.1:5432/acteon_test

cargo test --locked -p acteon-server --features postgres \
  --test federation_two_server \
  independent_domains_recover_across_redis_and_postgres \
  -- --ignored --nocapture
```

The successful contract reports one passing scenario. Its assertions establish
the useful result:

```text
initial export       invoke_uncertain
ordinary repeat      no network call
post-restart replay  accepted, same remote task
provider effects     1
post-revocation      rejected
provider effects     1
```

For the Redis-to-Redis run with the official A2A client, install
`a2a-sdk==1.2.2`, set `ACTEON_A2A_PYTHON` to that environment's Python binary,
and run `two_acteon_servers_negotiate_export_restart_and_deliver_one_effect`.

## Production review

Before enabling an export profile, review the source and destination
declarations together. The audience, trust ID and revision, signing public key,
agent resource, binding digest, skill, ingress effect, direct effects, budget
and depth must describe one identical contract. Acteon rejects mismatches at
startup or import; do not repair them by broadening the destination trust.

Rotate a profile by adding a higher immutable epoch. Retain the older epoch
until every delivery that names it is terminal. When a response may have been
lost, keep the outcome uncertain until exact replay or qualified finality proof
establishes what happened. Revoking trust blocks fresh imports; it does not
rewrite an effect that already completed.

Use the federation delivery evidence API to page sanitized state, inspect the
negotiated capability digest and stage, and reconcile or abandon ambiguous work
under independent management authority. The full route and configuration
reference is in [A2A federation](../features/a2a.md#governed-cross-domain-federation).
