# Execution-scope credential publication

Status: host building block merged in PR #420 following logical credential enrollment
(PR #419). The implementation provides a qualified provider catalog, credential policy
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
Actors outside a scope's independent issuance ceiling are omitted when their
grants resolve to no executable effects there. If their grants do resolve to an
effect, publication fails rather than expanding that ceiling. Managed actors
with no executable effects retain disabled records. Historical retirement still
requires independent authorization over the previous records.
Dynamic destinations, internal retries and auxiliary effects require specific
resolvers; an enabled scope must reject unqualified paths. Initial static
adapters are a delivery step, not the completion criterion for all execution.

## Publication protocol

Declare the execution scopes managed by the deployment. A reviewed monotonic
security revision must cover authentication and qualification policy changes.
For each scope, derive one complete credential configuration snapshot from the
same validated inputs, with terminal lifecycle enforced by its coordinator.

Publish target-scope snapshots before acknowledging the new auth-control epoch.
Execution projectors must reject the authentication control scope before writing
any state; the control scope remains dedicated to authentication epochs.
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
rather than hashing a path then re-reading a potentially changed file. PR #421
introduced `LoadedTlsClientConfig` and makes server HTTP client construction
reuse that snapshot. Its opaque keyed fingerprint includes the loaded identity,
CA bundle and certificate-verification policy. A real mTLS contract removes the
source files before connecting and verifies the exact client certificate at the
server. The static webhook factory now incorporates this fingerprint alongside
the actual destination, canonical headers, outbound policy and reviewed transport
behavior. Server startup uses that factory for its actual webhook instance.
Its host-only binding method creates scope/action qualifications tied to that
same instance; production scope declarations and catalog installation remain
required. The adapter disables redirects, proxies and transport retries.
Other adapters still require complete qualification of their actual effects. Disable
unregistered transport retries for the qualified adapter. Publication ceilings
must independently authorize historical removals; do not borrow prior policies
from state to enlarge them. Profile configuration must bind all declared scopes,
and gateway enforcement must refuse a missing original binding rather than
assuming an undeclared scope is a legacy path. Scope removal and deployment
migration need explicit lifecycle handling before advertising a turnkey profile.

## Production integration working branch

The working integration defines bounded scope declarations, retains actual
registered webhook factories, and prepares every declared catalog before any
publication. Preparation rejects missing, unsupported, duplicate or mismatched
provider registrations. The canonical deployment-policy fingerprint includes
independent publication authority and root allocation limits; the scope
projector includes it in the credential configuration fingerprint. A root-limit
change therefore requires a new reviewed security revision even when provider
settings and credential ceilings remain unchanged.

The private authentication proof retains the prepared policy fingerprint as
well as the original source reference. Root admission rejects a proof for a
different runtime policy, resolves the actual selected provider, computes the
semantic input digest, and derives the actor from authentication. It captures a
credentialed root with explicitly selected current permits and the evaluated
scope stamp. Missing permits do not allocate a root. This host boundary remains
unwired from public dispatch until the common mediator and scope fencing exist.
A signed, non-expiring admission record in the configured state backend pins
the first context handle, execution ID, exact credential and permit revisions,
effect, budget limits and deadline. Concurrent retries converge on that record.
Recovery verifies the original work and rechecks current authority before
publishing a missing context or budget. It preserves existing spending and
cannot extend the deadline. Retained signing keys support recovery after
rotation; record modification and transplantation fail closed. This is not an
aggregate team budget or an exactly-once effect guarantee.

Authentication publication and watcher startup follow actual provider
construction and gateway validation. A real configured-backend process contract
proves that failed runtime construction leaves the existing authority record
and a live replica's authentication intact. This is startup ordering evidence,
not provider-attempt enforcement.

Coordinator format 8 adds mandatory scope ownership metadata. Trusted hosts
reserve virgin scopes permanently for execution or one authentication source;
conflicting claims serialize through CAS, and retained reservation history must
agree with ownership metadata. Control scopes refuse execution
state while retaining source publication and principal revocation. Older
coordinator protocols are refused without recreation. Existing installations
require a reviewed cutover that fences old writers and preserves outstanding
reconciliation obligations. The explicit `scope-upgrade` command previews a
protocol-7 or unclaimed protocol-8 record and applies only its reviewed digest
through CAS. It preserves existing accounting, provenance and uncertain work,
and refuses incompatible classification or malformed history. Normal startup
never invokes this cutover.

The gateway now has one selected-provider execution interface in place of its
raw executor field. Direct execution, modifications, deduplication, throttling,
reroutes, circuit fallbacks and synthesized approval notifications/retries pass
the actual selected provider instance and resolved dispatch context to that
interface. An installed mediator's refusal never falls through to legacy
execution. The default adapter retains the existing executor and its shared
concurrency, clock, retries and DLQ behavior. This interface does not itself
establish authentication or qualification. The strict `GovernedProviderMediator`
retains credential-required durable executors and accepts only explicit
host-created `ProviderExecutionAuthority`. It checks the actual selected instance
and refuses unsupported attachment contexts. Uncertain attempts remain reserved
for reconciliation and never trigger an automatic resend through the gateway.
`PreparedExecutionScope::capture_provider_authority` constructs this capability
from the original private authentication proof and durable root admission.
The gateway's `dispatch_with_execution_admission` takes a borrowed trusted
admission adapter and propagates it explicitly through direct, modified,
deduplicated, throttled, rerouted and fallback calls. Admission runs on the final
actual selected work. The server's `AuthenticatedProviderAdmission` retains the
original private authentication binding and stable operation identity, and
constructs invocation authority through durable capture. No caller labels,
task-local state or automatically inherited background identity establish proof.
This entry point requires an authority-enforcing mediator; using it with the
legacy executor is refused before effects. The strict mediator also refuses
dispatches missing private admission before deduplication/throttle state changes.
For an authenticated request, deduplication and throttle conditions are applied
after admission of the actual selected work; missing permits therefore cannot
consume slots before a valid retry. The provider attempt gate still performs
current authority checks after these operational conditions and before sends.
Approval notifications and retries receive no inherited request capability.
Real webhook contracts verify all six immediate paths and completed replay,
changed-work refusal, revocation and refused notification inheritance. Public
HTTP handlers and production startup still need to install these building blocks.
LLM and other effects
outside selected-provider execution require their own mediated boundaries.

Before enabling the server profile, install the prepared root-admission path
on public HTTP dispatch and the durable governed adapter at startup,
and reject missing provenance on every enabled
execution path. Rules, deferred work, transformations, peer calls and delegated
work require their corresponding protected context propagation; preparing a
static catalog does not establish that coverage. Public configuration,
provisioning, SDK/UI support and full workforce delivery remain required.
