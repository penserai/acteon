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
providers and unsupported private contexts refuse execution. Deferred workflow,
approval and child execution still need explicit retained-authority integration;
they must not inherit a request's permit implicitly. The `durable=true` receipt
API is currently rejected in this profile even though provider execution itself
uses durable records.

The profile currently rejects guardrail, embedding and enrichment configurations
because their auxiliary calls are not qualified by this runtime. Complete mesh,
team mandates and auxiliary-effect governance remain separate implementation
work; enabling this profile does not establish those capabilities.
