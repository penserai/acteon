# Roadmap

Acteon's current product is an execution and governance platform spanning actions,
durable orchestration, agents, and managed event processing. Use the
[capability guide](../features/index.md) for what you can build today. This page
separates shipped milestones from possible extensions; it is not a delivery commitment.

## Available today

- [Durable executions](../features/durable-executions.md), [worker queues](../features/task-queues.md), and [code workflows](../features/workflows.md).
- [A2A tasks](../features/a2a.md), the [Agentic Bus](../concepts/agentic-bus.md), [swarm orchestration](../features/agent-swarm.md), and [mixed-backend federation qualification](../guides/federated-agent-mesh.md).
- [Governed model inference](../features/governed-model-provider.md) with locked contracts and response validation.
- [Managed stream stages](../features/managed-stream-stages.md), [quarantine repair](../features/stream-input-contracts.md), audited halt/resume, and [outbox delivery](../features/managed-stream-outbox.md).
- [Secret custody](../features/secret-custody.md) with versioned, host-owned references resolved at the effect boundary.
- [Durable dispatch receipts](../features/durable-dispatch.md), [action signing](../features/action-signing.md), [dead-letter retention](../features/dlq-retention.md), and [generated Prometheus alerts](../features/prometheus-alerting.md).

## Areas under consideration

### External secret-provider adapters

The host-owned secret resolver and scoped environment adapter are available.
Native adapters for Vault and cloud secret managers can implement the same
provider contract, retaining namespace, tenant, purpose, and version checks at
the effect boundary. Adoption should be driven by deployment demand rather
than adding provider SDKs to the default server binary.

### A2A callback scale

Durable ordered A2A callbacks now use the configured state backend for the Task
mutation log, callback cursors, fenced leases, retry deadlines, and dead letters.
The live broadcast is only a low-latency wake-up; bounded Task-page recovery
repairs missed events and restart gaps. A repairable ready-work index directs
normal polling to Tasks with callback work. Redis, PostgreSQL, and DynamoDB use
native bounded continuation for recovery inventory; custom state backends can
override the same paging contract. Backends may later add a native due-time
index for very large fleets. Bounded Task-page repair remains the correctness
fallback.

### DynamoDB Hierarchical Tenant Index

Hierarchical/multi-tenant audit reads currently fall back to a table `Scan` on the DynamoDB backend, because the `ns_tenant` composite partition key only supports exact match (see [DynamoDB Audit Backend](../backends/dynamodb-audit.md)). Add a sparse GSI keyed on `namespace` (PK) + `tenant` (SK) so a scoped read becomes a `begins_with(tenant, "acme.")` **Query** (indexed range) instead of a full Scan. Requires a write-time projection of the new key attributes and a one-time backfill. Only worth doing if users run scoped multi-tenant audit reads at volume on DynamoDB (ClickHouse/Postgres already handle this as an indexed prefix range).

### Additional Kafka publishing formats

The [Agentic Bus](../concepts/agentic-bus.md) already provides Kafka-backed topics,
publishing, subscriptions, and JSON Schema validation. Further integration with
Avro/Protobuf registries and provider-style publishing is a possible extension;
it should build on the existing transport and governance capabilities.

### Cost Attribution & Tenant Billing Export

Possible additions include per-provider cost configuration, per-tenant pricing
overrides, and usage exports for billing systems. API and CLI designs remain to
be defined.

## Longer-term direction

### Multi-Region HA Failover

Instance groups with health checking, leader election or consistent hashing for request routing, cross-region circuit breaker state sync, and geographic routing rules. Start with simple leader-election via state backend locks, grow to consistent hashing.
