# Redis Backend

The Redis backend provides fast, distributed state storage with strong mutual exclusion guarantees on a single instance.

<span class="badge production">Production</span>

## When to Use

- Most production deployments
- When you need distributed state across multiple Acteon instances
- When sub-millisecond latency matters more than ACID guarantees

## Configuration

```toml title="acteon.toml"
[state]
backend = "redis"
url = "redis://localhost:6379"
prefix = "acteon"
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `url` | string | — | Redis connection URL |
| `prefix` | string | `"acteon"` | Key prefix for all entries |

### Connection URL Format

```
redis://[username:password@]host[:port][/db]
redis+sentinel://host[:port][/db]?sentinel_master=mymaster
```

## Docker Setup

```bash
# Start Redis
docker compose up -d

# Or manually
docker run -d --name acteon-redis -p 6379:6379 redis:7-alpine
```

## Characteristics

| Property | Value |
|----------|-------|
| **Throughput** | ~2,000 ops/sec |
| **Latency** | 5-10ms (network round-trip) |
| **Persistence** | Configurable (RDB/AOF) |
| **Distribution** | Multi-instance support |
| **Mutual Exclusion** | Strong (single instance) |
| **Feature Flag** | `redis` (default) |

## Lock Behavior

### Single Instance

Redis provides strong mutual exclusion on a single instance. The lock is acquired atomically and released when the action completes.

### Sentinel / Cluster

With Redis Sentinel or Cluster, locks may be lost during failover. This is acceptable for most use cases where occasional duplicate processing is tolerable.

!!! tip "Strict Consistency"
    If you need locks that survive failover (e.g., financial transactions), use [PostgreSQL](postgres.md) or [DynamoDB](dynamodb.md) instead.

## Example Configuration

```toml title="examples/redis.toml"
[server]
host = "127.0.0.1"
port = 8080

[state]
backend = "redis"
url = "redis://localhost:6379"
prefix = "acteon"

[rules]
directory = "./rules"
```

## Usage

```bash
docker compose up -d
cargo run -p acteon-server -- -c examples/redis.toml
```
