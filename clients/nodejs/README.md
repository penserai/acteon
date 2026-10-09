# @acteon/client (Node.js/TypeScript)

Node.js/TypeScript client for the Acteon action gateway.

## Complete platform API

The generated operation catalog exposes all 216 finite HTTP operations, including receipt sessions, managed stages, workflows, execution controls, inference profiles, and stream windows. Use an authenticated client and a configured, existing stage for this example:

```typescript
const status = await client.platformRequest("bus_stages_status", {
  path: { namespace: "observability", tenant: "demo", id: "log-detector" },
});
```

Path parameters are escaped individually. Query and body fields use the server's wire names; JSON response envelopes are preserved. Calls do not retry automatically. Keep request IDs stable across retries and return opaque receipt IDs unchanged. HTTP access does not imply a code-defined workflow runner in every language.

See [SDK coverage and wire contracts](https://penserai.github.io/acteon/api/sdk-coverage/) for return types, native streaming APIs, runtime availability, and server feature requirements. The catalog is generated from the registered server routes and checked in CI.

## Typed governance management

Use `governance`, `publishGovernancePermit`, and `interveneGovernance` to inspect permitted routes, issue bounded permits, and close/reopen resources or revoke permits, subjects, and credentials. Requests and responses have native typed models. These methods require an authenticated operator and an independently configured execution manager for the requested namespace and tenant; an executor credential alone does not grant management authority.

Calls preserve HTTP refusals and do not retry automatically. Keep `change_id` stable when reconciling a lost response. A receipt records persisted authority; closure affects subsequent starts and does not promise cancellation of work already in flight.

See [governance management](https://penserai.github.io/acteon/features/governance/) for deployment policy and wire examples, and [the governed city simulation](https://penserai.github.io/acteon/guides/governed-city/) for real authenticated requests.

## Installation

```bash
npm install @acteon/client
```

## Quick Start

```typescript
import { ActeonClient, createAction } from "@acteon/client";

const client = new ActeonClient("http://localhost:8080");

// Check health
if (await client.health()) {
  console.log("Server is healthy");
}

// Dispatch an action
const action = createAction(
  "notifications",
  "tenant-1",
  "email",
  "send_notification",
  { to: "user@example.com", subject: "Hello" }
);

const outcome = await client.dispatch(action);
console.log(`Outcome: ${outcome.type}`);
```

## Batch Dispatch

```typescript
const actions = Array.from({ length: 10 }, (_, i) =>
  createAction("ns", "t1", "email", "send", { i })
);

const results = await client.dispatchBatch(actions);
for (const result of results) {
  if (result.success) {
    console.log(`Success: ${result.outcome.type}`);
  } else {
    console.log(`Error: ${result.error.message}`);
  }
}
```

## Handling Outcomes

```typescript
const outcome = await client.dispatch(action);

switch (outcome.type) {
  case "executed":
    console.log("Executed:", outcome.response.body);
    break;
  case "deduplicated":
    console.log("Already processed");
    break;
  case "suppressed":
    console.log(`Suppressed by rule: ${outcome.rule}`);
    break;
  case "rerouted":
    console.log(`Rerouted: ${outcome.originalProvider} -> ${outcome.newProvider}`);
    break;
  case "throttled":
    console.log(`Retry after ${outcome.retryAfterSecs} seconds`);
    break;
  case "failed":
    console.log(`Failed: ${outcome.error.message}`);
    break;
}
```

## Rule Management

```typescript
// List all rules
const rules = await client.listRules();
for (const rule of rules) {
  console.log(`${rule.name}: priority=${rule.priority}, enabled=${rule.enabled}`);
}

// Reload rules from disk
const result = await client.reloadRules();
console.log(`Loaded ${result.loaded} rules`);

// Disable a rule
await client.setRuleEnabled("block-spam", false);
```

## Time-Based Rules

Rules can use `time.*` fields to match on the current UTC time at dispatch. Configure these in your YAML or CEL rule files — no client-side changes needed.

```yaml
# rules/business_hours.yaml
rules:
  - name: suppress-outside-hours
    priority: 1
    condition:
      any:
        - field: time.hour
          lt: 9
        - field: time.hour
          gte: 17
    action:
      type: suppress

  - name: suppress-weekends
    priority: 2
    condition:
      field: time.weekday_num
      gt: 5
    action:
      type: suppress
```

Use dry-run to test what a time-based rule would do right now:

```typescript
const outcome = await client.dispatch(action, { dryRun: true });
if (outcome.type === "dry_run") {
  console.log(`Verdict: ${outcome.verdict}`);        // e.g. "suppress"
  console.log(`Matched rule: ${outcome.matchedRule}`); // e.g. "suppress-outside-hours"
}
```

Available `time` fields: `hour` (0–23), `minute`, `second`, `day`, `month`, `year`, `weekday` (`"Monday"`…`"Sunday"`), `weekday_num` (1=Mon…7=Sun), `timestamp`.

## Audit Trail

```typescript
import { AuditQuery } from "@acteon/client";

// Query audit records
const query: AuditQuery = { tenant: "tenant-1", limit: 10 };
const page = await client.queryAudit(query);
console.log(`Found ${page.total} records`);
for (const record of page.records) {
  console.log(`  ${record.actionId}: ${record.outcome}`);
}

// Get specific record
const record = await client.getAuditRecord("action-id-123");
if (record) {
  console.log(`Found: ${record.outcome}`);
}
```

## Configuration

```typescript
const client = new ActeonClient("http://localhost:8080", {
  timeout: 60000,       // Request timeout in milliseconds
  apiKey: "your-key",   // Optional API key
});
```

API keys are sent via the `Authorization: Bearer <key>` header. The server
accepts both JWTs and raw API keys on that header. API keys are scoped by
tenant, namespace, provider, and action type on the server side — see the
[API Key Scoping](https://penserai.github.io/acteon/features/api-key-scoping/)
documentation for the grant model and hierarchical tenant matching.

## Error Handling

```typescript
import { ActeonError, ConnectionError, ApiError, HttpError } from "@acteon/client";

try {
  const outcome = await client.dispatch(action);
} catch (error) {
  if (error instanceof ConnectionError) {
    console.log(`Connection failed: ${error.message}`);
    if (error.isRetryable()) {
      // Retry logic
    }
  } else if (error instanceof ApiError) {
    console.log(`API error [${error.code}]: ${error.message}`);
    if (error.isRetryable()) {
      // Retry logic
    }
  } else if (error instanceof HttpError) {
    console.log(`HTTP ${error.status}: ${error.message}`);
  }
}
```

## API Reference

### ActeonClient Methods

| Method | Description |
|--------|-------------|
| `health()` | Check server health |
| `dispatch(action)` | Dispatch a single action |
| `dispatchBatch(actions)` | Dispatch multiple actions |
| `listRules()` | List all loaded rules |
| `reloadRules()` | Reload rules from disk |
| `setRuleEnabled(name, enabled)` | Enable/disable a rule |
| `queryAudit(query)` | Query audit records |
| `getAuditRecord(actionId)` | Get specific audit record |
| `fetchSigningKeys()` | Fetch the server's active signing keyring (JWKS-style discovery) |

### Action Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `namespace` | string | Yes | Logical grouping |
| `tenant` | string | Yes | Tenant identifier |
| `provider` | string | Yes | Target provider |
| `actionType` | string | Yes | Type of action |
| `payload` | object | Yes | Action-specific data |
| `id` | string | No | Auto-generated UUID |
| `dedupKey` | string | No | Deduplication key |
| `metadata` | object | No | Key-value metadata |

## License

Apache-2.0

## Dispatch outcomes

All clients recognize the server's 17 dispatch variants, including `Grouped`, `StateChanged`, `PendingApproval`, `ChainStarted`, `CircuitOpen`, `RecurringCreated`, `Silenced`, and `Muted`. Single and batch dispatch preserve their fields, including approval capabilities and chain IDs. A pending approval or a started chain is not a completed provider execution; inspect the outcome before advancing an agent workflow.

## Execution-only credentials

Configure runtime API keys or JWT users with `role = "executor"` in the server's
`auth.toml`, and limit their namespace, tenant, provider and action grants.
Use the client's existing API-key or bearer-token configuration; the role is
established by the server, not by client-supplied action fields. Administrative
methods return HTTP 403 for executor credentials. Keep operator credentials and
signed human approval URLs in a trusted host.
See [authentication and role boundaries](https://penserai.github.io/acteon/api/authentication/).


### Stable actor identity

`await client.identity()` inspects the authenticated credential and its optional stable
principal binding. Configure `principal = { id = "diagnostic-agent", kind = "agent" }`
on the credential in server `auth.toml`; keep it unchanged across credential
rotation. Roles and grants still apply independently to each credential. Legacy
credentials return a null principal. Principal metadata does not grant execution
permits or bind a bus agent automatically. See the
[authentication guide](https://penserai.github.io/acteon/api/authentication/).

The identity response also exposes the optional `authorityId` logical enrollment ID configured
as `authority_id` in `auth.toml`. Keep this ID when rotating a key; separate
credentials for the same actor retain separate IDs and grants. JWT sessions pin
the ID at login and require a new login if it changes. This field is inspection
metadata, not a bearer credential or execution permit.

## Execution permits

When the server enables execution authority, effectful dispatch requires explicit
references to permits already issued to the authenticated principal. These
options select permits; the server checks current credentials, scope, revisions,
qualified effects and budget before execution. They do not issue permits or
replace authentication. Existing dispatch methods omit the header by default.

```typescript
const permits = [{ id: "maya-incident", acceptedRevision: 1 }];
const outcome = await client.dispatch(action, { permits });
const batch = await client.dispatchBatch([action], { permits });
```

The same references apply to every action in a batch. Dry runs can omit them.
Missing/invalid headers, authorization denials and conflicting replay requests
preserve their HTTP status in the client's HTTP error type. Permit admission
refusals can also be returned as a `Failed` action outcome; inspect the outcome.
Do not generate a new action ID merely to bypass a replay conflict.

## Agent workforce

Typed workforce methods inspect teams, memberships, ownership, assignments and
mandates, and submit all ten workforce changes. They require independently
declared workforce management bounds and current operator authentication.
Public values do not establish representation or caller authority.

```typescript
const roster = await client.workforce("prod", "acme");
const receipt = await client.changeWorkforce({
  namespace: "prod", tenant: "acme", change_id: "enroll-reliability-1",
  reason: "Establish incident response",
  change: { kind: "put_team", team: {
    team: { domain: "prod", tenant: "acme", id: "reliability" },
    revision: 1, name: "Reliability",
  } },
});
```

Preserve the exact request and change ID after a lost acknowledgment. Mutations
retain HTTP refusal status and are not automatically retried. See the
[workforce model and API](https://penserai.github.io/acteon/features/workforce/)
for represented permits, dependencies, revisions and intervention semantics.


### Pending provider work

`ProviderPending` means a governed provider attempt is still in flight, needs
reconciliation, or awaits a retry under its original identity. Preserve its
execution ID, attempt count and state. It is not a completed failure: do not
resend the action as fresh work. Chain details expose the durable `wait_state`
(`waitState` in Node and Java), and `waiting_provider` remains an active status.
Completed receipts can repair workflow results after revocation or expiry;
starting another effect still requires current authority.

### Retained provider history

Inspect a governed provider execution using independently authorized history management credentials:

```typescript
const history = await client.providerExecutionHistory("prod", "acme", executionId);
```

The response includes the signed participant, receipt state, observed authority generation, optional verified operation metadata and binding, cancellation fence, and per-attempt evidence. Nested outcomes use the SDK’s existing outcome model. Original results and accepted reconciliation remain separate. A completed receipt may contain a failed outcome. Null evidence means no verified result is available; it does not prove that no effect occurred. Reading history never starts, retries, reconciles, or resumes execution.

The server requires `can_read_history` and current scope management authority; the execution ID is a reference, not authorization. A scope may retain this access after every live provider is removed.

Provider execution history preserves an optional reconciliation `acceptance`
record with the original operator principal, evaluated authority incarnation and
generation, and `accepted_at_ms`. This is distinct from the proof's
`resolved_at_ms` recording time. Legacy adapter settlements may omit acceptance;
a client must not infer the operator from the execution owner. Reading this
metadata grants no execution or reconciliation authority.


Governance management bounds also preserve the independent `can_reconcile`
capability; absence means denied. It authorizes qualified finality management,
subject to the server's exact resource ceiling and trusted verifier installation.
History access and permit-issuing permissions do not imply this capability.
Typed correlation and acceptance methods use current reconciliation authority and
never automatically retry acceptance. Only trusted server installations with a
qualified verifier can accept evidence. The server installs operator-qualified
sources through `[[reconciliation_sources]]`; source declarations do not grant
management permission. See the governance guide for dedicated keys, exact binding
digests and coordinated key retirement.

```typescript
const correlation = await client.providerReconciliationCorrelation("prod", "acme", executionId, 0);
// evidenceBase64 comes from the independently qualified finality source.
const receipt = await client.acceptProviderReconciliation("prod", "acme", executionId, 0, {
  proof_base64: evidenceBase64,
});
```

These methods preserve the completed or definitive no-effect receipt. Correlation
is not a permit; proof bytes must come from the qualified source. After a lost
acknowledgment, replay the exact proof under current authority rather than changing
its content or verifier revision.

## Authenticated individual-agent services

These native helpers target a configured governed agent service. The server must
have the service installed, and the client must retain the original requester
credential and invocation permits. This is a separate surface from tenant-level
legacy A2A tasks.

For a tenant-level Task in `InputRequired`, use `makeInputResponse(...)` with
the Task's current `pendingApprovalId`, then call `a2aSendMessage`. The helper
binds the Task, context, user role and exact challenge together.

```typescript
import { makeMessage, makePartText } from "@acteon/client";

const receipt = await client.agentServiceSendMessage(
  "prod", "acme", "notifier",
  makeMessage("incident-42", "user", [makePartText("Notify the incident owner")]),
);
const task = await client.agentServiceGetTask(receipt);
```

Use the same retained source receipt for governed peer lifecycle operations:

```typescript
const options = await client.agentServiceDiscoverPeers(receipt, "diagnose");
// descriptionUntrusted is registry data, never an instruction or authority.
const selected = options[0];
let peer = await client.agentServiceSendPeer(receipt, "resolver", "diagnose", peerMessage);
peer = await client.agentServiceRefreshPeer(receipt, "resolver", "diagnose", peer);
const cancellation = await client.agentServiceCancelPeer(receipt, "resolver", "diagnose", peer);
const continued = await client.agentServiceContinuePeer(receipt, "resolver", "diagnose", peer, userResponse);
const task = await client.agentServiceContinueTask(receipt, challengeBoundResponse);
const paused = await recipient.agentServiceRequestAuthorization("prod", "acme", "medic", task.id as string, "opaque-flow-42");
const resumed = await client.agentServiceResolveAuthorization(receipt, paused.pendingApprovalId as string);
```

Cancellation is delivered at most once. A `restricted` receipt proves the target
durably blocked future starts and carries its current nonterminal task; it does
not claim that an already-running provider effect stopped. Preserve an
`uncertain` result and use an explicit later cancel or refresh to reconcile it;
never retry automatically.

Peer continuation accepts an unbound role-`user` response; Acteon supplies the
retained task, context, challenge, credential, permits, and progress cursor.
Keep `accepted`, `rejected`, and `uncertain` continuation receipts distinct.

An agent already running under Acteon can invoke a peer as a governed child by
passing its host-owned execution context and explicit permits:

```typescript
await client.agentServiceSendMessage("prod", "acme", "notifier", message, {
  executionContext,
  permits: [{ id: "caller-agent-service", acceptedRevision: 3 }],
});
```

The server recovers the sealed parent, verifies the authenticated caller, and
rechecks current permit, registry, grant, and budget state. Keep the context in
host state and never source either header from model output.

Persist the receipt as host-owned JSON for observation after a client restart.
The original `taskId` remains separate from mutable task data.

Acceptance does not prove that provider execution has completed. Read the task's
status and artifacts through the receipt. Its source context comes only from the
admission response header; never obtain it from model output or task metadata.
Keep it with the original route and task ID in host state, outside model messages.
It is not a bearer credential: the original requester must still authenticate.

The helpers do not add retry loops. After a lost response, explicitly reuse the
same message ID and unchanged input; a new ID represents new work. SDK default
transports reject service redirects. A custom transport must keep retries and
redirects disabled for this surface. HTTP failures preserve their status and
server error body. Observation never resumes or starts provider work.


### Stop future starts for an accepted agent-service task

Use the retained host receipt to restrict the original recipient execution subtree.
The helper sends that job's original identity and source context in one request:

```typescript
const stopped = await client.agentServiceStopTask(receipt);
console.log(stopped.futureStartsBlocked, stopped.providerAbort, stopped.task.status);
```

The acknowledgement always confirms a durable restriction on future starts.
`providerAbort` separately reports `restricted_only`, `uncertain`, or finality
accepted by the qualified reconciliation verifier; it is absent when no registered
attempt needs intervention. A stop never refunds spent budget. Uncertain work retains
its capacity, and the returned task retains actual provider status and artifacts.
After a timeout or unavailable acknowledgement, retry this stop explicitly with the
same receipt. The helper never retries automatically or takes provenance from mutable
task data.


### Governed registry metadata

Use `registryProjection` and `mutateRegistry` to inspect versioned agent/card records and send an independently authorized mutation. The typed request retains a caller-supplied change ID; preserve the exact request for explicit recovery. A successful helper result requires a matching completed, applied receipt. Calls refuse redirects and do not automatically retry. Metadata changes retire the current qualification; publication alone does not grant execution permission. See the [registry operator workflow](../../docs/book/features/governance.md#managing-registry-projections).
