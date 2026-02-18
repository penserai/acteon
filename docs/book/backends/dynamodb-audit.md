# DynamoDB Audit Backend

The DynamoDB audit backend stores audit records in a dedicated DynamoDB table with Global Secondary Indexes for efficient querying and native TTL for automatic record expiration.

<span class="badge production">Production</span> for AWS-native deployments

## When to Use

- AWS-native infrastructure where you want a fully managed audit store
- SOC2/HIPAA compliance — DynamoDB conditional writes provide hash chain CAS support
- Serverless or auto-scaling architectures
- When you want native TTL-based record expiration without background cleanup

## Configuration

```toml title="acteon.toml"
[audit]
enabled = true
backend = "dynamodb"
url = "http://localhost:8000"      # DynamoDB Local for development (omit for AWS)
region = "us-east-1"
table_name = "acteon_audit"
prefix = "acteon_"
ttl_seconds = 2592000              # 30 days (controls expires_at_ttl attribute)
store_payload = true
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `region` | string | `us-east-1` | AWS region |
| `table_name` | string | `acteon_audit` | DynamoDB table name |
| `url` | string | — | DynamoDB endpoint URL. Set for local development; omit to use AWS default endpoint. |
| `prefix` | string | `acteon_` | Key prefix for partitioning |
| `ttl_seconds` | integer | 2592000 | Record TTL in seconds (sets `expires_at_ttl` attribute) |

## Table Schema

The audit table uses `id` (String) as the partition key with three Global Secondary Indexes:

| GSI Name | Partition Key | Sort Key | Purpose |
|----------|--------------|----------|---------|
| `ns_tenant_dispatched` | `ns_tenant` (S) | `dispatched_at_ms` (N) | Query by namespace+tenant sorted by time |
| `ns_tenant_sequence` | `ns_tenant` (S) | `sequence_number` (N) | Hash chain tip lookups and ascending sequence queries |
| `action_id_index` | `action_id` (S) | `dispatched_at_ms` (N) | Lookup by action ID |

The `ns_tenant` attribute is a composite key in the format `"{namespace}#{tenant}"`.

All GSIs use `ALL` projection for flexibility.

## Hash Chain Support

DynamoDB supports hash chain integrity for [SOC2/HIPAA compliance mode](../features/compliance-mode.md). When `hash_chain = true`:

1. Each audit record write uses `TransactWriteItems` with two items:
   - A **fence item** (`PK = SEQ#{namespace}#{tenant}#{sequence_number}`) with `attribute_not_exists(id)` condition
   - The **audit record** itself
2. If two replicas race for the same sequence number, one transaction fails with `ConditionalCheckFailedException`
3. The losing replica retries with jittered exponential backoff, re-fetching the chain tip

This provides equivalent atomicity to PostgreSQL's UNIQUE constraint approach.

## TTL / Record Expiration

DynamoDB native TTL is enabled on the `expires_at_ttl` attribute (epoch seconds). DynamoDB automatically deletes expired items in the background — typically within 48 hours of expiration.

The `cleanup_expired()` method returns `Ok(0)` as a no-op since DynamoDB handles cleanup natively.

## Characteristics

| Property | Value |
|----------|-------|
| **Persistence** | Strongly consistent |
| **Query Model** | GSI-based with filter expressions |
| **Hash Chain** | Full support (conditional writes) |
| **TTL** | Native DynamoDB TTL |
| **Billing** | Pay-per-request (on-demand) |
| **Feature Flag** | `dynamodb` |

## AWS Configuration

For production AWS deployments, configure credentials via standard AWS methods:

```bash
# Environment variables
export AWS_ACCESS_KEY_ID=your-key
export AWS_SECRET_ACCESS_KEY=your-secret
export AWS_DEFAULT_REGION=us-east-1

# Or use AWS profiles
export AWS_PROFILE=production
```

## Docker Setup (DynamoDB Local)

```bash
# Start DynamoDB Local
docker compose --profile dynamodb up -d

# Or manually
docker run -d --name acteon-dynamodb -p 8000:8000 \
  amazon/dynamodb-local:latest
```

When `url` is set in the audit config (indicating local development), the server automatically creates the audit table and GSIs on startup.

## Building with DynamoDB Support

```bash
cargo build -p acteon-server --features dynamodb
```
