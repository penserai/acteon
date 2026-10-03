<p align="center">
  <img src="docs/logo.svg" alt="Acteon — Actions forged in Rust" width="200">
</p>

<p align="center">
  <a href="https://github.com/penserai/acteon/actions/workflows/ci.yml"><img src="https://github.com/penserai/acteon/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

# Turn intent into accountable action.

**Acteon is an open-source execution and governance platform for AI agents and
deterministic operations.** It turns agent decisions, application requests, and
events into work governed by identity, policy, durable execution, and operational
evidence.

Use one platform to dispatch an integration, run a business workflow, coordinate
agents, or turn live telemetry into an incident response. Rules, approvals,
chains, worker workflows, typed model calls, and managed streams compose around
a shared operational foundation.

Acteon includes an HTTP server, Admin UI, CLI, MCP server, and SDKs for Rust,
Python, TypeScript, Go, and Java. It is built in Rust, self-hosted, and licensed
under Apache 2.0. Start locally with a log provider; add persistent storage,
credentials, worker code, and optional agent or Kafka capabilities as needed.

**[Run the quick start](https://penserai.github.io/acteon/getting-started/quickstart/)** ·
[Explore the platform](https://penserai.github.io/acteon/concepts/) ·
[Choose an execution model](https://penserai.github.io/acteon/concepts/execution-model/)

## A name rooted in transformation

In Greek mythology, **Actaeon** was a hunter transformed by Artemis into a stag—the
very creature he pursued. That idea of transformation gives Acteon its name and
its stag emblem.

Acteon brings that spirit to automation: raw intent takes shape through policy
and execution. An agent's proposal, a service request, or a burst of events can
be filtered, reshaped, routed, and carried through a durable workflow. **Actions
forged in Rust**, with controls that you define and outcomes you can inspect.

## Build a complete flow

- [Cascading neural observability](https://penserai.github.io/acteon/guides/neural-observability-detector/): real Kafka, Redis, and local Laya inference with deterministic routing, recovery, and audited controls.
- [Agent swarm coordination](https://penserai.github.io/acteon/guides/agent-swarm-coordination/): coordinated agent work with identity, policy, approvals, evaluation, and recovery.
- [Incident response](https://penserai.github.io/acteon/guides/incident-response-pipeline/) and [order processing](https://penserai.github.io/acteon/guides/ecommerce-order-pipeline/): durable operations across services and human decisions.
- [All guides](https://penserai.github.io/acteon/guides/), including A2A, cloud pipelines, and migration from Alertmanager.

## Features

### Rule-Based Action Processing

- **Suppression** — Block actions matching specific conditions (e.g., spam filtering, maintenance windows)
- **Deduplication** — Prevent duplicate processing using configurable keys and TTLs
- **Throttling** — Rate-limit actions per tenant, provider, or action type with automatic retry-after hints
- **Rerouting** — Dynamically redirect actions to different providers based on priority, load, or content
- **Payload Modification** — Transform action payloads before execution (redaction, enrichment, normalization)

### Durable Execution

- **Event History** — Append-only, per-execution event log (steps, timers, signals, terminal outcome) via `GET /v1/executions/{id}/history`
- **Durable Timers** — `timer` chain steps sleep for seconds or months with no resources consumed, surviving restarts
- **Signals** — `wait_for_signal` steps pause executions until an external signal arrives (buffered if early, with optional timeouts and timeout routing)
- **Definition Versioning** — In-flight executions pin the chain definition they started with; updates never change running executions
- **Visibility** — List and filter executions (including terminal) by status, time window, and custom search attributes
- **Task Queues & Workers** — CAS-guarded queues with lease/heartbeat/complete semantics; `worker` chain steps run on your own workers with automatic retry, backoff, and DLQ
- **Workflows as Code** — Checkpoint-based durable workflows in Python/TypeScript: `ctx.step()`, `ctx.sleep()`, `ctx.wait_for_signal()`, child workflows with parent-close policies — executed on customer workers, orchestrated and policy-gated by Acteon

### Event Grouping & State Machines

- **Event Grouping** — Batch related events together for consolidated notifications with configurable wait times and group sizes
- **State Machines** — Track event lifecycle through configurable states (e.g., open → investigating → resolved) with automatic timeout transitions
- **Inhibition** — Suppress dependent events when parent events are active using expression functions
- **Fingerprinting** — Correlate related events using configurable field-based fingerprints
- **Background Processing** — Automatic group flushing, timeout processing, and state cleanup

### Pluggable Backends

- **State Storage** — Memory, Redis, PostgreSQL, or DynamoDB for distributed locks and deduplication state
- **Audit Trail** — Memory, PostgreSQL, ClickHouse, or Elasticsearch for searchable action history with configurable retention

### Agent Interop ([A2A Protocol v1.0](https://a2aprotocol.org))

- **Task Engine** — 8-state Task lifecycle (`Submitted`, `Working`, `InputRequired`, `AuthRequired`, `Completed`, `Canceled`, `Failed`, `Rejected`) with CAS-retried atomic transitions, multi-hop reference-graph cycle detection, and the stale-task reaper as a backstop
- **Two Transports, One Engine** — JSON-RPC 2.0 (`message/send`, `tasks/get`, `tasks/cancel`, push-config CRUD, `agent/getAuthenticatedExtendedCard`) and the spec §11 REST binding share the same method implementations
- **Pause-for-Human** — `InputRequired` and `AuthRequired` interrupts pair a Task transition with a `BusApproval` row in one atomic step
- **Artifact Streaming Gatekeeper** — Enforces strict `chunk_index` order, no-updates-after-`lastChunk`, and `totalChunks` completeness across multi-chunk artifact deliveries
- **SSE Events** — `GET /a2a/{ns}/{tenant}/v1/tasks/{id}/events` re-uses the gateway broadcast and the same per-tenant connection caps as `/v1/stream`
- **Push Notifications** — Per-task webhook configs (CRUD over JSON-RPC + REST) plus a background delivery worker with bounded concurrent dispatch, short-TTL config cache, and refined retry classification (`408`/`425`/`429` transient, other `4xx` terminal)
- **Discovery** — Public unauthenticated `.well-known/agent.json` with single-card-verbatim vs. tenant-aggregated semantics, enriched with the gateway's intrinsic security schemes (`acteon.bearer`, `acteon.apiKey`)
- **Multi-tenant** — Every A2A endpoint scoped by `/a2a/{namespace}/{tenant}/…` with full grant-level authorization

### Enterprise Ready

- **Multi-Tenant** — Namespace and tenant isolation with per-tenant rate limiting
- **Authentication** — API key and JWT support with role-based access control and grant-level authorization
- **Hot Reload** — Update rules and auth configuration without restarts
- **Graceful Shutdown** — Drain in-flight requests before stopping
- **Observability** — Prometheus metrics, generated alerting rules, structured logging, and comprehensive audit trails

### Developer Experience

- **Admin UI** — Polished web interface for monitoring and configuration
- **OpenAPI/Swagger** — Auto-generated API documentation with interactive UI
- **Polyglot Clients** — Official SDKs for Rust, Python, Node.js/TypeScript, Go, and Java
- **Simulation Framework** — Test harness with mock providers, failure injection, and multi-node scenarios
- **YAML Rules** — Human-readable rule definitions with CEL expression support

## Architecture

All crates are organized under `crates/` with logical groupings:

### Core Components

| Crate | Description |
|-------|-------------|
| `crates/core` | Shared domain models (`Action`, `ActionOutcome`, keys, attachments, DAGs) |
| [`crates/client`](crates/client/README.md) | Native Rust HTTP client for the Acteon API |
| `crates/server` | HTTP server (Axum) with OpenAPI documentation and Swagger UI |
| `crates/gateway` | Gateway pipeline: locking, rules, execution, grouping, state machines, A2A engine |
| `crates/executor` | Action execution engine with retries, timeouts, and dead-lettering |
| `crates/provider` | Core Provider trait, provider registry, and built-in log provider |
| `crates/http` | Outbound HTTP transport client abstraction with SSRF protection |
| `crates/time` | Time abstraction supporting deterministic and manual virtual time |
| `crates/crypto` | Cryptographic utilities: AES-256-GCM encryption, Ed25519 signing, TLS |
| [`crates/simulation`](crates/simulation/README.md) | Testing framework with mock providers, cluster harness, and failure injection |
| `crates/wasm-runtime` | Sandboxed WASM plugin runtime (Wasmtime) for custom rule evaluations |
| `crates/llm` | LLM guardrail client for AI-assisted safety and content evaluation |
| `crates/embedding` | Vector embedding generation and cosine similarity for semantic routing |
| `crates/ops` | Shared operations client library powering CLI, MCP server, and tests |
| `crates/mcp-server` | Model Context Protocol (MCP) server exposing tools to LLMs and agents |
| `crates/cli` | Complete terminal CLI (`acteon`) for administration and operations |
| `crates/swarm` | Autonomous agent swarm orchestrator with adversarial critique and recovery |
| `crates/swarm-provider` | Ambient Swarm provider turning multi-agent goals into Acteon actions |
| `crates/bus` | Agentic message bus (Kafka-backed) for agent discovery, threads, and tool-calls |

### State Backends

| Crate | Description |
|-------|-------------|
| `crates/state/state` | Abstract state store / distributed lock trait |
| `crates/state/memory` | In-memory state backend (zero dependencies) |
| `crates/state/redis` | Redis state backend (high-throughput distributed locks) |
| `crates/state/postgres` | PostgreSQL state backend (ACID consistency) |
| `crates/state/dynamodb` | DynamoDB state backend (AWS-native serverless) |

### Audit Backends

| Crate | Description |
|-------|-------------|
| `crates/audit/audit` | Abstract audit trail trait and compliance verification |
| `crates/audit/memory` | In-memory audit backend |
| `crates/audit/postgres` | PostgreSQL audit backend (ACID, indexed queries, TTL) |
| `crates/audit/clickhouse` | ClickHouse audit backend (columnar analytics) |
| `crates/audit/elasticsearch` | Elasticsearch audit backend (search and ILM) |
| `crates/audit/dynamodb` | DynamoDB audit backend (AWS-native, native TTL, hash chain CAS) |

### Rules Frontends

| Crate | Description |
|-------|-------------|
| `crates/rules/rules` | Rule engine IR, evaluation engine, and matching predicates |
| `crates/rules/yaml` | YAML rule file parser and schema validator |
| `crates/rules/cel` | Common Expression Language (CEL) frontend |

### Cloud Providers

| Crate | Description |
|-------|-------------|
| `crates/aws` | AWS providers: SNS, SQS, Lambda, EventBridge, SES v2, S3, EC2, AutoScaling |
| `crates/azure` | Azure providers: Blob Storage, Event Hubs |
| `crates/gcp` | GCP providers: Cloud Storage, Cloud Pub/Sub |

### Integrations

| Crate | Description |
|-------|-------------|
| `crates/integrations/webhook` | Generic HTTP webhook dispatcher (Bearer, Basic, HMAC, API key) |
| `crates/integrations/email` | Email provider (SMTP via Lettre, AWS SES v2) |
| `crates/integrations/slack` | Slack Web API / Webhook messaging |
| `crates/integrations/pagerduty` | PagerDuty Events API v2 incident management |
| `crates/integrations/opsgenie` | Atlassian OpsGenie Alert API v2 |
| `crates/integrations/victorops` | VictorOps / Splunk On-Call REST integration |
| `crates/integrations/pushover` | Pushover mobile push notification delivery |
| `crates/integrations/telegram` | Telegram Bot messaging |
| `crates/integrations/wechat` | WeChat Work (企业微信) notification provider |
| `crates/integrations/twilio` | Twilio SMS and MMS provider |
| `crates/integrations/teams` | Microsoft Teams Incoming Webhook (MessageCard & Adaptive Card) |
| `crates/integrations/discord` | Discord Webhook provider with embeds and avatars |

## Running locally

### Prerequisites

- Rust 1.88+
- Cargo

### Quick start (in-memory, no external services)

```sh
cargo run --locked -p acteon-server -- -c examples/quickstart/acteon.toml
```

Run from the repository root. This config starts `http://127.0.0.1:8080` with
in-memory state, suppression/deduplication rules, and a log provider named `email`
that sends no real email. Follow the [verified quick start](docs/book/getting-started/quickstart.md)
for complete dispatch requests and expected responses. Omitting `-c` loads the
repository's separate `acteon.toml` demo if present; it does not force defaults.
You can then:

- Open the **Admin UI** at [http://127.0.0.1:8080/](http://127.0.0.1:8080/) after building it with `cd ui && npm ci && npm run build`
- Open **Swagger UI** at [http://127.0.0.1:8080/swagger-ui/](http://127.0.0.1:8080/swagger-ui/)
- Fetch the **OpenAPI spec** at [http://127.0.0.1:8080/api-doc/openapi.json](http://127.0.0.1:8080/api-doc/openapi.json)
- Hit the **health endpoint**: `curl http://127.0.0.1:8080/health`

### CLI options

```
cargo run -p acteon-server -- [OPTIONS] [COMMAND]

Options:
  -c, --config <PATH>   Path to TOML config file [default: acteon.toml]
      --host <HOST>      Override bind host
      --port <PORT>      Override bind port

Commands:
  encrypt   Encrypt a value for use in auth.toml (reads from stdin)
  migrate   Run database migrations for configured backends, then exit
```

Examples:

```sh
# Custom port
cargo run -p acteon-server -- --port 3000

# With a config file
cargo run -p acteon-server -- -c my-config.toml

# Run database migrations before first start
scripts/migrate.sh -c my-config.toml
```

### Configuration

Create an `acteon.toml` file (all sections are optional -- defaults are shown):

```toml
[server]
host = "127.0.0.1"
port = 8080
# shutdown_timeout_seconds = 30  # Max time to wait for pending tasks during shutdown

[ui]
# enabled = true
# dist_path = "ui/dist"

[state]
backend = "memory"   # "memory", "redis", "postgres", or "dynamodb"
# url = "redis://localhost:6379"
# prefix = "acteon"
# region = "us-east-1"       # DynamoDB only
# table_name = "acteon"      # DynamoDB only

[audit]
# enabled = false
# backend = "memory"         # "memory", "postgres", "clickhouse", or "elasticsearch"
# url = "postgres://acteon:acteon@localhost:5432/acteon"
# prefix = "acteon_"
# ttl_seconds = 2592000      # 30 days
# cleanup_interval_seconds = 3600
# store_payload = true

[rules]
# directory = "./rules"      # Path to YAML rule files

[executor]
# max_retries = 3
# timeout_seconds = 30
# max_concurrent = 100
# dlq_enabled = false
# dlq_retention_seconds = 604800  # Optional DLQ retention (7 days)

[auth]
# enabled = false
# config_path = "auth.toml"   # Path to auth config file
# watch = true                # Hot-reload on file changes

[audit.redact]
# enabled = false
# fields = ["password", "token", "api_key", "secret"]
# placeholder = "[REDACTED]"
```

### Environment

Set the `RUST_LOG` environment variable to control log verbosity:

```sh
RUST_LOG=debug cargo run -p acteon-server
```

## Development with backends

The `docker-compose.yml` ships with profiles for every supported backend. Redis runs by default; all others are opt-in.

### Available backends

| Backend | Type | Docker profile | Default URL |
|---------|------|----------------|-------------|
| Memory | state, audit | *(none)* | n/a |
| Redis | state | *(default)* | `redis://localhost:6379` |
| PostgreSQL | state, audit | `postgres` | `postgres://acteon:acteon@localhost:5432/acteon` |
| ClickHouse | audit | `clickhouse` | `http://localhost:8123` |
| Elasticsearch | audit | `elasticsearch` | `http://localhost:9200` |
| DynamoDB Local | state | `dynamodb` | `http://localhost:8000` |

### Starting backends

```sh
# Start Redis (default, always runs)
docker compose up -d

# Start a single optional backend
docker compose --profile postgres up -d

# Start multiple backends at once
docker compose --profile postgres --profile elasticsearch up -d
```

### Example configurations

Ready-to-use config files are provided in the `examples/` directory. Pair each one with the matching Docker profile:

```sh
# Redis state (default Docker services)
docker compose up -d
cargo run -p acteon-server -- -c examples/redis.toml

# PostgreSQL state + audit
docker compose --profile postgres up -d
scripts/migrate.sh -c examples/postgres.toml
cargo run -p acteon-server --features postgres -- -c examples/postgres.toml

# ClickHouse audit (with Redis state)
docker compose --profile clickhouse up -d
scripts/migrate.sh -c examples/clickhouse.toml
cargo run -p acteon-server --features clickhouse -- -c examples/clickhouse.toml

# Redis state + Elasticsearch audit
docker compose --profile elasticsearch up -d
scripts/migrate.sh -c examples/elasticsearch-audit.toml
cargo run -p acteon-server -- -c examples/elasticsearch-audit.toml

# DynamoDB Local state
docker compose --profile dynamodb up -d
scripts/migrate.sh -c examples/dynamodb.toml
cargo run -p acteon-server --features dynamodb -- -c examples/dynamodb.toml
```

### Combining backends

State and audit backends are independent. You can mix any state backend with any audit backend:

```toml
# Redis for state, PostgreSQL for audit
[state]
backend = "redis"
url = "redis://localhost:6379"

[audit]
enabled = true
backend = "postgres"
url = "postgres://acteon:acteon@localhost:5432/acteon"
```

```sh
docker compose --profile postgres up -d
scripts/migrate.sh -c acteon.toml
cargo run -p acteon-server -- -c acteon.toml
```

## API endpoints

| Method | Path | Description |
|--------|------|-------------|
| **System & Metrics** |||
| GET | `/health` | Health check with provider status and metrics snapshot |
| GET | `/metrics` | Dispatch counters |
| GET | `/metrics/prometheus` | Prometheus exposition endpoint with retention gauges |
| GET | `/v1/metrics/alerts/prometheus.yaml` | Generated Prometheus alerting rules for active configuration |
| **Dispatch & Streams** |||
| POST | `/v1/dispatch` | Dispatch a single action (supports `?dry_run=true`) |
| POST | `/v1/dispatch/batch` | Dispatch multiple actions atomically |
| GET | `/v1/stream` | Server-Sent Events (SSE) real-time stream of action outcomes |
| **Rules & Governance** |||
| GET | `/v1/rules` | List loaded routing rules |
| POST | `/v1/rules/reload` | Reload rules from configured directory |
| PUT | `/v1/rules/{name}/enabled` | Enable or disable a rule dynamically |
| POST | `/v1/rules/evaluate` | Dry-run evaluate action with detailed rule trace |
| GET | `/v1/rules/coverage` | Rule condition test coverage report |
| GET | `/v1/quotas` | List tenant quota policies and current usage |
| GET | `/v1/silences` | List and create Alertmanager-compatible silences |
| GET | `/v1/time-intervals` | List and manage temporal routing windows |
| **Durable Execution & Workflows** |||
| GET | `/v1/chains` | List task chains and execution state |
| GET | `/v1/chains/{chain_id}/dag` | Chain execution Directed Acyclic Graph (DAG) |
| GET | `/v1/chains/definitions` | List and manage reusable chain definitions |
| POST | `/v1/queues/{queue}/poll` | Worker queue polling with lease acquisition |
| POST | `/v1/queues/tasks/{id}/complete` | Mark leased worker task as complete |
| POST | `/v1/workflows/start` | Start code-based durable workflow execution |
| GET | `/v1/executions` | List executions with state, timers, and signals |
| **A2A Protocol & Swarm** |||
| POST | `/a2a/{ns}/{tenant}` | A2A JSON-RPC 2.0 protocol endpoint |
| POST | `/a2a/{ns}/{tenant}/v1/message:send` | A2A REST task submission |
| GET | `/a2a/{ns}/{tenant}/v1/tasks/{id}` | A2A task status and artifact inspect |
| GET | `/a2a/{ns}/{tenant}/.well-known/agent.json` | Public A2A agent discovery card |
| GET | `/v1/swarm/runs` | List autonomous agent swarm runs |
| **Agentic Message Bus** |||
| GET | `/v1/bus/topics` | List and create Kafka-backed bus topics |
| POST | `/v1/bus/publish` | Publish message with schema validation |
| GET | `/v1/bus/subscriptions` | Durable consumer subscriptions, lag, and offsets |
| GET | `/v1/bus/agents` | Agent registry with liveness and heartbeat |
| GET | `/v1/bus/conversations` | Multi-agent conversation threads and message replay |
| **Audit & Resilience** |||
| GET | `/v1/audit` | Query audit records with multi-dimensional filters |
| POST | `/v1/audit/replay` | Bulk replay actions from audit records |
| POST | `/v1/audit/verify` | Verify cryptographic SHA-256 compliance hash chain |
| GET | `/v1/actions/{id}/verify` | Cryptographic verification of Ed25519 action signature |
| GET | `/v1/providers/health` | Provider health metrics, latencies, and circuit states |
| POST | `/admin/circuit-breakers/{provider}/trip` | Administratively trip a provider circuit breaker |
| POST | `/v1/dlq/drain` | Drain dead-letter queue records for reprocessing |

Full OpenAPI 3.0 schemas and interactive testing are available in the Swagger UI (`/swagger-ui/`).

## Event Grouping & State Machines

### State Machine Rules

Track event lifecycle through configurable states with automatic timeout transitions:

```yaml
# rules/alert-lifecycle.yaml
rules:
  - name: alert-state-machine
    condition:
      field: action.action_type
      eq: alert
    action:
      type: state_machine
      state_machine: alert
      fingerprint_fields:
        - action_type
        - metadata.cluster
        - metadata.service
```

Configure state machines in your `acteon.toml`:

```toml
[[state_machines]]
name = "alert"
initial_state = "firing"
states = ["firing", "acknowledged", "resolved"]

[[state_machines.transitions]]
from = "firing"
to = "acknowledged"

[[state_machines.transitions]]
from = "acknowledged"
to = "resolved"

[[state_machines.timeouts]]
state = "firing"
after_seconds = 3600
transition_to = "stale"
```

### Event Grouping

Batch related events for consolidated notifications:

```yaml
rules:
  - name: group-cluster-alerts
    condition:
      field: action.action_type
      starts_with: cluster_
    action:
      type: group
      group_by:
        - metadata.cluster
        - metadata.severity
      group_wait_seconds: 60      # Wait before first notification
      group_interval_seconds: 300 # Min time between notifications
      max_group_size: 100
```

### Inhibition

Suppress dependent events when parent events are active using expression functions:

```yaml
rules:
  - name: inhibit-pod-alerts-on-cluster-down
    condition:
      all:
        - field: action.action_type
          starts_with: pod_
        - call: has_active_event
          args: [cluster_down, action.metadata.cluster]
    action:
      type: suppress
      reason: "Cluster is down"
```

Available expression functions for state lookups:
- `has_active_event(event_type, label_value)` — Check if an active event exists
- `get_event_state(fingerprint)` — Get current state of an event
- `event_in_state(fingerprint, state)` — Check if event is in a specific state

## Lock consistency

Acteon uses distributed locks to ensure only one instance processes a given action at a time. The consistency guarantees vary by backend:

| Backend | Failover Behavior | Recommendation |
|---------|-------------------|----------------|
| Redis (single) | Strong mutual exclusion | Good for development, single-node production |
| Redis (Sentinel/Cluster) | Lock may be lost during failover | Use only if occasional duplicates are acceptable |
| PostgreSQL | Locks survive failover (ACID) | Recommended for strong consistency |
| DynamoDB | Strong consistency available | Recommended for strong consistency |
| Memory | Single-process only | Development/testing only |

If your application requires strict mutual exclusion guarantees (e.g., financial transactions), use PostgreSQL or DynamoDB as your state backend. The Redis backend is suitable for scenarios where occasional duplicate processing during rare failover events is acceptable.

## Testing & Simulation

The `acteon-simulation` crate provides comprehensive testing tools:

```rust
use acteon_simulation::prelude::*;
use acteon_core::Action;

#[tokio::test]
async fn test_deduplication() {
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("email")
            .add_rule_yaml(DEDUP_RULE)
            .build()
    ).await.unwrap();

    let action = Action::new("ns", "tenant", "email", "notify", json!({}))
        .with_dedup_key("unique-key");

    harness.dispatch(&action).await.unwrap().assert_executed();
    harness.dispatch(&action).await.unwrap().assert_deduplicated();

    harness.provider("email").unwrap().assert_called(1);
    harness.teardown().await.unwrap();
}
```

### Features

- **RecordingProvider**: Captures all provider calls for verification
- **FailingProvider**: Simulates timeouts, connection errors, rate limiting
- **Mixed Backends**: Test any combination of state and audit backends
- **Failure Injection**: `FailureMode::EveryN`, `FirstN`, `Probabilistic`
- **End-to-End Audit**: Verify all outcomes are recorded (executed, suppressed, deduplicated, throttled, failed)

### Running Simulations

```sh
# Single backend simulations
cargo run -p acteon-simulation --example redis_simulation --features redis
cargo run -p acteon-simulation --example postgres_simulation --features postgres

# Mixed backend simulations (e.g., Redis state + PostgreSQL audit)
cargo run -p acteon-simulation --example mixed_backends_simulation \
  --features "redis,postgres" -- redis-postgres
```

See the [acteon-simulation README](crates/simulation/README.md) for full documentation.

## Client Libraries

Official client libraries are available for multiple languages:

| Language | Package | Documentation |
|----------|---------|---------------|
| Rust | `acteon-client` | [README](crates/client/README.md) |
| Python | `acteon-client` | [README](clients/python/README.md) |
| Node.js/TypeScript | `@acteon/client` | [README](clients/nodejs/README.md) |
| Go | `github.com/penserai/acteon/clients/go/acteon` | [README](clients/go/README.md) |
| Java | `com.acteon:acteon-client` | [README](clients/java/README.md) |

### Quick Examples

**Rust:**
```rust
let client = ActeonClient::new("http://localhost:8080");
let action = Action::new("ns", "tenant", "email", "send", json!({"to": "user@example.com"}));
let outcome = client.dispatch(&action).await?;
```

**Python:**
```python
client = ActeonClient("http://localhost:8080")
action = Action("ns", "tenant", "email", "send", {"to": "user@example.com"})
outcome = client.dispatch(action)
```

**Node.js/TypeScript:**
```typescript
const client = new ActeonClient("http://localhost:8080");
const action = createAction("ns", "tenant", "email", "send", { to: "user@example.com" });
const outcome = await client.dispatch(action);
```

**Go:**
```go
client := acteon.NewClient("http://localhost:8080")
action := acteon.NewAction("ns", "tenant", "email", "send", map[string]any{"to": "user@example.com"})
outcome, _ := client.Dispatch(ctx, action)
```

**Java:**
```java
ActeonClient client = new ActeonClient("http://localhost:8080");
Action action = new Action("ns", "tenant", "email", "send", Map.of("to", "user@example.com"));
ActionOutcome outcome = client.dispatch(action);
```

See the [clients directory](clients/README.md) for full documentation.

## Tests

```sh
cargo test --workspace
```

## Linting

```sh
cargo clippy --workspace --no-deps -- -D warnings
cargo fmt --all -- --check
```

## License

Copyright 2026 Renzo C. Sanchez-Silva

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
