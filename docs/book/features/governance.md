# Governance management

Operators can issue permits, close resources, reopen them and revoke authority
while humans, agents and software continue operating. These changes use the
same configured state backend and atomic authority boundary as governed
execution. A closure that commits before an effect starts prevents that start.
An effect that started first may finish.

## Declare independent management rights

First enable [execution permits](execution-permits.md) and shared authentication.
Add the operator's stable principal to the execution scope's `subjects`, then
add an explicit manager declaration to that same scope:

```toml
[[execution_authority.scopes.managers]]
principal = { id = "operator", kind = "human" }
subjects = [{ id = "agent/maya", kind = "agent" }]
routes = [{ provider = "incident", action_type = "execute" }]
valid_from_ms = 0
limits = { max_units = 5, max_concurrent = 2, deadline_ms = 4102444800000 }
can_issue_permits = true
can_intervene = true
can_read_history = true
```

Bind that principal to an enrolled credential with `operator` or `admin` role
and a grant for the exact namespace and tenant. Provider/action grants can be
empty when this credential should manage the scope without dispatching:

```toml
[[api_keys]]
name = "operator"
authority_id = "credential/operator"
principal = { id = "operator", kind = "human" }
key_hash = "REPLACE_WITH_SHA256_OF_YOUR_KEY"
role = "operator"

[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = []
actions = []
```

Management is unavailable without an explicit declaration, even to an admin.
An executor cannot manage the scope, even when its principal is named in a
manager declaration. The server verifies the actual private credential binding,
current role, scope grants, policy fingerprint, revocation and validity.

`can_read_history` is a separate, default-denied grant. It exposes retained
provider evidence only for the manager's declared subjects. Readers can use an
empty route list without gaining permit issuance or intervention rights.

History inspection rechecks both the original authentication authority and the
execution-scope authority after reading the evidence. A changed observation
returns a conflict; current disablement or an expired management grant denies the
read. This also applies to absent or unreadable evidence, so a reader that loses
access cannot learn a record's storage status through an early error response.
These are checks against two authoritative records, not a multi-record
transaction. A later intervention cannot recall a response that was already
authorized. Use execution-scope interventions to govern provider starts.


Manager subjects and routes must fit the independently declared scope bounds.
The limits cap newly issued permits; they do not create aggregate team funding.
`can_intervene` grants control over subjects and the complete resource footprints
of those routes. A shared provider or endpoint resource can affect several
routes. Inspect actual resource references before selecting a closure.

Changing management declarations changes the prepared security policy. Increase
`authority_revision` in the shared authentication file when deploying that change.
Manager intervention footprints must fit the bounded control ceiling; startup
rejects oversized resolved footprints before publishing authority.
A replica with old policy or old credential observations refuses management
instead of attaching a fresh authority stamp to an old decision.

## Inspect and apply changes

| Operation | Endpoint |
|---|---|
| Inspect current bounded management view | `GET /v1/governance?namespace=prod&tenant=acme` |
| Read verified provider history | `GET /v1/governance/executions/{execution_id}?namespace=prod&tenant=acme` |
| Issue or revise a qualified permit | `POST /v1/governance/permits` |
| Close/reopen a resource or revoke authority | `POST /v1/governance/changes` |

Inspection returns permitted routes and their exact qualified effect resources,
current permits within the manager's subjects/routes, closures, subject
revocations and management bounds. It does not expose signing keys, authentication
fingerprints, other credentials or unrestricted coordinator state.

For publication, provide `namespace`, `tenant`, `change_id`, `expected_revision`,
`permit` and `reason`. The permit contains `id`, `revision`, typed `subject`,
concrete `routes`, `valid_from_ms` and `limits`. The server resolves the actual
qualified effects; public inputs cannot invent provider bindings. New IDs require
expected revision 0 and revision 1. Revisions advance by exactly one and cannot
change the subject or reactivate a revoked ID.

For intervention, provide the scope, a stable `change_id`, a reason and one typed
`change`:

| Kind | Fields |
|---|---|
| `close_resource`, `reopen_resource` | Exact `resource` from inspection |
| `revoke_subject` | Typed `subject` from management bounds |
| `revoke_permit` | `permit_id`, `expected_revision` |
| `revoke_credential` | Logical `credential_id`, `expected_revision` |

Revoking a permit or credential requires the actual subject and every resource
in its current policy to fit the manager's bounds. Subject and credential
revocation apply in the named execution scope. Credential IDs are terminal:
remove a revoked enrollment from subsequent authentication configurations and
use a new logical ID when granting new authority. Otherwise a configuration
revision attempting to republish that revoked ID is refused.

A successful receipt identifies the actor, reason, change ID and committed
generation. `pending` describes secondary control-event delivery; the restriction
is already authoritative. Preserve the same ID and exact input when recovering
a lost acknowledgment. Replay checks current management authority and observes
the original event without reapplying it. Replaying an earlier closure after
reopening does not close the resource again.

HTTP 400 rejects invalid requests, 401 requires authentication, 403 refuses
current authority, 409 reports an authority/revision or idempotency conflict,
and 503 reports unavailable governance. A 409 requires fresh evaluation; the
SDKs do not automatically retry management mutations.

## SDKs and UI

| SDK | Inspect | Publish | Intervene |
|---|---|---|---|
| Rust | `governance` | `publish_governance_permit` | `intervene_governance` |
| Python, sync/async | `governance` | `publish_governance_permit` | `intervene_governance` |
| TypeScript | `governance` | `publishGovernancePermit` | `interveneGovernance` |
| Go | `Governance` | `PublishGovernancePermit` | `InterveneGovernance` |
| Java | `governance` | `publishGovernancePermit` | `interveneGovernance` |

All methods use typed models and retain HTTP refusal status. The **Governance**
page in the Admin UI inspects a scope, issues new permits, shows route/resource
status and applies reasoned closure, reopening and permit-revocation commands.
Current server checks remain authoritative if the displayed snapshot changes.

Run the [governed-city simulation](../guides/governed-city.md) to see a human
operator issuing authority and stopping new agent operations through real calls.

With an explicit history grant, the Governance page also accepts a provider
execution UUID and displays the authenticated participant, retained receipt, operation integrity, attempt
statuses, original verified results and accepted reconciliation. The panel is
available for history-only scopes without installed providers. Changing the
inspected scope resets the receipt selection, and a denied read hides any cached
receipt. Evaluated reconciliation includes the original accepting operator's
principal, authority generation and acceptance time. Proof-recording time remains
separate; authorized replay preserves the original acceptance record. Older
adapter settlements can lack operator attribution, which the UI displays explicitly.
History controls do not invoke, retry or settle providers.

The Rust host integration also supplies `ProviderReconciliationStore` for qualified
finality acceptance after a provider binding is retired. It uses the configured
state backend, retained context keys and complete original operation seals. The
host installs verifiers for exact immutable binding digests; reconciliation does
not retain a provider connection or reuse current execution settings. Staged
receipts require their original verifier revision, while accepted exact-proof
replay preserves the original audit under current operator authority. The independent `can_reconcile` management grant defaults to denied and requires
an explicit `reconciliation_resources` ceiling for the complete attempt footprint.
Trusted server embeddings install verifier qualifications before serving requests;
private authentication is rechecked before staging and on every settlement CAS
attempt, including replay. The HTTP and typed SDK interfaces below use these guards.
The standard server can install operator-qualified receipt sources at startup;
without an exact binding declaration, finality management fails closed.

### Configure an external finality source

Add a source declaration to the server TOML after independently reviewing the
source's finality contract. Obtain the immutable binding digest from the trusted
provider catalog's `BoundProvider::reconciliation_binding_digest()` and retain it
with that deployment. A current endpoint name cannot substitute for a retired
binding's digest.

```toml
[[reconciliation_sources]]
namespace = "prod"
tenant = "acme"
binding_digest = "REPLACE_WITH_ORIGINAL_64_CHARACTER_LOWERCASE_HEX_DIGEST"
source_id = "incident-journal"
qualification_ref = "contracts/incident-journal-finality-v1"
verifier_revision = "incident-journal-v1"
keys = [{ id = "journal-k1", secret_env = "ACTEON_FINALITY_INCIDENT_K1" }]
```

Supply a dedicated hex-encoded key through the named environment variable.
References must begin with `ACTEON_FINALITY_`; decoded keys must contain 32–1024
bytes. Startup rejects missing or malformed keys, aliases of the authentication
or execution authority keys, duplicate bindings, conflicting source declarations,
undeclared scopes and declarations for read-only `history_only` scopes. There
can be at most 128 source declarations and 16 keys per binding. All declarations
and keys are resolved before authority publication, using the configured state
backend for the subsequent execution and reconciliation records.

`source_id` and `qualification_ref` identify the operator's reviewed source and
contract. They are deployment metadata, not evidence that the source actually
implements irrevocable finality. The source must correlate the complete attempt,
including its token and binding digest. A no-effect receipt must guarantee that
no delayed worker or delivery can later perform that attempt. An empty lookup,
HTTP timeout or model judgment cannot provide this guarantee. HMAC keys are
shared secrets: every holder can issue receipts, so isolate the issuer from
ordinary participants and management request payloads.

Trust roots are immutable within a running server. Rotate keys by deploying a
new reviewed declaration, retaining any keys needed for unresolved staged proofs
under their original verifier revision. Removing a declaration disables that
binding's reconciliation on the restarted replica; it does not recall an already
accepted receipt or stop replicas still running the old declaration. Coordinate
replica replacement when retiring a source. Live source revocation and deployment
cutover fencing remain separate implementation work.

The declaration does not grant `can_reconcile`, install a provider, dispatch work
or automatically probe the source. Operators still need independent management
permission and an exact resource ceiling.
For retained work after all provider drivers are removed, use the explicit
[`reconciliation_only` scope mode](execution-permits.md#retire-providers-while-retaining-finality-management).
It permits evidence management while keeping execution, permit issuance,
interventions and workforce mutation disabled. `history_only` remains read-only.

### Qualified finality acceptance

Correlation is available at
`GET /v1/governance/executions/{execution_id}/attempts/{ordinal}/correlation`.
Acceptance uses
`POST /v1/governance/executions/{execution_id}/attempts/{ordinal}/reconciliation`.
Both require `namespace` and `tenant` query parameters, current private
management authentication and `can_reconcile`. Correlation is an observation for
the independently qualified source; its token never grants execution or settlement
authority. Responses disable caching.

The acceptance body contains only `proof_base64`, encoding up to 64 KiB of opaque
source evidence. Requests cannot choose an actor or verifier. The response is a
typed provider receipt with the final outcome. Exact-proof replay preserves the
original acceptance audit. Invalid evidence, missing qualification, stale authority
and out-of-scope ownership refuse acceptance without sending or retrying work.

| SDK | Correlation | Acceptance |
|---|---|---|
| Rust | `provider_reconciliation_correlation` | `accept_provider_reconciliation` |
| Python, sync and async | `provider_reconciliation_correlation` | `accept_provider_reconciliation` |
| TypeScript | `providerReconciliationCorrelation` | `acceptProviderReconciliation` |
| Go | `ProviderReconciliationCorrelation` | `AcceptProviderReconciliation` |
| Java | `providerReconciliationCorrelation` | `acceptProviderReconciliation` |

The SDKs preserve the scope query, original correlation and typed completed/no-effect
outcomes. They do not automatically retry acceptance. A timeout can mean the
acceptance committed: obtain fresh management authority and replay the exact proof.
Do not replace a staged receipt with a new verifier revision or infer no-effect
finality from a timeout or missing external record.

## Coverage

These controls govern the qualified immediate execution profile. They do not
cancel already started external operations or supply compensation. Closure state
is per exact resource; independently overlapping named closures, drain/pause,
external cancellation acknowledgments and intervention reconciliation remain
separate work. [Agent workforce](workforce.md) supplies teams and representation
mandates for this immediate execution profile. Shared delegated funding and
protected deferred execution remain separate platform capabilities.


### Host integration: approved peer discovery

Rust hosts can compose the existing individual agent registry with independently
credentialed participant contexts through `acteon_executor::delegation`.
`ApprovedPeerBinding::new_trusted` pins a reviewed card, skill, endpoint,
transport, recipient principal and complete `agent.invoke` effect. Each effect
includes the peer's scope-local Agent resource. The registry agent ID and
recipient principal are an explicit host mapping; card descriptions cannot
establish that identity.

`ApprovedPeerRegistry` reads the existing agent and card records through the
configured `StateStore`. Discovery checks current administrative state, online
status and the exact approved card digest. It reads the actual card even when
the separate `has_agent_card` presence hint is stale. The hint grants no
authority: a missing or changed card produces no candidate, and an enrolled
registry qualification must still be current and active. Changed cards need
renewed host approval. The candidate contains bounded descriptive data, the binding digest
and the observed authority revision. Treat descriptions as untrusted data.

```rust
let candidates = registry.discover_candidates("diagnose", PeerCandidateQuery {
    coordinator: &coordinator,
    parent: &verified_parent,
    parent_permits: &parent_permits,
    recipients: &trusted_recipient_resolver,
    clock: clock.as_ref(),
}).await?;
```

Discovery checks the source before calling the trusted resolver, which supplies
each recipient's independently verified,
credentialed context and permit references. Discovery checks the complete effect
against both participants' original accepted ceilings and current credentials,
permits, workforce relationships, closures, cancellation fences, deadlines and
available budgets. Both contexts belong to the same scope. Self-delegation and
cycles through a participant already funding the parent branch are refused.
Backend failures, corrupt records and stale authority fail the query; ordinarily
denied candidates are omitted.

Discovery is advisory and writes no reservations, children or start leases.
The returned revision describes the observation; it is not authority to invoke
a peer. Cross-participant child admission must preserve both authority sources
and establish shared sponsorship before a transport adapter can register a send.
That admission and real A2A runtime handoff remain subsequent implementation work.
This host integration currently has no HTTP discovery route or model-callable
runtime tool. HTTPS URL syntax approval also requires a separately qualified
transport with network confinement, endpoint authentication and protocol checks.


### Service delegation and shared sponsorship

A service request can authorize an agent to do work that its caller cannot execute
personally. Trusted runtimes use a `DelegationGrant` to pin the actual source and
recipient principals, individual agent resource, approved binding digest, skill,
complete `agent.invoke` ingress effect and qualified effect footprint. The ingress
resources include every resource in that footprint. Direct execution permissions
remain separate: the caller needs current authority for the ingress request,
and the recipient needs its own current credentials, permits and workforce
relationships for each effect it executes.

Publish and retire these grants through
`AuthorityCoordinator::publish_delegation_grant` and
`revoke_delegation_grant`, using independently authenticated management bounds.
Publication and terminal retirement share the coordinator's StateStore CAS
boundary with effect starts. Accepted executions keep both the original grant
revision and the current restriction checks. A later expansion of a grant cannot
expand an already accepted request. Retirement requires a new grant ID for new
authority; it cannot resurrect an old execution's rights.

At initial acceptance, `TrustedContextStore::capture_delegating_root` seals the
selected grant references into the durable root acceptance journal. The host then
uses `capture_delegated_child` with independently authenticated recipient
admission for the **actual new input**, its direct effects, complete service intent,
credential and permit references, qualified binding digest and optional verified
workforce representation. For represented work, the verified initiator must be
the immediate source participant. Root requester identity remains available
separately through `original_requester()`; it cannot supply the recipient's team
membership or substitute for its actual caller.

The child receives a recipient-owned budget leaf linked to the existing sponsor
ancestry. Allocation creates no separate recipient funding root and spends no
attempt units. Each registered effect charges that leaf and every sponsor
ancestor once, within the same CAS. Known settlement releases their concurrency;
spent units remain spent. Cancellation, subject revocation, grant retirement and
closures of the enclosing agent resources apply throughout the branch. The budget
pins the admitted recipient context reference to prevent execution-ID rebinding.

Onward grants and their complete footprints must fit inside the already accepted
service intent. This allows a coordinating agent to request another service while
remaining unable to execute that service's private operations directly. Nested
starts recheck each source's ingress authority, each ancestor's original and
current service intent, and the final recipient's direct authority. Same-actor
workflow continuations preserve the delegation proof and sponsorship.

These are trusted Rust runtime APIs. Registry selection, input qualification,
agent-specific runtime binding and A2A transport must supply the approved plan
and independently authenticated recipient acceptance. The existing advisory
registry preview is separate from child admission. Public delegation routes,
client tooling and the durable peer transport belong to the mesh integration
phase.

New contexts use signed format 5. Previously accepted formats 2–4 remain readable;
new grants cannot be attached to an old acceptance by replay. Upgrade and drain
older workers before activating the new grant event types, since those workers
refuse records they cannot understand. Retain the admission journals, context
verification keys and historical grant publications needed by live descendants
and reconciliation.

#### Discovering a delegated service

An approved peer can carry an `ApprovedServicePlan` with two exact effect lists:
its complete downstream intent and the operations that the recipient may perform
directly. `ApprovedPeerBinding::new_service_trusted` pins both lists together with
the card, skill, endpoint, transport, and executing principal. Changing the plan
changes the binding digest and requires a corresponding accepted grant.

For these bindings, `ApprovedPeerRegistry::discover_candidates` checks the
caller's current ingress permission and an initially accepted service grant
before resolving private recipient authority. It then reads the current registry
and rechecks the grant and both participants after those reads. The recipient's
private effects are checked against its own credential and permits; the caller
needs the invocation permission and approved service intent, without receiving
those private permissions. Retirement, narrowed intent, expiry, closures, and
participant revocation refuse a candidate.

A service candidate includes the selected `accepted_grant` reference. This is
advisory metadata, not execution authority. The host still authenticates a fresh
`RootContextAdmission` for the actual recipient input and calls
`capture_delegated_child`; a discovery context is never reused as the invocation
or its payer. Discovery creates no context, budget allocation, or effect start.
Ordinary peer bindings retain their existing intersection-based preview.


### Durable individual-agent task execution

`AgentProviderRuntime` connects an authenticated recipient context to a qualified
provider operation and the existing task engine. The host supplies the approved
service binding, provider, signed context, and permits. Task messages cannot
choose an executing principal, provider, endpoint, or service plan. The signed
input digest must match the message mapped to that provider operation.

Acceptance persists an immutable journal through the configured StateStore. The
recipient execution ID becomes the task ID, so a lost acknowledgment or runtime
replacement recovers the same work. A task projection can be reconstructed from
that journal; execution evidence determines whether the operation is already
complete, still running, or requires reconciliation. An ambiguous provider result
remains unresolved across restart and stale-task reaping.

Acceptance checks current authority, cancellation, deadlines, and remaining
units without reserving an execution slot. This allows a recipient to acknowledge
queued work while its sender holds the sponsor's last concurrent slot. Every
actual provider start still uses the governed executor's atomic authority and
shared-budget reservation. Revocation or closure between acceptance and start
refuses execution. Known results and terminal task state are projected together.
Recovery rejects copied acceptance records and substituted task identities.
Terminal result artifacts are checked against execution evidence and repaired
from that evidence without another provider call. The reaper also checks the
acceptance journal, so removing a projection's metadata cannot certify an
uncertain outcome.

This is a trusted Rust host adapter for one qualified provider operation.
Agent-specific HTTP/A2A configuration, authenticated network envelopes, transport
qualification, cancellation acknowledgment, and adapters for other runtime
families remain separate integration work. No new client SDK wire API is exposed
by this adapter.
