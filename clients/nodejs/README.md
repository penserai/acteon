# @acteon/client (Node.js/TypeScript)

Node.js/TypeScript client for the Acteon action gateway.

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
