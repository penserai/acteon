# Roadmap

Planned features and enhancements for Acteon, ordered by estimated value/effort ratio. This is a living document — priorities may shift based on user feedback and operational experience.

## Quick Wins

### Prometheus Alerting Rules Export

`acteon metrics export-alerts` command and `GET /v1/metrics/alerts/prometheus.yaml` endpoint that auto-generates Prometheus alert rules from your Acteon config — per-provider SLO thresholds, quota utilization alerts, circuit breaker trip alerts, compliance mode audit failures, and retention TTL warnings.

**Crates:** `acteon-ops`, `acteon-server`
**Complexity:** Small (~400 LOC)

## Medium Effort

### Cursor-Based Audit Pagination

`AuditQuery` currently uses offset-based pagination exclusively. This is efficient for Postgres/ClickHouse because rule coverage aggregation uses native `GROUP BY` and bypasses paging, but non-SQL audit backends (Memory, Elasticsearch, DynamoDB) fall through to `InMemoryAnalytics`, which pages with offsets internally and hits the classic linear-degradation anti-pattern on large scans.

Replace offset with cursor-based pagination (`after_id` / `before_timestamp` / opaque continuation tokens) across all audit backends and their client SDKs. Unlocks efficient deep scans for rule coverage, audit replay, and compliance exports on non-SQL backends. Also eliminates pagination-drift bugs when new records land mid-scan.

**Crates:** `acteon-audit`, `acteon-audit-*` (all backends), `acteon-client`, polyglot SDKs, `acteon-cli`
**Complexity:** Medium (~1500 LOC; touches every audit backend and every SDK's query surface)

### Kafka Provider Integration

Native `acteon-kafka` provider for publishing actions to Kafka topics with topic selection via routing rules, schema registry integration (Avro/Protobuf), batch publishing, partition key configuration, and circuit breaker integration with broker health checks.

**Crates:** New `acteon-kafka` crate, `acteon-server`
**Complexity:** Medium (~1500 LOC)

### Action Signing & Tamper-Proof Dispatch

Ed25519/ECDSA signing of dispatch requests with a `signature` and `signer_id` on each action. Validate incoming actions against a keyring. Provide `GET /v1/actions/{id}/verify` for cryptographic proof of action origin. Export signed audit records for downstream verification.

**Crates:** `acteon-crypto`, `acteon-core`, `acteon-server`
**Complexity:** Medium (~1200 LOC)

### Cost Attribution & Tenant Billing Export

Per-provider cost configuration, per-tenant pricing overrides, and billing export endpoints. `GET /v1/billing/tenant/{tenant}/usage` returns volume, cost, and breakdown by provider. `GET /v1/billing/export?format=csv` for bulk monthly export. CLI command `acteon billing export`.

**Crates:** New `acteon-billing` crate, `acteon-server`, `acteon-cli`
**Complexity:** Medium (~1200 LOC)

## Large Effort

### Multi-Region HA Failover

Instance groups with health checking, leader election or consistent hashing for request routing, cross-region circuit breaker state sync, and geographic routing rules. Start with simple leader-election via state backend locks, grow to consistent hashing.

**Crates:** New `acteon-ha-coordinator` crate, `acteon-server`, state backends
**Complexity:** Large (~2500 LOC)
