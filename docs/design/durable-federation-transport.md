# Durable outbound federation transport

Status: implemented transport foundation; export authority and runtime installation remain open.

## Problem

Cross-domain agent execution is a two-request protocol. The source first sends
an audience-bound signed authority envelope. Only after the destination returns
an import receipt may the source send the exact signed invocation. A process can
crash or lose a response after either destination commit. Treating that as an
ordinary retry can import changed authority, execute a message twice, or report
success without evidence.

The transport also crosses two trust boundaries. Model output must not select a
credential or arbitrary destination, and a successful HTTP status is not proof
that the destination accepted the signed intent the source retained.

## Decision

`DurableFederationTransport` is a backend-neutral journal over the configured
`StateStore`. Its trusted host input contains:

- the local parent execution reference used for lineage and scope;
- one operator-qualified remote origin and agent ID;
- the exact signed envelope and signed invocation;
- the typed A2A message duplicated inside the signed invocation.

The delivery ID is deterministic from the parent execution ID and the digest of
the exact signed invocation. Before networking, Acteon persists the complete
intent, adapter revision, binding digest, and replay qualification. Optional
payload encryption uses the same host encryptor as other retained execution
state. Credentials remain in the adapter and are never serialized into the
journal.

The adapter owns endpoint policy and protocol binding. The concrete
`ActeonFederationHttpAdapter` uses `GuardedClient`, disables redirects and
proxies, bounds response bodies, supplies one host credential, and constructs
only the two protected Acteon federation routes from the signed remote scope.

## State machine

```text
registered
    │ CAS claim
    ▼
importing ── known refusal ───────────────► rejected(import)
    │
    ├─ lost/invalid/failed response ──────► import_uncertain
    │
    └─ exact receipt ─────────────────────► imported
                                               │ CAS claim
                                               ▼
                                           invoking
                                               ├─ known refusal ─► rejected(invoke)
                                               ├─ ambiguous ─────► invoke_uncertain
                                               └─ exact task ────► accepted
```

An in-progress claim is exposed as uncertain because another worker or a crashed
process may have crossed the network boundary. A normal repeated `submit` only
observes uncertain state. It never resends that stage. An explicit
`replay_idempotent` may reclaim an expired claim or ambiguous stage only when the
installed adapter declares that replaying the exact signed request returns the
original acceptance. A compare-and-swap claim gives one local network owner;
late settlement from an older owner cannot overwrite a newer state.

Import and invocation are checkpointed separately. Once the exact import receipt
is durable, invocation recovery never imports again. The injected Acteon clock
drives claim expiry, signed validity checks, and adapter timeout, which keeps
simulation and production timing in one domain. Terminal evidence remains
observable after the signed request expires, while a new network claim outside
the signed validity interval is refused.

## Evidence validation

The journal validates retained state on every read. It rejects changes to the
parent, origin, agent, adapter revision, binding, signed bytes, message, or
derived delivery ID. Before import it verifies that the envelope identifies the
same agent and source principal. Before invocation it verifies the import
reference and envelope digest and requires the body message to equal the message
inside the signed payload.

An import response is accepted only when its scope, import ID, envelope digest,
trust revision, actors, resource, binding, skill, expiry, and inert
`execution_started: false` assertion match the envelope. An invocation response
is accepted only when it is a valid task in the signed remote scope and its
first history message is the exact requested message. A malformed successful
response remains uncertain.

## Authority boundary

The journal does not issue federation authority. Its caller must recover the
accepted local execution and recheck current source credential, permit,
delegation, closure, budget, destination binding, and export policy before every
initial submission or explicit replay. The upcoming export-authority layer will
construct and sign the bounded envelope from those current records and install
the transport behind a trusted runtime method. Public or model-authored requests
will carry only the intended target/skill/message selectors permitted by that
runtime.

## Verification

Focused contracts cover exact persistence before network delivery, one network
owner under concurrency, import and invocation response loss, no ordinary
resend, explicit qualified recovery, expired-claim recovery, signed expiry,
terminal observation after expiry, message substitution, and retained-state
tampering. The concrete HTTP adapter has a route and credential contract for the
two protected endpoints. A later release gate must run the complete source and
destination servers through committed-response loss and source restart on every
supported production state backend.

## Remaining gates

This foundation intentionally leaves these authority and operational layers
open:

1. export policy and signer-key rotation that derive envelopes from current
   local permits and delegated lineage;
2. deployment configuration for remote origin, audience, credential reference,
   trust/key identifiers, replay capability, and TLS material;
3. a trusted runtime API that rechecks source authority immediately before the
   journal and never accepts credentials or signed authority from model output;
4. observation, expiry, reconciliation, and operator controls for ambiguous
   deliveries;
5. capability/key-card negotiation and credential exchange;
6. two-server restart and response-loss qualification across Redis,
   PostgreSQL, and DynamoDB, followed by external implementation conformance.
