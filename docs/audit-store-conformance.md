# Audit-store conformance

`AuditStore` now has an executable portable contract in
`acteon_audit::testing::run_audit_store_conformance_tests`. Backend tests can
run it with an isolated record prefix, so a shared integration database can
exercise the same contract without observing or overwriting records from
another test. The memory, PostgreSQL, and DynamoDB backends run it. PostgreSQL
uses a fresh table prefix against the disposable CI database; DynamoDB uses a
fresh table against the disposable local emulator.

The contract verifies missing reads, ID round trips, newest-record lookup for an
action, exact namespace/tenant filtering, caller identity filtering,
hierarchical tenant scope, and cursor traversal. Cursor coverage includes two
records with the same timestamp, ensuring the record ID tie-breaker prevents a
record from being repeated or skipped.

The memory backend now applies `AuditQuery.caller_id`, matching the query
contract already implemented by the database backends. Retention cleanup stays
outside this portable suite because memory uses the injected clock, DynamoDB
uses asynchronous TTL expiry, and ClickHouse deletion is asynchronous. Those
backends retain their dedicated retention contracts.
