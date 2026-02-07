# Action Replay

Action replay lets you reconstruct and re-dispatch actions from the audit trail.
This is invaluable for incident response and recovery scenarios:

- **Replay failed actions** after a provider outage is resolved
- **Re-execute suppressed actions** after fixing an overly aggressive rule
- **Reprocess actions** from a specific time window (e.g. DLQ drain timeframe)

## Prerequisites

Replay requires:

1. **Audit enabled** with `store_payload: true` in the server configuration
2. The audit record must contain the original action payload (records written
   with `store_payload: false` cannot be replayed)

## Single Action Replay

Replay a specific action by its audit record action ID:

```
POST /v1/audit/{action_id}/replay
```

The endpoint:

1. Looks up the audit record by `action_id`
2. Reconstructs the original `Action` from stored fields
3. Adds `replayed_from` metadata pointing to the original action ID
4. Dispatches through the full gateway pipeline with a new UUID

### Example

```bash
curl -X POST http://localhost:8080/v1/audit/550e8400-e29b-41d4-a716-446655440000/replay
```

```json
{
  "original_action_id": "550e8400-e29b-41d4-a716-446655440000",
  "new_action_id": "661f9511-f30c-52e5-b827-557766551111",
  "success": true,
  "error": null
}
```

### Error Cases

| Status | Reason |
|--------|--------|
| 404 | Audit record not found or audit not enabled |
| 422 | No stored payload (privacy mode was enabled) |
| 403 | Insufficient permissions or tenant/namespace not authorized |

## Bulk Replay

Replay multiple actions matching audit query filters:

```
POST /v1/audit/replay?tenant=acme&outcome=failed&limit=50
```

### Query Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `namespace` | string | Filter by namespace |
| `tenant` | string | Filter by tenant |
| `provider` | string | Filter by provider |
| `action_type` | string | Filter by action type |
| `outcome` | string | Filter by outcome (e.g. `failed`, `suppressed`) |
| `verdict` | string | Filter by verdict |
| `matched_rule` | string | Filter by matched rule name |
| `from` | string | Start of time range (RFC 3339) |
| `to` | string | End of time range (RFC 3339) |
| `limit` | integer | Max records to replay (default 50, max 1000) |

### Example: Replay all failed actions from the last hour

```bash
curl -X POST 'http://localhost:8080/v1/audit/replay?outcome=failed&from=2025-01-15T12:00:00Z&limit=100'
```

```json
{
  "replayed": 8,
  "failed": 1,
  "skipped": 2,
  "results": [
    {
      "original_action_id": "550e8400-...",
      "new_action_id": "771f0622-...",
      "success": true,
      "error": null
    }
  ]
}
```

Records are **skipped** when:
- The caller is not authorized for the record's tenant/namespace
- The record has no stored payload (privacy mode)

## Client SDK Usage

### Rust

```rust
use acteon_client::ActeonClient;

let client = ActeonClient::new("http://localhost:8080");

// Single replay
let result = client.replay_action("550e8400-...").await?;
println!("Replayed as: {}", result.new_action_id);

// Bulk replay
use acteon_client::ReplayQuery;
let query = ReplayQuery {
    tenant: Some("acme".into()),
    outcome: Some("failed".into()),
    limit: Some(50),
    ..Default::default()
};
let summary = client.replay_audit(&query).await?;
println!("Replayed: {}, Failed: {}", summary.replayed, summary.failed);
```

### Python

```python
from acteon_client import ActeonClient

client = ActeonClient("http://localhost:8080")

# Single replay
result = client.replay_action("550e8400-...")
print(f"Replayed as: {result.new_action_id}")

# Bulk replay
from acteon_client import ReplayQuery
query = ReplayQuery(tenant="acme", outcome="failed", limit=50)
summary = client.replay_audit(query)
print(f"Replayed: {summary.replayed}, Failed: {summary.failed}")
```

### Node.js / TypeScript

```typescript
// Single replay
const result = await client.replayAction("550e8400-...");
console.log(`Replayed as: ${result.newActionId}`);

// Bulk replay
const summary = await client.replayAudit({
  tenant: "acme",
  outcome: "failed",
  limit: 50,
});
console.log(`Replayed: ${summary.replayed}, Failed: ${summary.failed}`);
```

### Go

```go
// Single replay
result, err := client.ReplayAction(ctx, "550e8400-...")
fmt.Println("Replayed as:", result.NewActionID)

// Bulk replay
query := &acteon.ReplayQuery{
    Tenant:  stringPtr("acme"),
    Outcome: stringPtr("failed"),
    Limit:   intPtr(50),
}
summary, err := client.ReplayAudit(ctx, query)
fmt.Printf("Replayed: %d, Failed: %d\n", summary.Replayed, summary.Failed)
```

### Java

```java
// Single replay
ReplayResult result = client.replayAction("550e8400-...");
System.out.println("Replayed as: " + result.getNewActionId());

// Bulk replay
Map<String, String> params = Map.of(
    "tenant", "acme",
    "outcome", "failed",
    "limit", "50"
);
ReplaySummary summary = client.replayAudit(params);
System.out.printf("Replayed: %d, Failed: %d%n",
    summary.getReplayed(), summary.getFailed());
```

## Provenance Tracking

Every replayed action includes metadata that links back to the original:

```json
{
  "metadata": {
    "labels": {
      "replayed_from": "550e8400-e29b-41d4-a716-446655440000"
    }
  }
}
```

This creates an audit chain: you can always trace a replayed action back to
its source. The original metadata labels are also preserved in the replay.

## Authorization

Replay endpoints require:

1. **Dispatch permission** (admin or operator role)
2. **Tenant/namespace authorization** -- the caller must have grants covering
   the audit record's tenant, namespace, and action type

For bulk replay, unauthorized records are silently skipped (counted in the
`skipped` field of the response).
