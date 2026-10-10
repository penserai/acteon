# Durable outbound federation transport

Status: implemented transport, source authority, runtime installation and authenticated export API; production qualification gates remain open.

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

Registration and delivery are separate trusted operations. A host can retain
the exact intent with `register`, atomically reserve current local permit and
budget authority in its coordinator, and only then call `deliver_registered`.
The convenience `submit` method composes registration and delivery for callers
that already established an equivalent authority boundary. Calling delivery
without the durable intent conflicts before any adapter call.

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

The journal itself does not issue federation authority. The
`GovernedFederationExporter` supplies that source boundary. Its immutable,
non-deserializable profile pins the remote origin, audiences, trust/key
revision, expected public key, target, qualified agent binding, effect ceiling,
budget, depth, lifetime, and one local `federation.export` effect. It constructs
both signatures from a host key and refuses a signer whose public key differs
from the profile.

For a stable export ID, invocation nonce, message, and timestamps, the exporter
reconstructs identical signed bytes. It registers those bytes without network
traffic, then registers a local permitted attempt whose unit reservation equals
the entire delegated remote unit ceiling. The source therefore pays the full
remote allowance up front and cannot promise the same local units to another
domain. A known remote acceptance or rejection settles local concurrency;
ambiguous delivery remains uncertain and retains it.

Authorization and delivery are separately callable. Before a registered import
or invocation stage crosses its still-unused network boundary, the exporter
rechecks the current source credential, permit, representation/delegation
lineage, closures, and local export effect. Definitive denial records a local
rejection without networking and settles concurrency. Explicit idempotent replay
also requires current authority; observation and accounting repair do not.
Public or model-authored requests never supply a credential, endpoint, signer,
effect ceiling, permit, or authority context.

Deployment configuration binds each export profile to one declared source agent,
remote origin and audience, trust/key revision, exact foreign service grant,
credential environment variable, signer environment variable, outbound policy,
timeout and maximum signed lifetime. The source agent's permit must independently
list the profile, and its private credential must independently carry the exact
`federation.<profile>/export` authentication grant. Credential projection maps
only that grant to the profile's local `federation.export` effect. Preparation
adds the effect to the agent's qualified service plan and publisher ceiling;
permit publication adds only the profiles explicitly named by that permit.
Deployment preflight rejects a source credential that does not cover every
direct service-plan effect. Outbound-only domains may configure exports without
installing inbound trusts or peers.

At startup the server resolves secrets, verifies that the private signing key
matches the configured public key, pins the HTTP adapter and TLS qualification
into a binding digest, and installs the exporter over the deployment's selected
`StateStore`, shared clock and optional payload encryptor. Startup fails before
the listener is exposed if any profile, secret, signer, TLS policy or adapter is
unavailable.

Each profile contains a bounded epoch keyring. One highest `active_epoch` is
used for new issuance; every older declared epoch retains its exact signer,
transport credential, trust/key revision and reviewed remote agent card. The
issuance journal stores both a stable common-profile digest and the selected
epoch digest. Adding and activating a higher epoch therefore leaves old signed
intent reproducible, while changes to either its routing boundary or retained
epoch material conflict. Runtime installation builds a separate immutable
exporter and adapter binding for every epoch and requires all referenced secrets
to be available. Configuration rejects duplicate epochs and rollback to a lower
active epoch.

The authenticated agent API accepts only an installed profile name and A2A
message under an already accepted local agent task. Acteon recovers the source
context from that task, matches current private authentication, verifies the
qualified service binding, checks the exact profile export grant, and rechecks
the current export permit. A durable
issuance record allocates the export ID, invocation nonce, issue time, expiry
and attenuated remote budget exactly once using the configured state backend.
Retries and process restarts therefore reproduce the same signed bytes. The API
has separate submit, observation and explicit idempotent-replay operations;
observation performs no network call and remains available for accounting after
authority changes.

An independently authorized reconciliation manager can page through sanitized
delivery evidence across every installed export epoch. The projection exposes
identity, lineage, trust/key epoch, lifecycle stage, claim deadline, signed
expiry and replay qualification. It omits all payloads, endpoints and secrets,
filters by the manager's declared source principals, and revalidates both
management and authentication authority after backend I/O. The five SDKs expose
the same typed page.

The same independent reconciliation grant can abandon an ambiguous or expired
claimed delivery. Abandonment is a terminal local state with immutable operator,
authority-generation, reason, time and idempotency evidence. Invoke-stage
abandonment retains the validated import receipt internally so corrupted stage
lineage fails closed. The journal commits before governed export accounting is
settled; replaying the same disposition repairs a lost settlement acknowledgment.
No management field can supply a peer result or trigger a network request.

## Verification

Focused contracts cover separated registration and authorized delivery, exact
persistence before network delivery, one network owner under concurrency,
import and invocation response loss, no ordinary resend, explicit qualified
recovery, expired-claim recovery, signed expiry, terminal observation after
expiry, message substitution, and retained-state tampering. The concrete HTTP
adapter has a route and credential contract for the two protected endpoints. A
rotation contract proves that adding a higher signer, credential and remote key
card preserves the common issuance binding and the exact old epoch digest while
selecting distinct material for new work. Configuration tests reject active
epoch rollback. A later release gate must run the complete source and
destination servers through
committed-response loss and source restart on every supported production state
backend.

## Remaining gates

The remaining production gates are:

1. qualified peer-finality evidence and backend-native bounded pagination for
   large journals;
2. capability/key-card negotiation and credential exchange;
3. two-server restart and response-loss qualification across Redis,
   PostgreSQL, and DynamoDB, followed by external implementation conformance.
