# DynamoDB State Backend

The DynamoDB backend provides strongly consistent state storage using AWS DynamoDB with conditional writes.

<span class="badge production">Production</span> for AWS-native deployments

## When to Use

- AWS-native infrastructure
- Serverless architectures
- When you need managed, scalable state storage
- Strong consistency requirements without self-managed databases

## Configuration

```toml title="acteon.toml"
[state]
backend = "dynamodb"
url = "http://localhost:8000"      # DynamoDB Local for development
region = "us-east-1"
table_name = "acteon_state"
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `url` | string | — | DynamoDB endpoint (local or AWS) |
| `region` | string | — | AWS region |
| `table_name` | string | — | DynamoDB table name |

## Docker Setup (DynamoDB Local)

```bash
# Start DynamoDB Local
docker compose --profile dynamodb up -d

# Or manually
docker run -d --name acteon-dynamodb -p 8000:8000 \
  amazon/dynamodb-local:latest
```

## Characteristics

| Property | Value |
|----------|-------|
| **Throughput** | ~340 ops/sec |
| **Latency** | 50-100ms |
| **Persistence** | Fully managed |
| **Distribution** | Multi-region capable |
| **Mutual Exclusion** | Strong (conditional writes) |
| **Feature Flag** | `dynamodb` |

## How It Works

DynamoDB uses **conditional writes** for atomic operations:

- `check_and_set` → `PutItem` with `attribute_not_exists` condition
- `compare_and_swap` → `UpdateItem` with version condition
- Distributed locking → `PutItem` with TTL and condition expressions

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

## Example Configuration

```toml title="examples/dynamodb.toml"
[server]
host = "127.0.0.1"
port = 8080

[state]
backend = "dynamodb"
url = "http://localhost:8000"
region = "us-east-1"
table_name = "acteon_state"

[rules]
directory = "./rules"
```

## Building with DynamoDB Support

```bash
cargo build -p acteon-server --features dynamodb
```

## Usage

```bash
docker compose --profile dynamodb up -d
scripts/migrate.sh -c examples/dynamodb.toml
cargo run -p acteon-server --features dynamodb -- -c examples/dynamodb.toml
```
