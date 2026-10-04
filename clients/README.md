# Acteon Client Libraries

Official client libraries for Acteon, the execution and governance platform for agents and services.

## Available Clients

| Language | Directory | Package |
|----------|-----------|---------|
| [Rust](../crates/client/README.md) | `crates/client/` | `acteon-client` |
| [Python](python/README.md) | `clients/python/` | `acteon-client` |
| [Node.js/TypeScript](nodejs/README.md) | `clients/nodejs/` | `@acteon/client` |
| [Go](go/README.md) | `clients/go/` | `github.com/penserai/acteon/clients/go/acteon` |
| [Java](java/README.md) | `clients/java/` | `com.acteon:acteon-client` |

## API Consistency

All five clients provide typed helpers for common operations and a generated platform operation API for every finite HTTP route. Higher-level runtime support differs by language; see [SDK coverage and wire contracts](https://penserai.github.io/acteon/api/sdk-coverage/) for the exact boundary.

The table below lists common helpers, not the complete API:

### Methods

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

### Action Structure

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | No | Auto-generated UUID |
| `namespace` | string | Yes | Logical grouping |
| `tenant` | string | Yes | Tenant identifier |
| `provider` | string | Yes | Target provider |
| `action_type` | string | Yes | Type of action |
| `payload` | object | Yes | Action-specific data |
| `dedup_key` | string | No | Deduplication key |
| `metadata` | object | No | Labels for filtering |

### Outcome Types

| Type | Description |
|------|-------------|
| `executed` | Action was executed by the provider |
| `deduplicated` | Action was already processed (duplicate) |
| `suppressed` | Action was blocked by a rule |
| `rerouted` | Action was sent to a different provider |
| `throttled` | Action was rate-limited, retry later |
| `failed` | Action failed after retries |

### Error Types

| Error | Retryable | Description |
|-------|-----------|-------------|
| Connection | Yes | Network failure, timeout |
| HTTP 5xx | Yes | Server error |
| HTTP 4xx | No | Client error |
| API (depends) | Varies | Server-reported error |

## Quick Examples

### Python

```python
from acteon_client import ActeonClient, Action

client = ActeonClient("http://localhost:8080")
action = Action("ns", "tenant", "email", "send", {"to": "user@example.com"})
outcome = client.dispatch(action)
```

### Node.js/TypeScript

```typescript
import { ActeonClient, createAction } from "@acteon/client";

const client = new ActeonClient("http://localhost:8080");
const action = createAction("ns", "tenant", "email", "send", { to: "user@example.com" });
const outcome = await client.dispatch(action);
```

### Go

```go
client := acteon.NewClient("http://localhost:8080")
action := acteon.NewAction("ns", "tenant", "email", "send", map[string]any{"to": "user@example.com"})
outcome, err := client.Dispatch(ctx, action)
```

### Java

```java
ActeonClient client = new ActeonClient("http://localhost:8080");
Action action = new Action("ns", "tenant", "email", "send", Map.of("to", "user@example.com"));
ActionOutcome outcome = client.dispatch(action);
```

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
