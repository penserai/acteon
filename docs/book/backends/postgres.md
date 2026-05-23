# PostgreSQL State Backend

The PostgreSQL backend provides ACID-guaranteed state storage with locks that survive failover.

<span class="badge recommended">Recommended</span> for strict consistency

## When to Use

- Financial transactions or operations requiring strict mutual exclusion
- Environments where duplicate processing is unacceptable
- When you need ACID guarantees for state operations
- Production deployments requiring maximum reliability

## Configuration

```toml title="acteon.toml"
[state]
backend = "postgres"
url = "postgres://acteon:acteon@localhost:5432/acteon"
prefix = "acteon_"
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `url` | string | — | PostgreSQL connection URL |
| `prefix` | string | `"acteon_"` | Table name prefix |

## Docker Setup

```bash
# Start PostgreSQL
docker compose --profile postgres up -d

# Or manually
docker run -d --name acteon-postgres -p 5432:5432 \
  -e POSTGRES_USER=acteon \
  -e POSTGRES_PASSWORD=acteon \
  -e POSTGRES_DB=acteon \
  postgres:16-alpine
```

## Characteristics

| Property | Value |
|----------|-------|
| **Throughput** | ~850 ops/sec |
| **Latency** | 10-20ms |
| **Persistence** | Full ACID |
| **Distribution** | Multi-instance support |
| **Mutual Exclusion** | ACID-guaranteed |
| **Feature Flag** | `postgres` |

## Why PostgreSQL for State?

1. **Locks survive failover** — PostgreSQL advisory locks and row-level locks are ACID-compliant. A failover to a replica preserves lock state.

2. **Perfect deduplication** — Atomic `INSERT ON CONFLICT` ensures exactly-once dedup semantics.

3. **Transactional counters** — Throttle counters use transactions for accuracy.

4. **Standard infrastructure** — Most teams already operate PostgreSQL.

## Example Configuration

```toml title="examples/postgres.toml"
[server]
host = "127.0.0.1"
port = 8080

[state]
backend = "postgres"
url = "postgres://acteon:acteon@localhost:5432/acteon"

[audit]
enabled = true
backend = "postgres"
url = "postgres://acteon:acteon@localhost:5432/acteon"

[rules]
directory = "./rules"
```

## Building with PostgreSQL Support

```bash
cargo build -p acteon-server --features postgres
```

## Usage

```bash
docker compose --profile postgres up -d
scripts/migrate.sh -c examples/postgres.toml
cargo run -p acteon-server --features postgres -- -c examples/postgres.toml
```
