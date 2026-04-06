# Roadmap

Planned features and enhancements for Acteon, ordered by estimated value/effort ratio. This is a living document — priorities may shift based on user feedback and operational experience.

## Quick Wins

### Rule Coverage CLI

`acteon rules coverage` command that analyzes loaded rules and generates a coverage matrix (namespace x tenant x provider x action_type). Identifies unmatched combinations and blind spots in safety policy. Optionally integrates with the audit trail to surface real unmatched actions.

**Crates:** `acteon-cli`, `acteon-rules`
**Complexity:** Small (~500 LOC)

### Prometheus Alerting Rules Export

`acteon metrics export-alerts` command and `GET /v1/metrics/alerts/prometheus.yaml` endpoint that auto-generates Prometheus alert rules from your Acteon config — per-provider SLO thresholds, quota utilization alerts, circuit breaker trip alerts, compliance mode audit failures, and retention TTL warnings.

**Crates:** `acteon-ops`, `acteon-server`
**Complexity:** Small (~400 LOC)

## Medium Effort

### Kafka Provider Integration

Native `acteon-kafka` provider for publishing actions to Kafka topics with topic selection via routing rules, schema registry integration (Avro/Protobuf), batch publishing, partition key configuration, and circuit breaker integration with broker health checks.

**Crates:** New `acteon-kafka` crate, `acteon-server`
**Complexity:** Medium (~1500 LOC)

### Tenant-Scoped API Keys

Extend the auth system with `allowed_tenants`, `allowed_namespaces`, `allowed_providers`, and `allowed_action_types` fields on API keys. Validate dispatch requests against key scopes before rule evaluation. Support hierarchical tenants (e.g., `acme.us-east` inherits from `acme`).

**Crates:** `acteon-server` (auth module)
**Complexity:** Medium (~800 LOC)

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
