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

## Coverage

These controls govern the qualified immediate execution profile. They do not
cancel already started external operations or supply compensation. Closure state
is per exact resource; independently overlapping named closures, drain/pause,
external cancellation acknowledgments and intervention reconciliation remain
separate work. Teams, representation mandates, shared delegated funding and
protected deferred execution are also separate platform capabilities.
