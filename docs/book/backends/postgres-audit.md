# PostgreSQL Audit Backend

The PostgreSQL audit backend provides ACID-guaranteed audit trail storage with indexed queries and automatic TTL cleanup.

<span class="badge recommended">Recommended</span> for production

## Configuration

```toml title="acteon.toml"
[audit]
enabled = true
backend = "postgres"
url = "postgres://acteon:acteon@localhost:5432/acteon"
prefix = "acteon_"
ttl_seconds = 2592000
cleanup_interval_seconds = 3600
store_payload = true
```

## Characteristics

| Property | Value |
|----------|-------|
| **Throughput** | ~28,000 records/sec (async writes) |
| **Consistency** | ACID |
| **Retention** | TTL-based with background cleanup |
| **Query** | SQL with indexed columns |
| **Feature Flag** | `postgres` |

## Schema

The PostgreSQL backend automatically creates a table for audit records with indexes on commonly queried columns:

- `action_id`
- `namespace` + `tenant`
- `outcome`
- `dispatched_at`
- `expires_at`

## Queries

The backend supports all `AuditQuery` filter parameters:

```bash
# Filter by tenant and outcome
curl "http://localhost:8080/v1/audit?tenant=tenant-1&outcome=executed"

# Date range query
curl "http://localhost:8080/v1/audit?from=2026-01-01T00:00:00Z&to=2026-01-31T23:59:59Z"

# Pagination
curl "http://localhost:8080/v1/audit?limit=100&offset=200"
```

## Setup

```bash
docker compose --profile postgres up -d
cargo run -p acteon-server --features postgres -- -c examples/postgres.toml
```
