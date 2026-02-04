# Acteon Client Libraries

Official client libraries for the Acteon action gateway.

## Available Clients

| Language | Directory | Package |
|----------|-----------|---------|
| [Rust](../acteon-client/README.md) | `acteon-client/` | `acteon-client` |
| [Python](python/README.md) | `clients/python/` | `acteon-client` |
| [Node.js/TypeScript](nodejs/README.md) | `clients/nodejs/` | `@acteon/client` |
| [Go](go/README.md) | `clients/go/` | `github.com/penserai/acteon/clients/go/acteon` |
| [Java](java/README.md) | `clients/java/` | `com.acteon:acteon-client` |

## API Consistency

All clients provide the same API surface:

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
