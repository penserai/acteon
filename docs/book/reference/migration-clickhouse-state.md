# Migration: ClickHouse State Backend Removal

As of Acteon v0.1.0, the **ClickHouse state backend** has been removed. 

## Rationale

State backends in Acteon are responsible for distributed locking, deduplication, and state machine transitions. These operations require **strong consistency** and **atomic compare-and-swap (CAS)** operations to guarantee correctness in multi-replica deployments.

While ClickHouse is an excellent columnar database for analytics (and remains fully supported as an **audit backend**), its asynchronous mutation model and eventual consistency characteristics are not suitable for high-frequency distributed state management. 

To ensure system integrity and prevent race conditions, Acteon now only supports state backends that provide strong consistency guarantees.

## Impact

*   The `acteon-state-clickhouse` crate has been deleted.
*   The `clickhouse` value for `[state] backend` in `acteon.toml` is no longer supported.
*   The `clickhouse` feature flag in `acteon-server` now only enables the **audit** backend.

## Migration Steps

If you are currently using ClickHouse for state storage, you must migrate to one of the following supported backends:

| Backend | Recommended For |
|---------|-----------------|
| **Redis** | General purpose, high-throughput, low-latency deployments. |
| **PostgreSQL** | Deployments requiring strict ACID guarantees and survivable locks. |
| **DynamoDB** | AWS-native deployments. |

### 1. Choose a New Backend

Update your `acteon.toml` to use a supported state backend. For example, to migrate to Redis:

```toml
[state]
backend = "redis"
url = "redis://your-redis-host:6379"
```

### 2. Audit Data (No Change Required)

If you are using ClickHouse for your **audit trail**, no action is required. ClickHouse remains a first-class citizen for audit data and analytics. You can continue to use it by setting:

```toml
[audit]
enabled = true
backend = "clickhouse"
url = "http://your-clickhouse-host:8123"
```

### 3. State Migration

Because state data (locks, deduplication keys) is typically transient or has a short TTL, a "lift and shift" migration of the data is usually not necessary:

1.  **Deduplication**: If you rely on long-lived deduplication keys, these will be lost during the switch. Consider a maintenance window or double-writing if this is critical.
2.  **Locks**: In-flight locks will be lost. Ensure no critical processes are mid-execution during the cutover.
3.  **State Machines**: Active event states will be reset. Events will be treated as "new" when they next arrive.

## Verification

After updating your configuration, verify the server starts correctly:

```bash
cargo run -p acteon-server -- -c acteon.toml
```

If the server fails to start with an "unsupported state backend" error, ensure you have updated the `[state]` section and are not using the `clickhouse` value there.
