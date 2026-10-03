# Platform architecture

Acteon separates policy, execution, coordination, and persistence into composable
Rust components. The server brings them together behind operational APIs. Workers,
model runtimes, agent engines, and external services connect at explicit boundaries.

## Runtime view

```mermaid
flowchart TB
    Clients[Applications, agents, CLI, and Admin UI] --> API[HTTP APIs and scoped authorization]
    MCP[MCP server] --> API
    API --> Gateway[Action gateway: policy and provider execution]
    API --> Exec[Durable executions and worker queues]
    API --> Bus[Agentic Bus]
    API --> A2A[A2A task services]
    Gateway --> Providers[Providers: integrations, governed models, swarm goals]
    Exec --> Gateway
    Exec <--> Workers[Your task and workflow workers]
    Bus <--> Kafka[Kafka topics and subscriptions]
    Kafka <--> Stages[Your managed stream workers]
    Stages --> Outbox[Checkpointed output delivery]
    Outbox --> Gateway
    Providers --> External[External services and model or agent runtimes]
    Gateway --> State[(State store)]
    Exec --> State
    Bus --> State
    A2A --> State
    Stages --> State
    Outbox --> State
    Gateway -.-> Audit[(Configured audit store)]
```

The diagram shows the main runtime relationships, not a mandatory topology.
In-memory state and a log provider are enough for the quick start. Persistent
state supports recovery across restarts. Agent orchestration and the Kafka bus
are optional build/runtime capabilities.

## Responsibilities and boundaries

| Component | Responsibility |
|---|---|
| **Server and operational interfaces** | Authenticate callers, enforce scoped API access, expose dispatch and lifecycle operations, and serve the Admin UI |
| **Action gateway** | Apply the configured dispatch pipeline: policy, coordination, provider execution, and audit recording |
| **Execution engine** | Track chains and workflows, pinned definitions, steps, waits, worker tasks, and history |
| **Providers** | Translate actions into integration calls, including validated JSON inference and accepted swarm goals |
| **Workers** | Run custom task/workflow code or typed stream processors in your environment |
| **Bus and A2A services** | Support topics, agent identity, conversations, and interoperable task lifecycles through their respective APIs |
| **State backend** | Persist coordination state, execution progress, leases, receipts, and stream checkpoints |
| **Audit backend** | Store configured searchable action evidence independently of execution state |

A provider response, a durable acceptance receipt, a completed workflow, and a
Kafka acknowledgement describe different boundaries. See
[the execution model](execution-model.md) for how they compose and
[governance](governance.md) for where policy applies.

## Crate Organization

All crates live under the `crates/` directory with logical groupings:

### Core Components

| Crate | Package | Description |
|-------|---------|-------------|
| `crates/core` | `acteon-core` | Shared types: `Action`, `ActionOutcome`, newtypes, state machine configs |
| `crates/gateway` | `acteon-gateway` | Central orchestration — lock, rules, execution, grouping, state machines |
| `crates/server` | `acteon-server` | HTTP server (Axum) with Swagger UI and OpenAPI |
| `crates/client` | `acteon-client` | Native Rust HTTP client for the Acteon API |
| `crates/executor` | `acteon-executor` | Action execution with retries, backoff, and concurrency limits |
| `crates/provider` | `acteon-provider` | Provider trait definitions and registry |
| `crates/simulation` | `acteon-simulation` | Testing framework with mock providers and failure injection |

### State Backends

| Crate | Package | Description |
|-------|---------|-------------|
| `crates/state/state` | `acteon-state` | Abstract `StateStore` and `DistributedLock` traits |
| `crates/state/memory` | `acteon-state-memory` | In-memory backend (single-process) |
| `crates/state/redis` | `acteon-state-redis` | Redis backend (distributed) |
| `crates/state/postgres` | `acteon-state-postgres` | PostgreSQL backend (ACID) |
| `crates/state/dynamodb` | `acteon-state-dynamodb` | AWS DynamoDB backend |

### Audit Backends

| Crate | Package | Description |
|-------|---------|-------------|
| `crates/audit/audit` | `acteon-audit` | Abstract `AuditStore` trait |
| `crates/audit/memory` | `acteon-audit-memory` | In-memory audit (testing) |
| `crates/audit/postgres` | `acteon-audit-postgres` | PostgreSQL audit (persistent) |
| `crates/audit/clickhouse` | `acteon-audit-clickhouse` | ClickHouse audit (analytics) |
| `crates/audit/elasticsearch` | `acteon-audit-elasticsearch` | Elasticsearch audit (search) |

### Rules Frontends

| Crate | Package | Description |
|-------|---------|-------------|
| `crates/rules/rules` | `acteon-rules` | Rule engine IR and evaluation |
| `crates/rules/yaml` | `acteon-rules-yaml` | YAML rule file parser |
| `crates/rules/cel` | `acteon-rules-cel` | CEL expression support |

### Integrations

| Crate | Package | Description |
|-------|---------|-------------|
| `crates/integrations/email` | `acteon-email` | Email/SMTP provider via Lettre |
| `crates/integrations/slack` | `acteon-slack` | Slack message provider |
| `crates/integrations/pagerduty` | `acteon-pagerduty` | PagerDuty Events API v2 provider |
| `crates/integrations/twilio` | `acteon-twilio` | Twilio SMS provider |
| `crates/integrations/teams` | `acteon-teams` | Microsoft Teams incoming webhooks |
| `crates/integrations/discord` | `acteon-discord` | Discord webhook provider |
| `crates/integrations/webhook` | `acteon-webhook` | Generic HTTP webhook provider |
| `crates/llm` | `acteon-llm` | LLM-based guardrail evaluation |

## Data Flow

A direct dispatch follows the gateway path below. Durable admission and longer
executions add their own persistence and lifecycle protocols around it. Audit
recording applies when an audit backend is configured:

```mermaid
sequenceDiagram
    participant C as Client
    participant S as Server
    participant G as Gateway
    participant R as Rule Engine
    participant ST as State Store
    participant E as Executor
    participant P as Provider
    participant A as Audit Store

    C->>S: POST /v1/dispatch
    S->>G: gateway.dispatch(action)
    G->>ST: Acquire distributed lock
    G->>R: Evaluate rules
    R-->>G: RuleVerdict (allow/deny/throttle/...)

    alt Suppressed
        G-->>S: ActionOutcome::Suppressed
    else Deduplicated
        G->>ST: check_and_set(dedup_key)
        ST-->>G: Already exists
        G-->>S: ActionOutcome::Deduplicated
    else Allowed
        G->>E: Execute action
        E->>P: provider.execute(action)
        P-->>E: ProviderResponse
        E-->>G: ActionOutcome::Executed
    end

    G->>ST: Release lock
    G->>A: Record audit entry
    G-->>S: ActionOutcome
    S-->>C: HTTP Response
```

## Design Principles

1. **Type Safety** — Strong typing with newtypes (`Namespace`, `TenantId`, `ActionId`, `ProviderId`) prevents field confusion at compile time.

2. **Trait-Based Abstraction** — Every pluggable component (`StateStore`, `AuditStore`, `DynProvider`) is defined as an async trait, enabling backend swapping without code changes.

3. **Zero Unsafe Code** — The workspace forbids `unsafe` code via `#![forbid(unsafe_code)]`.

4. **Pipeline Model** — Actions flow through a linear pipeline (intake → rules → execution → audit), making the system predictable and debuggable.

5. **Backend Independence** — Choose state and audit storage independently to match recovery and evidence requirements.

6. **Hot Reload** — Rules and auth configuration can be reloaded at runtime without server restarts.
