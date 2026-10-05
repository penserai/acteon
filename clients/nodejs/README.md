# @acteon/client (Node.js/TypeScript)

Node.js/TypeScript client for the Acteon action gateway.

## Complete platform API

The generated operation catalog exposes all 187 finite HTTP operations, including receipt sessions, managed stages, workflows, execution controls, inference profiles, and stream windows. Use an authenticated client and a configured, existing stage for this example:

```typescript
const status = await client.platformRequest("bus_stages_status", {
  path: { namespace: "observability", tenant: "demo", id: "log-detector" },
});
```

Path parameters are escaped individually. Query and body fields use the server's wire names; JSON response envelopes are preserved. Calls do not retry automatically. Keep request IDs stable across retries and return opaque receipt IDs unchanged. HTTP access does not imply a code-defined workflow runner in every language.

See [SDK coverage and wire contracts](https://penserai.github.io/acteon/api/sdk-coverage/) for return types, native streaming APIs, runtime availability, and server feature requirements. The catalog is generated from the registered server routes and checked in CI.

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
