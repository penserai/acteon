# Dead-letter retention

Acteon can bound dead-letter storage with one executor setting:

```toml
[executor]
dlq_enabled = true
dlq_retention_seconds = 604800 # seven days
```

The window applies to both the built-in in-memory action DLQ and the
state-store-backed A2A push-delivery DLQ. An entry expires when its age reaches
the configured duration. The in-memory queue uses the gateway's shared clock,
so embedded applications and deterministic tests can drive the exact boundary;
state-backed push entries use the state backend's TTL contract.

When the setting is omitted, existing indefinite-retention behavior is kept.
Custom `DeadLetterSink` implementations remain responsible for their own
retention policy. The background cleanup cadence removes expired entries from
the built-in queue, while state backends expire push-DLQ rows through their
normal TTL handling.

Retention never drains a live entry early. Operators can still inspect or
drain live entries through `/v1/dlq/*` and inspect or delete A2A push entries
through `/v1/a2a/{namespace}/{tenant}/push-dlq`.
