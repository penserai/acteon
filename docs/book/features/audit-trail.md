# Audit Trail

The audit trail provides a comprehensive, searchable record of every action dispatched through Acteon and its outcome. It supports configurable retention, payload storage, and field-level redaction.

## How It Works

Every action dispatch creates an `AuditRecord` containing:

- **Action metadata**: namespace, tenant, provider, action_type
- **Rule verdict**: which rule matched and why
- **Outcome**: executed, suppressed, deduplicated, throttled, failed, etc.
- **Timing**: dispatch time, completion time, duration in milliseconds
- **Optional payload**: the full action payload (if `store_payload` is enabled)
- **Caller info**: authentication method and caller ID

```mermaid
flowchart LR
    A[Action Dispatched] --> B[Gateway Processing]
    B --> C[Outcome Determined]
    C --> D[Create AuditRecord]
    D --> E[(Audit Store)]
    E --> F[Query / Search]
```

## Configuration

```toml title="acteon.toml"
[audit]
enabled = true
backend = "postgres"                 # "memory" | "postgres" | "clickhouse" | "dynamodb" | "elasticsearch"
url = "postgres://acteon:acteon@localhost:5432/acteon"
prefix = "acteon_"
ttl_seconds = 2592000                # 30 days
cleanup_interval_seconds = 3600      # Cleanup every hour
store_payload = true                 # Store action payloads
```

### Field Redaction

Automatically redact sensitive fields from stored payloads:

```toml
[audit.redact]
enabled = true
fields = ["password", "token", "api_key", "secret", "credit_card"]
placeholder = "[REDACTED]"
```

When redaction is enabled, any matching field names in the payload are replaced:

```json
// Before redaction
{"to": "user@example.com", "api_key": "sk-abc123", "body": "Hello"}

// After redaction
{"to": "user@example.com", "api_key": "[REDACTED]", "body": "Hello"}
```

## AuditRecord Structure

```rust
pub struct AuditRecord {
    pub id: String,                     // UUID
    pub action_id: String,
    pub chain_id: Option<String>,       // If part of a chain
    pub namespace: String,
    pub tenant: String,
    pub provider: String,
    pub action_type: String,
    pub verdict: String,                // "allow", "deny", etc.
    pub matched_rule: Option<String>,   // Rule that matched
    pub outcome: String,                // "executed", "suppressed", etc.
    pub action_payload: Option<Value>,  // Stored payload
    pub verdict_details: Value,         // Why the verdict was made
    pub outcome_details: Value,         // Outcome details
    pub metadata: Value,                // Additional metadata
    pub dispatched_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub expires_at: Option<DateTime<Utc>>,
    pub caller_id: Option<String>,
    pub auth_method: Option<String>,
}
```

## API Endpoints

### Query Audit Records

```bash
# All records
curl "http://localhost:8080/v1/audit"

# With filters
curl "http://localhost:8080/v1/audit?tenant=tenant-1&outcome=suppressed&limit=50"

# By date range
curl "http://localhost:8080/v1/audit?from=2026-01-01T00:00:00Z&to=2026-01-31T23:59:59Z"
```

### Query Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `namespace` | string | Filter by namespace |
| `tenant` | string | Filter by tenant |
| `provider` | string | Filter by provider |
| `action_type` | string | Filter by action type |
| `outcome` | string | Filter by outcome (executed, suppressed, etc.) |
| `verdict` | string | Filter by verdict |
| `matched_rule` | string | Filter by rule name |
| `caller_id` | string | Filter by caller |
| `chain_id` | string | Filter by chain ID |
| `from` | datetime | Start of date range |
| `to` | datetime | End of date range |
| `limit` | u32 | Max records (default 50, max 1000) |
| `offset` | u32 | Pagination offset |

### Get Record by Action ID

```bash
curl "http://localhost:8080/v1/audit/{action_id}"
```

### Response Format

```json
{
  "records": [
    {
      "id": "aud-abc123",
      "action_id": "act-def456",
      "namespace": "notifications",
      "tenant": "tenant-1",
      "provider": "email",
      "action_type": "send_email",
      "verdict": "allow",
      "outcome": "executed",
      "duration_ms": 45,
      "dispatched_at": "2026-01-15T10:00:00Z",
      "completed_at": "2026-01-15T10:00:00.045Z"
    }
  ],
  "total": 150,
  "limit": 50,
  "offset": 0
}
```

## Audit Backends

| Backend | Best For | Features |
|---------|----------|----------|
| **Memory** | Testing | Fast, no persistence |
| **PostgreSQL** | Production | ACID, indexed queries, TTL cleanup |
| **DynamoDB** | AWS-native | Managed, hash chain support, native TTL |
| **ClickHouse** | Analytics | Columnar storage, fast aggregations |
| **Elasticsearch** | Search | Full-text search, index lifecycle |

See [Audit Backends](../backends/audit-backends.md) for detailed backend comparison.

## Client SDK

=== "Rust"

    ```rust
    use acteon_client::{ActeonClient, AuditQuery};

    let client = ActeonClient::new("http://localhost:8080");

    // Query with filters
    let page = client.query_audit(&AuditQuery {
        tenant: Some("tenant-1".into()),
        outcome: Some("executed".into()),
        limit: Some(100),
        ..Default::default()
    }).await?;

    println!("Found {} records", page.total);

    // Get specific record
    if let Some(record) = client.get_audit_record("action-id").await? {
        println!("Outcome: {}", record.outcome);
    }
    ```

## Automatic Cleanup

Audit records expire based on `ttl_seconds`. The background cleanup worker runs every `cleanup_interval_seconds` and removes expired records:

```toml
[audit]
ttl_seconds = 2592000              # 30 days
cleanup_interval_seconds = 3600    # Check every hour
```

!!! note "Elasticsearch"
    The Elasticsearch backend doesn't use TTL-based cleanup. Instead, use Elasticsearch's built-in [Index Lifecycle Management (ILM)](https://www.elastic.co/guide/en/elasticsearch/reference/current/index-lifecycle-management.html) for retention policies.

## Related Features

- **[Compliance Mode](compliance-mode.md)**: Adds SHA-256 hash chaining (`record_hash`, `previous_hash`, `sequence_number` fields), synchronous audit writes, and optional record immutability for SOC2/HIPAA requirements.
- **[Data Retention](data-retention.md)**: Per-tenant audit TTL resolution and background reaper for automatic cleanup.
- **[Payload Encryption](payload-encryption.md)**: Encrypts payloads at rest; hash chaining operates on the ciphertext.
