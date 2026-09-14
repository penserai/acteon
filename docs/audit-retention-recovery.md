# Audit retention recovery

Audit records carry an optional `expires_at` timestamp. The in-memory audit
store uses the system clock by default and accepts an explicit `acteon-time`
clock for deterministic embeddings and tests.

`cleanup_expired()` treats `expires_at <= now` as expired. It removes the
primary record and its action-index entry together, while retaining later
records for the same action and records without a TTL.

## Executable evidence

`scenarios/audit-retention.json` runs the `audit_retention_recovery` scenario on
the memory backend. A shared manual clock holds an expiring record before its
deadline, advances to the exact ten-second boundary, and runs cleanup. The
scenario verifies removal, repeated-cleanup idempotence, action-index
consistency, and preservation of a later and a non-expiring record. A mutation
that skips the advance must fail the exact-boundary gate.

This proves the in-memory retention contract. Redis, PostgreSQL, DynamoDB,
Elasticsearch, and other audit stores still use their backend clocks and need
backend-specific expiry and cleanup evidence. DLQ retention, remote TTLs, and
process/network scheduling remain separate boundaries.
