# Execution-scope credential publication

Status: implementation in progress following merged logical credential enrollment
(PR #419). The branch implements a qualified provider catalog, credential policy
projection, ordered auth-provider publication and private original scope
bindings. Production configuration/startup and the gateway effect path are not
installed yet; this is not an enabled server execution profile.

## Security boundary

Authentication now resolves the actual logical credential and observes the
shared security configuration. Execution must bind those original facts to the
credential policy in the target scope. It must never look up the newest
configuration reference and attach it to an older caller identity.

Use the gateway's configured `Arc<dyn StateStore>` for every coordinator and
retained record. A shared storage backend does not make separate coordinator
CAS operations atomic. Memory remains process-local.

## Qualified effects

Build a finite, immutable catalog from the actual provider instances constructed
by the server. Every entry contains scope, selected provider, action, endpoint
identity and version, complete protected resources, and reviewed failure behavior.
Qualification must include fallback targets. Catalog resolution checks the actual
selected instance, not just its name. The qualified binding version is itself a
protected route resource, derived from the endpoint, primary action, complete
resources and failure-contract revision; an accepted root cannot silently adopt
a replacement definition. Endpoint revisions bind the actual
validated destination and relevant immutable settings; secrets contribute only
to keyed fingerprints, never plaintext state or inspection responses.

Resolve each enrollment's own grants against that catalog. Preserve every grant
dimension and dispatch-role restrictions. Never union policies for one actor.
Dynamic destinations, internal retries and auxiliary effects require specific
resolvers; an enabled scope must reject unqualified paths. Initial static
adapters are a delivery step, not the completion criterion for all execution.

## Publication protocol

Declare the execution scopes managed by the deployment. A reviewed monotonic
security revision must cover authentication and qualification policy changes.
For each scope, derive one complete credential configuration snapshot from the
same validated inputs, with terminal lifecycle enforced by its coordinator.

Publish target-scope snapshots before acknowledging the new auth-control epoch.
Only replace local authentication tables after every required target snapshot
and the auth-control epoch have been published. The control epoch fingerprint
binds the complete, canonically ordered scope-reference manifest, so replicas
with different qualification/projection inputs cannot join the same epoch. Retain the exact target
configuration references in those tables and in the private authenticated
binding. A partial update is restrictive: old references in updated scopes must
fail, while unchanged scopes retain their previous policy. Retry identical input
to reconcile acknowledgments; never roll back an acknowledged scope to an old
revision. Startup and replica reconciliation use the same protocol.

This ordering is a candidate protocol that requires interleaving proofs. It must
not be described as a transaction across scopes. Unsupported writer paths must
be refused; a later source publication cannot retroactively qualify an earlier
scope mutation.

## Admission and effect enforcement

Capture roots only with the original credential/configuration references and
current issued permits. Validate scope management authorization against the same
scope snapshot whose stamp is used for the mutation CAS. Cache neither role nor
permit decisions as permanent authorization.

At every real provider attempt, including retries and deferred continuations,
validate the current credential, permits, complete effect resources and root
budget through the target coordinator. Reuse retained outcomes rather than
replaying uncertain effects. Qualified provider bindings must survive workflow
persistence and restart; caller labels and imported contexts cannot create them.

Auth-control principal disablement stops authentication. Enforcement on accepted
roots requires the corresponding target-scope principal/permit restrictions.
Global controls must report per-scope application and partial failures rather
than claiming an instantaneous transaction across the city.

## Required adversarial evidence

- Real HTTP replicas, each resolving its own actual credentials and provider
  instances, cannot refresh old authentication into broader scope authority.
- Every interruption between scope publication, control publication and local
  table swap leaves restrictive state and supports identical-input recovery.
- A late stale publisher cannot restore policy or resurrect a retired ID.
- Changed provider destinations and fallback selection invalidate qualification;
  dynamic or auxiliary effects cannot escape through an undeclared path.
- Scope mutation races, root admission races, closure/revocation during waits,
  and restart recovery use current authority at their actual CAS boundaries.
- Actual backend and server fixtures prove publication on supported configured
  state stores; SDKs, UI and public docs expose provisioning and inspection.
- The workforce scenario uses real personal/team delegation, peer invocation,
  offboarding and closures once those platform features are installed.

Provisioning APIs, team memberships and mandate lineage remain part of the
original platform objective. Completing this publication integration will not
complete the workforce or city-control phases by itself.


## Current evidence and remaining integration

Focused contracts exercise actual HTTP provider invocation and retained-result
recovery, refusal of same-name replacement instances, independent credential
policies, terminal omission retirement and original-reference freshness. A real
HTTP middleware contract injects a lost scope publication acknowledgment: the
old transport identity remains observable but its original execution binding is
ineligible; identical-input retry reconciles without refreshing the old proof.
A node omitting the declared projection is rejected by the control-epoch digest.

The next changes must construct qualified bindings from actual server provider
factories and immutable client settings, declare independent publication ceilings
and fixed policy deadlines, defer auth exposure/watching until full publication,
and pass the captured binding into real root/effect enforcement. The two-scope fault contract now proves restrictive partial publication and
identical-input convergence. Concurrent interleavings, production binary and
configured-backend projection contracts remain required.
Public provisioning/inspection APIs, SDK/UI coverage and the workforce scenario
remain required rather than being inferred from these internal contracts.


Implementation notes for production wiring: obtain client identity/material once
and use those exact loaded values for both construction and keyed qualification,
rather than hashing a path then re-reading a potentially changed file. Disable
unregistered transport retries for the qualified adapter. Publication ceilings
must independently authorize historical removals; do not borrow prior policies
from state to enlarge them. Profile configuration must bind all declared scopes,
and gateway enforcement must refuse a missing original binding rather than
assuming an undeclared scope is a legacy path. Scope removal and deployment
migration need explicit lifecycle handling before advertising a turnkey profile.
