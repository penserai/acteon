# Execution permits

Enable execution authority to require current permits for qualified provider
calls. A human, agent or deterministic service authenticates normally, then
selects explicitly issued permits for each request. Acteon admits the final work
selected by rules against those permits and a bounded root budget before calling
the provider.

Authority, signed contexts, budget accounting and execution records use the
configured `[state]` backend. Redis is one option alongside memory, PostgreSQL
and DynamoDB. Memory is suitable for local tests; choose a persistent backend
for recovery across server restarts.

## Configure the deployment

Start with [shared authentication authority](../api/authentication.md). It needs
an authentication-control scope separate from the execution scope, a stable
logical credential ID and a stable principal in the authentication file. For
example, an API key can bind `authority_id = "credential/maya"` to
`principal = { id = "agent/maya", kind = "agent" }`, with executor role and
ordinary grants for `prod/acme/incident/execute`.

Set `ACTEON_EXECUTION_AUTHORITY_KEY` to a secret of at least 32 bytes and retain
it across replicas and restarts. It signs private execution contexts and binds
prepared policy. Keep this distinct from `ACTEON_AUTH_KEY` and
`ACTEON_AUTH_AUTHORITY_KEY`; inject the keys from your deployment's secret store.

Add this to the server configuration, substituting your actual webhook endpoint:

```toml
[auth]
enabled = true
config_path = "auth.toml"

[auth.authority]
namespace = "auth-control"
tenant = "deployment"
source_id = "workforce-auth"
bootstrap = true

[[providers]]
name = "incident"
type = "webhook"
url = "https://incident.example.com/events"

[[execution_authority.scopes]]
namespace = "prod"
tenant = "acme"
bootstrap = true
publisher = { id = "operations-owner", kind = "human" }
subjects = [{ id = "agent/maya", kind = "agent" }]
routes = [{ provider = "incident", action_type = "execute" }]
valid_from_ms = 0
credential_limits = { max_units = 5, max_concurrent = 2, deadline_ms = 4102444800000 }
root_max_units = 5
root_max_concurrent = 1
root_lifetime_ms = 60000

[[execution_authority.scopes.permits]]
id = "maya-incident"
revision = 1
subject = { id = "agent/maya", kind = "agent" }
routes = [{ provider = "incident", action_type = "execute" }]
valid_from_ms = 0
limits = { max_units = 5, max_concurrent = 2, deadline_ms = 4102444800000 }
```

`bootstrap` permits explicit initialization of a new scope; existing legacy
scopes require the reviewed [scope cutover](../api/authentication.md).
Declarations qualify actual registered provider instances and concrete routes.
Permit publication is independent of credential grants. Omitting a previously
published permit does not revoke it. Change permit revisions explicitly and
retain independent issuance bounds.

The publisher's identity kind describes its actor; it does not establish team
membership or confer authority. Scope declarations explicitly provide issuance
bounds. Choose validity and root limits appropriate to the operation rather than
copying the example's long validity horizon into production.

Manage issued permits and resource closures through [governance management](governance.md).

## Retire execution while retaining history

An execution scope can become a history-only scope after its live routes have
been retired. This mode uses the same configured state backend and the existing
scope identity. It needs no registered provider and creates no executable
credential effects.

For the existing scope declaration, set these fields:

```toml
bootstrap = false
history_only = true
routes = []
chains = []
permits = []
```

Retain the original namespace, tenant, publisher, subjects and reviewed finite
bounds. Set `historical_effects` to the exact previously reviewed effect
footprints, including their operations and concrete resources. Capture those
footprints from the qualified deployment or the bounded governance route view
before removing the routes. These are retained publication bounds; the server
does not turn them into executable routes or infer new authority from stored
operation labels.

Every manager in this scope must have `can_read_history = true`, an explicit
subject allowlist and `routes = []`. Disable `can_issue_permits` and
`can_intervene`, and remove workforce management declarations. Keep ordinary
credential namespace/tenant grants for the enrolled reader, with `operator` or
`admin` role. Increase the shared authentication `authority_revision` when
publishing this policy change; old replicas and captured proofs cannot use the
new read policy.

Retain the execution signing key and the payload encryption key when encryption
is enabled. History-only startup connects to an existing scope and refuses
bootstrap, live routes, chains, deployment permits and governance/workforce write
grants. Reading a receipt neither revives an old permit nor releases an
unresolved attempt's charges. See [historical receipts](durable-executions.md#historical-receipts)
for the evidence contract.

## Retire providers while retaining finality management

Use `reconciliation_only = true` when unresolved attempts still need qualified
finality acceptance after live providers have been removed. This is separate from
`history_only`, which continues to reject all write management.

```toml
bootstrap = false
history_only = false
reconciliation_only = true
routes = []
chains = []
permits = []
```

Keep the existing scope identity, subjects, finite bounds, `historical_effects`,
execution signing key and any payload encryption key as described above. Startup
connects to existing state and refuses to create a new scope. Managers may hold
`can_read_history`, `can_reconcile`, or both, with explicit subject allowlists.
Reconciliation requires the complete `reconciliation_resources` ceiling and an
operator-qualified source declaration for the original binding digest. Disable
permit issuance, interventions and workforce management, and keep manager routes
empty. See [finality-source configuration](governance.md#configure-an-external-finality-source).

Increase the shared authentication `authority_revision` when publishing this
policy change. The retained scope projects no executable credential effects and
installs no provider drivers. Old authentication observations are refused; a new
management observation can accept qualified finality and replay the accepted
proof while preserving the original audit. Receipt acceptance settles the
existing attempt's concurrency charge without dispatching or retrying work.
Retirement does not itself prove that an external effect stopped. A qualified
source must establish completion or irrevocable absence before settlement.

| Scope mode | Live provider work | History access | Qualified finality acceptance |
|---|---|---|---|
| Ordinary execution | Requires credentials and permits | Requires explicit read grant | Requires independent reconciliation grant and source |
| `history_only` | Disabled | Requires explicit read grant | Disabled |
| `reconciliation_only` | Disabled | Requires explicit read grant | Requires independent reconciliation grant and source |

## Dispatch with explicit references

Send `x-acteon-execution-permits` as a JSON array of `id` and
`accepted_revision` entries. Effectful requests require 1–16 unique references
in one header of at most 8192 bytes. A reference alone cannot authorize work;
the server derives the caller from private authentication middleware.

```bash
curl --fail-with-body http://localhost:8080/v1/dispatch \
  -H "Authorization: Bearer $ACTEON_API_KEY" \
  -H 'Content-Type: application/json' \
  -H 'x-acteon-execution-permits: [{"id":"maya-incident","accepted_revision":1}]' \
  --data @action.json
```

The SDKs provide typed helpers:

| SDK | Single action | Batch |
|---|---|---|
| Rust | `dispatch_with_permits(&action, &permits)` | `dispatch_batch_with_permits(&actions, &permits)` |
| TypeScript | `dispatch(action, { permits })` | `dispatchBatch(actions, { permits })` |
| Python, sync/async | `dispatch(action, permits=permits)` | `dispatch_batch(actions, permits=permits)` |
| Go | `DispatchWithPermits(ctx, action, permits)` | `DispatchBatchWithPermits(ctx, actions, permits)` |
| Java | `dispatch(action, permits)` | `dispatchBatch(actions, permits)` |

See the [SDK documentation](../api/polyglot-clients.md) for construction examples.
The Admin UI's Dispatch page accepts permit IDs and revisions in separate fields.
Batch references must cover every selected action. Dry runs can omit permits.

## Replay and failures

Keep the original action ID for a retry of the same request. Durable admission
retains root limits, deadline and execution identity; a completed provider call
returns its retained outcome without sending again. An invalid permit does not
reserve a replay marker against a later valid retry. If optional signing replay
protection is enabled, changed-input retries return HTTP 409. Current authority
checks still apply to replays.

Missing or malformed references return HTTP 400; missing authentication returns
401 and unavailable scope authorization returns 403. A failed admission returns
a `Failed` outcome with `EXECUTION_ADMISSION_REFUSED`. Uncertain execution may
require reconciliation; inspect the outcome and do not start fresh work to
force another send. SDK HTTP errors retain status and response text.

## Supported execution paths

This profile qualifies static webhook routes for single and batch dispatch,
including immediate rule modifications and selected routing. Unqualified
providers and unsupported private contexts refuse execution. Explicit deployment
chain declarations and matching chain/provider permits admit a qualified plan
before root work becomes discoverable. Sequential provider steps and flat
parallel provider groups inherit bounded child authority from that plan; workers
recover pinned definitions and preserve original provider receipt identities.
See [provider receipt recovery](durable-executions.md#provider-receipt-recovery).

Sub-chain, deferred worker and approval execution still need their own qualified
retained-authority adapters; they must not inherit a request's permit implicitly. The `durable=true` receipt
API is currently rejected in this profile even though provider execution itself
uses durable records.

The profile currently rejects guardrail, embedding and enrichment configurations
because their auxiliary calls are not qualified by this runtime. Complete mesh,
deferred workforce execution and auxiliary-effect governance remain separate implementation
work; enabling this profile does not establish those capabilities.


### Independent reconciliation evidence

The qualified provider host supports finality verification separately from
execution admission. `ProviderReconciliationVerifier` and the built-in
`HmacFinalityVerifier` authenticate provider-specific finality for an existing
registered attempt. A signed receipt must correlate the full attempt and cover
its entire effect footprint; installing a verifier is a trusted host qualification
step. Receipt keys are dedicated to the finality source and are never supplied
through action metadata.

The original uncertain evidence remains immutable. A separate attestation records
the accepted resolution, and a coordinator CAS pins both references and the prior
unresolved state while releasing concurrency once. Completion observation can
therefore repair after expiry or cancellation without granting a fresh provider
start. A no-effect resolution remains terminal for the original operation.

See [Durable Executions](durable-executions.md#reconciliation-from-independent-finality-receipts)
for the Rust host interface and recovery behavior. Authenticated management
adapters and independently permitted remote probes remain separate integration
work; the platform does not accept an arbitrary operator assertion as finality.
