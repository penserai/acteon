# Governed descendant accounting and provenance

Status: proposed implementation contract following workforce PR #427.

This extends the city implementation; it does not redefine the full objective
around immediate webhook execution. The next delivery must carry trusted
provenance through real workflow steps and recovery. Autonomous cross-principal
A2A handoff and team funding follow on the same accounting contract.

## Decision

A child execution has its own semantic input digest and opaque context handle,
but cannot acquire an independent root allocation merely by being redispatched.
Its root identity is derived from verified parent provenance. A public action,
agent card, workflow output or SDK label cannot select a payer or ancestry.

Every effect start atomically reserves against the shared root and every
applicable descendant ceiling in the configured StateStore coordinator CAS.
Settlement releases concurrency in the same ledgers. Spent units remain spent;
uncertain effects retain their reservations. A child cannot multiply a narrow
branch budget by spawning grandchildren. Parallel siblings cannot overspend the
root. Ledger relationships are immutable, acyclic and bounded in depth and count.

Signed child provenance retains the actual actor, original initiator,
representation and its pinned dependencies, parent and root identity, accepted
operation/resource tuples, deadline and independently qualified input digest.
The child effect set and deadline cannot exceed its parent. Changing actor or
represented party requires an explicit validated delegation/mandate edge;
ordinary same-actor workflow continuation cannot establish that authority.

Persist the original child admission before exposing it to a worker. A lost
acknowledgment or second replica must recover the original handle, execution,
root, ceilings and input binding. Context persistence and ledger registration
are repairable steps; an incomplete admission never authorizes an external send.
Effect registration remains the authority and budget linearization point.

## Workflow delivery

1. Add signed child context construction, recovery and immutable root/parent
   binding. Older root contexts remain readable as roots, without acquiring
   ancestry or delegation rights.
2. Add atomic descendant accounting, including history validation and settlement
   repair. If the coordinator wire shape changes, extend the explicit reviewed
   cutover; never silently populate authority from legacy records.
3. Admit a qualified workflow plan with independently validated route footprints,
   definition/version digest and original inputs. Bind each actual step input to
   a child context before invoking its qualified provider.
4. Persist references in workflow/chain work records, waits and retry receipts.
   Reevaluate current permits, credentials, workforce dependencies and closures
   before each effect, including after approval and restart. Approval is a job
   disposition, not a new source of tool authority.
5. Add typed SDK and UI inspection/control surfaces, public documentation and a
   real multi-step HTTP scenario with a durable backend and independent workers.

Legacy unprovenanced work in an enforced scope is parked for reviewed recovery,
not assigned the current worker's privileges. Dynamic rerouting, fallback and
model output must stay inside the accepted qualified effect set. Unsupported
adapters fail qualification instead of using an ungoverned execution path.

## Acceptance evidence

- Parallel children compete for one remaining root unit and one concurrency slot;
  exactly one effect is allowed, with no counter drift on the second replica.
- A branch capped at one unit cannot spend again through a grandchild or sibling
  admission replay; a privileged recipient cannot enlarge that branch or root.
- Lost admission/settlement acknowledgments and process replacement preserve
  original identities, reservations and evidence without duplicate sends.
- Relevant membership, mandate, principal or permit revocation during a workflow
  wait refuses the next effect without spending or releasing uncertain attempts.
- Closure-before-start refuses; start-before-closure remains honestly in flight.
- Tampered job definitions, inputs, ancestry, payer, context references and
  cross-tenant bindings fail before the receiver observes any effect.
- A real workflow performs two qualified steps under one root; restart and a
  retry reuse its provenance, and intervening offboarding blocks the next step.
- Memory plus a durable backend pass the same independent-client contract. SDKs,
  UI, public guide, final-head CI and publication are part of delivery.

Team allocation limits and autonomous A2A still need explicit issuance,
recipient entitlement and runtime-binding policy. Neither is inferred from
roster membership, funding metadata, a card advertisement or this child context.

## Implementation checkpoint: qualified plans and durable handoff

The uncommitted descendant branch adds host-only qualification of complete
chain definitions, including all provider branches, parallel steps, sub-chains
and cancellation targets. The qualification digest pins definitions, original
semantic input and actual registered provider revisions/footprints. A plan
requires explicit `chain.start` authority for each enclosed chain as well as its
provider effects; qualification does not issue that authority. Worker and full
pipeline dispatch adapters currently refuse qualification until their authority
handoffs are implemented.

Signed descendants retain enclosing chain restrictions. Effect registration
checks their closures in the same coordinator CAS as current permits and shared
budget accounting. Receipt recovery validates the same complete resource set;
it does not silently discard those restrictions.

`PlanHandoffStore` uses the configured `StateStore`, with optional deployment
payload encryption. Before a job becomes discoverable, it pins the original
input, accepted definitions, signed root reference and exact permit revisions.
Recovery rebuilds qualification against actual registered provider instances and
verifies the root's signed plan/input binding. A changed route revision or
corrupt definition refuses recovery. Historical recovery is observational;
current child admission and effect registration remain separate checks.

Before child admission, one host-selected logical attempt atomically retains its
candidate context handle, execution ID, input, parent, limits and call site.
Competing replicas or a lost acknowledgement recover that original identity.
Changing input, parent or limits under the same logical attempt is a conflict.
Admission repair retains the attempt identity; an actual new effect attempt
requires an explicit new host identity under the same root. These records have
no TTL: provenance must not disappear while durable work can resume. A bounded
retention/reconciliation policy remains part of lifecycle delivery.

The next integration must establish job ownership from authoritative chain
state, capture a plan root from private authentication and independently allowed
plan bounds, persist handoff before indexing work, and pass the recovered child
admission to the actual selected provider boundary. Public job IDs and context
references alone must never construct that adapter. Existing chain execution is
not yet wired to this store.

Per-instance cancellation must also linearize through the authority coordinator
before its chain-status projection. Closing a chain definition already blocks
new enclosed effects, but cancelling a job in a separate state record cannot
fence a child that was prepared before cancellation. Cancellation notifications
need explicitly qualified standing authority; cancellation must not implicitly
create permission for cleanup effects.

## Implementation checkpoint: authenticated plan roots

Deployment scopes now declare concrete `chains` with explicit eligible
`subjects`, separately from qualified provider routes. Deployment-issued permits
also list their allowed chain names. Wildcards, duplicate names, undeclared
subjects and permit chains outside those independent bounds are rejected during
read-only preparation. The full chain/subject declaration is canonicalized and
included in the deployment policy fingerprint. Empty chain declarations retain
the existing provider-only fingerprint shape.

The credential projector adds `chain.start` effects only for an explicitly
listed principal with a dispatch-capable role and a matching authenticated
namespace/tenant grant. Each provider effect still needs its own credential
grant. Complete plan admission requires current exact permits for every enclosed
chain and provider effect, including sub-chains. A provider-only permit cannot
be upgraded into a plan permit by a request or model output.

`PreparedExecutionScope::capture_plan_root` derives the principal, credential,
configuration reference and initiator from the private middleware binding. It
reuses the same current limit attenuation, represented-work evaluation and
credentialed root admission as the existing immediate provider path. The job
class is the qualified entry chain name, with representation still requiring an
explicit mandate. Public labels do not provide any of those facts.

`ExecutionAuthorityRuntime::admit_chain_job` derives the durable root admission
key from the authoritative job UUID, captures the signed root, and persists the
accepted plan through its scope's configured `StateStore` and payload encryptor.
Changing a candidate key cannot allocate another root for that job. Historical
recovery remains available after credential revocation; it does not authorize a
fresh admission. The engine must establish ownership of the job before calling
recovery and must use the planned child adapter at its actual provider boundary.

These are host building blocks, not a connected public chain execution feature.
Chain engine and deferred worker wiring, explicit chain-aware management and
workforce job-class declarations, cancellation fencing and release verification
remain part of delivery. Management rights are not enlarged by deployment chain
admission declarations.


## Connected engine boundary (unreleased)

The server installs a trusted chain execution mediator alongside its provider
mediator. Root chain admission receives private middleware proof and persists
plan provenance before chain state and discovery indexes. Background provider
steps receive child authority from that provenance, the actual selected provider
and a stable host step-attempt identity. Sequential provider steps and flat
parallel provider groups are connected; workers execute pinned definitions.

The connected path has actual-engine tests for shared budgets, enclosing-chain
closure, lost result projection, registry changes and forged/missing work
provenance. A catalog-qualified binding must be shared by admission and the
execution driver. An unqualified driver correctly fails exact effect checks;
the test fixture now follows the production catalog-resolution path.

Uncertain-work parking, sub-chain/nested parallel adapters, cancellation authority and standing cleanup
remain required. The supported server profile rejects unsupported work before
root publication; it must be extended as those adapters become enforceable.
No storage requirement changes: all authority and work evidence use the
configured state backend.


### Observation is distinct from effect admission

Owned chain work can inspect its retained call record and signed child context,
then recover a completed result through the exact qualified provider driver.
This path does not create a call reservation or acquire fresh child authority.
It remains valid for historical evidence after expiry or revocation, while the
next actual effect still requires current admission. Missing evidence cannot be
interpreted as success; corrupt or unavailable evidence cannot fall back to an
execution attempt. Receipt settlement repair is allowed without provider replay.

The gateway checks this observation path before fresh admission. The default
request admission has no observation path; the strict provider mediator exposes
inspection through trusted host identity and exact selected-instance checks.
No public context reference becomes a bearer credential. Durable parking and
reconciliation of uncertain work remain separate required lifecycle features.


### Pending work is an observable lifecycle state

The provider boundary now returns typed `ProviderPending` evidence, distinct
from `Failed`. Owned workflows persist `waiting_provider`, the original logical
attempt and receipt identities. Pending observation never rotates that identity
or records a failed step. Unresolved evidence outlives the workflow deadline;
its retained budget charge cannot be released by elapsed time.

Parallel execution drains started calls in bounded batches, retains known
sibling results and inspects original receipts after timeout. A decisive `any`
or fail-fast result prevents later batches from starting; unresolved calls from
the started batch still need evidence before finalization. Status/wait visibility
and dispatch decoding accompany the core type through server, all SDKs and UI.

Trusted external reconciliation and an instance cancellation fence remain
required controls. Reconciliation must bind independently trusted evidence to
the original attempt and preserve uncertainty history; a client/model claim or
an operator assumption is not proof of no effect. Cancellation must commit its
authority fence before its workflow projection, so a late worker cannot acquire
a fresh start merely because it previously loaded a running chain row.


### Independent finality and uncertainty history

An immutable provider result is never overwritten to claim reconciliation.
The separate finality attestation and coordinator reference retain original
evidence and the prior unresolved status. A trusted local verifier authenticates
the original context, actual action identity, attempt nonce/ordinal and complete
binding digest; accepting finality settles concurrency once without refunding
spent units or allowing another send. Uncommitted attestations require verifier
revalidation after restart. Accepted digest-pinned attestations are historical
facts and do not require an active verifier for observation.

Known ordinary provider evidence wins over an uncommitted candidate, including
an interrupted ledger acknowledgement. A late worker cannot repin evidence or
return a finalized attempt to uncertain. A no-effect proof requires irreversible
external finality for all effects and possible future deliveries. Signature
verification alone is not provider/attempt correlation or proof of that contract.
Qualified proof ingress, independent probe authority and authenticated
management adapters remain required integration work.

`HistoricalProviderStore` independently projects owned provider history through
the configured state store, retained signed contexts and digest-pinned evidence.
It uses no active provider/catalog/retry configuration or verifier and performs
no writes or repairs. Lookup by execution ID verifies the original context before
returning evidence. Full-operation seals authenticate delivery/retry metadata;
legacy pinned results or finality can authenticate binding metadata separately.
Uncommitted proofs and unacknowledged results remain pending. Parent cancellation
is retained in the projection without granting new execution authority.


### Retiring a provider without retiring its evidence

A validated history-only execution scope connects to retained configured state,
keeps independently reviewed historical effects, and projects credentials with
no executable effects. It uses an explicit empty history catalog and a mediator
that refuses every invocation. Normal executable constructors still reject
empty installations. No live provider adapter is needed for the management read.
Bootstrap, chains, deployment permits and governance/workforce write grants are
invalid in this mode. Reader authority remains current and subject-bounded;
historical context signatures and original evidence retain their original
identity even after credential projection advances. The UI exposes only receipt
inspection for this grant and keeps original outcomes separate from accepted
reconciliation evidence.
