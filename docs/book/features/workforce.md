# Agent workforce

Acteon organizes humans, personal agents, team agents and deterministic services
into a workforce with explicit representation mandates. Participants keep their
own authenticated identities. A job records who acts, who initiated it, and the
person or team it represents.

A Reliability team might include Maya, her personal assistant, a standing incident
investigator and an incident scheduler. Removing Maya's membership blocks her
assistant's membership-dependent team work. The independently mandated investigator
and scheduler can continue their standing duties. A resource closure still applies
to all of them.

## Relationships and authority

| Record | Purpose | Execution rights |
|---|---|---|
| Team | Organizational identity and name | None |
| Human membership | Direct team relationship and roles | None on its own |
| Agent ownership | Human or team stewardship | None |
| Duty assignment | Agent availability for exact job classes | None |
| Representation mandate | Actor, represented party, eligible initiators and explicit dependencies | Bounds representation; requires a permit |
| Represented permit | Concrete actor, qualified effects and root limits, bound to a mandate revision | Evaluated with current credentials, mandate, relationships and resource controls |

A team is not a shared login or a tenant. `TeamRef` identifies it by `domain`,
`tenant` and `id`; every human, agent and service retains a separate principal.
A human can belong to several teams. A job selects one explicit representation
lineage, so memberships from different teams cannot be combined to authorize it.

Personal agents representing a team require their owner's current requester
membership and their own duty assignment as explicit mandate dependencies.
Standing team mandates need no implicit membership dependency on their creator.
For agent actors, mandates pin the ownership revision. Transferring ownership
invalidates jobs depending on the old revision; it does not retarget them to the
new owner.

Membership roles are `requester`, `approver`, `workforce_manager` and
`mandate_issuer`. They describe the relationship and can be checked by job
adapters. They do not automatically grant API management, approval execution or
tools. Public management also requires independent deployment authorization.

## Enable workforce management

Add an explicit `workforce` policy to an existing execution manager. Ordinary
permit issuance or intervention rights do not imply workforce management.

```toml
[[execution_authority.scopes.managers]]
principal = {id="operator",kind="human"}
subjects = [{id="maya",kind="human"},{id="agent/maya",kind="agent"}]
routes = [{provider="incident",action_type="execute"}]
valid_from_ms = 0
limits = {max_units=5,max_concurrent=2,deadline_ms=4102444800000}
can_issue_permits = true
can_intervene = true
workforce = {teams=[{domain="prod",tenant="acme",id="reliability"}],job_classes=["execute"],can_manage_roster=true,can_issue_mandates=true}
```

The manager and managed principals must appear in the enclosing scope's
`subjects`; its routes must already be independently declared and qualified.
Teams must belong to the scope's tenant. Job classes are exact allowed action
types from the manager's registered routes in this provider execution profile.
Authentication must also grant scope management access, and current roles and
configuration authority remain enforced at the write boundary.

`can_manage_roster` permits bounded team, membership, ownership and assignment
changes. `can_issue_mandates` permits bounded mandate issuance and revocation.
`can_issue_permits` separately permits represented permit issuance. The server
checks these capabilities, allowed teams, typed principals, job classes,
qualified effects, validity and root limits. A roster-only manager can use empty
route and job-class lists when it does not assign duties or issue authority.

Workforce records, admission records and signed contexts use the configured
`StateStore`. Redis is optional. Memory supports a local demonstration;
PostgreSQL supports durable multi-replica operation and restart recovery.
Root admission intersects the deployment ceiling with every selected permit and
the required mandate. A tighter grant reduces the admitted budget and deadline;
it does not require raising the grant to the deployment maximum. Retries retain
the first admission's context and budget.

Existing protocol-8 authority records require the explicit reviewed
[protocol cutover](../api/authentication.md#upgrade-an-existing-authority-scope).

## API

`GET /v1/workforce?namespace=prod&tenant=acme` returns your current management
bounds, qualified routes and visible workforce records. Each record contains
`value` and `revoked`. Inspection is filtered to your independent bounds.

`POST /v1/workforce/changes` records a reasoned mutation:

```json
{
  "namespace": "prod",
  "tenant": "acme",
  "change_id": "enroll-reliability-1",
  "reason": "Establish the incident response workforce",
  "change": {
    "kind": "put_team",
    "team": {
      "team": {"domain": "prod", "tenant": "acme", "id": "reliability"},
      "revision": 1,
      "name": "Reliability"
    }
  }
}
```

| Change kind | Payload |
|---|---|
| `put_team` | `team`: team reference, revision, name |
| `disband_team` | `team`: team reference; `expected_revision` |
| `put_membership` | `membership`: ID, revision, team, human, roles, validity interval |
| `remove_membership` | `id`, `expected_revision` |
| `put_ownership` | `ownership`: agent, revision, human/team owner |
| `put_assignment` | `assignment`: ID, revision, team, agent, job classes, validity interval |
| `remove_assignment` | `id`, `expected_revision` |
| `put_mandate` | `mandate`: representation declaration below |
| `revoke_mandate` | `id`, `expected_revision` |
| `publish_represented_permit` | `permit`: ordinary permit declaration; `mandate`: accepted reference |

New records start at revision 1; updates advance by one revision. Removals,
disbanding and revocations are terminal for that record ID. Re-enrollment uses
a new relationship or mandate ID. Ownership is keyed by the actual agent
principal and advances its revision on transfer.

A personal agent's team mandate names its dependencies explicitly:

```json
{
  "kind": "put_mandate",
  "mandate": {
    "id": "maya-reliability-duty",
    "revision": 1,
    "represented": {
      "kind": "team",
      "team": {"domain": "prod", "tenant": "acme", "id": "reliability"}
    },
    "actor": {"id": "agent/maya", "kind": "agent"},
    "job_class": "execute",
    "eligible_initiators": [{"id": "agent/maya", "kind": "agent"}],
    "ownership": {"id": "agent/maya", "accepted_revision": 1},
    "dependencies": [
      {"kind": "membership", "reference": {"id": "maya-reliability", "accepted_revision": 1}},
      {"kind": "assignment", "reference": {"id": "maya-incident-duty", "accepted_revision": 1}}
    ],
    "routes": [{"provider": "incident", "action_type": "execute"}],
    "valid_from_ms": 0,
    "limits": {"max_units": 5, "max_concurrent": 1, "deadline_ms": 4102444800000}
  }
}
```

Create the team, membership, ownership and assignment first. Mandates and
represented permits name routes; the server resolves their actual effect
footprints from registered providers. Raw effect claims cannot supply authority.

`publish_represented_permit` accepts the same `permit` declaration as
[governance issuance](governance.md), plus
`"mandate": {"id": "maya-reliability-duty", "accepted_revision": 1}`.
The permit and its binding are committed together. The ordinary permit endpoint
cannot replace a represented permit with an unbound revision.

A receipt identifies the actual manager, reason, committed generation and change
ID. Preserve the exact request and ID when recovering a lost acknowledgment.
Replay observes the original change after checking current management authority;
it does not restore a subsequently removed relationship or revoked mandate.
The SDKs do not automatically retry mutations. HTTP 400 rejects invalid input,
401 requires authentication, 403 refuses authority, 409 reports an authority or
revision conflict, and 503 indicates unavailable governance.

## Dispatch and intervention

Dispatch uses the existing execution-permit header or native SDK `permits`
argument. The server resolves the required mandate from the selected permit
history, establishes the initiator from private authentication, and takes the
job class from the actual prepared provider route's action type. Request-body
`initiator`, `represented`, `job_class` or `mandate` fields cannot replace those
facts.

Accepted signed contexts pin the original actor, initiator, represented party,
mandate and relationship revisions. Retries retain that original context and
budget. Every new effect checks current eligibility. Removing a required
membership or assignment, changing ownership, disbanding a represented team,
revoking a mandate, or revoking an explicitly required human blocks dependent
new work. Independent standing work keeps its own authority.

A refused provider admission returns a `Failed` action outcome with code
`EXECUTION_ADMISSION_REFUSED`, `retryable=false`, and no outbound call. The
HTTP dispatch response can still be 200 because it carries an action outcome;
check the outcome as well as the transport status.

Resource closures and credential or permit revocations continue to use the
[governance management API](governance.md). They apply alongside workforce checks.
Already started external effects may finish; removing authority does not undo them.

## SDKs and operator UI

| SDK | Inspect | Change |
|---|---|---|
| Rust | `workforce` | `change_workforce` |
| Python, sync/async | `workforce` | `change_workforce` |
| TypeScript | `workforce` | `changeWorkforce` |
| Go | `Workforce` | `ChangeWorkforce` |
| Java | `workforce` | `changeWorkforce` |

All five SDKs provide typed models for every change and inspection record.
The **Workforce** page lets authorized operators inspect the roster, enroll and
update relationships, issue mandates and represented permits, and remove or
revoke authority with a reason. Manual retries preserve the exact reviewed
request and change ID.

## Execution profile

The current profile covers qualified immediate provider execution within one
coordinator namespace and tenant. Workforce records in separate execution scopes
are independently governed: sharing a team reference does not make offboarding
atomic across them. A registry entry does not itself issue a mandate or permit.
Autonomous registry-backed A2A handoff, delegated initiators, shared team or
child budgets, protected deferred execution and cross-scope workforce coordination
remain separate platform work. This profile does not claim those guarantees.

Run the [workforce simulation](../guides/agent-workforce.md) to see personal
offboarding, independent standing mandates and resource closures against a real
HTTP receiver.
