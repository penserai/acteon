# Roadmap

Acteon's current product is an execution and governance platform spanning actions,
durable orchestration, agents, and managed event processing. Use the
[capability guide](../features/index.md) for what you can build today. This page
separates shipped milestones from possible extensions; it is not a delivery commitment.

## Available today

- [Durable executions](../features/durable-executions.md), [worker queues](../features/task-queues.md), and [code workflows](../features/workflows.md).
- [A2A tasks](../features/a2a.md), the [Agentic Bus](../concepts/agentic-bus.md), and [swarm orchestration](../features/agent-swarm.md).
- [Governed model inference](../features/governed-model-provider.md) with locked contracts and response validation.
- [Managed stream stages](../features/managed-stream-stages.md), [quarantine repair](../features/stream-input-contracts.md), audited halt/resume, and [outbox delivery](../features/managed-stream-outbox.md).
- [Durable dispatch receipts](../features/durable-dispatch.md), [action signing](../features/action-signing.md), [dead-letter retention](../features/dlq-retention.md), and [generated Prometheus alerts](../features/prometheus-alerting.md).

## Areas under consideration

### DynamoDB Hierarchical Tenant Index

Hierarchical/multi-tenant audit reads currently fall back to a table `Scan` on the DynamoDB backend, because the `ns_tenant` composite partition key only supports exact match (see [DynamoDB Audit Backend](../backends/dynamodb-audit.md)). Add a sparse GSI keyed on `namespace` (PK) + `tenant` (SK) so a scoped read becomes a `begins_with(tenant, "acme.")` **Query** (indexed range) instead of a full Scan. Requires a write-time projection of the new key attributes and a one-time backfill. Only worth doing if users run scoped multi-tenant audit reads at volume on DynamoDB (ClickHouse/Postgres already handle this as an indexed prefix range).

### Cursor-Based Audit Pagination

`AuditQuery` currently uses offset-based pagination exclusively. This is efficient for Postgres/ClickHouse because rule coverage aggregation uses native `GROUP BY` and bypasses paging, but non-SQL audit backends (Memory, Elasticsearch, DynamoDB) fall through to `InMemoryAnalytics`, which pages with offsets internally and hits the classic linear-degradation anti-pattern on large scans.

Replace offset with cursor-based pagination (`after_id` / `before_timestamp` / opaque continuation tokens) across all audit backends and their client SDKs. Unlocks efficient deep scans for rule coverage, audit replay, and compliance exports on non-SQL backends. Also eliminates pagination-drift bugs when new records land mid-scan.

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
