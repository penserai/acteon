# Dead-letter retention

Acteon can bound dead-letter storage with one executor setting:

```toml
[executor]
dlq_enabled = true
dlq_retention_seconds = 604800 # seven days
```

The window applies to the built-in in-memory action DLQ. An entry expires when
its age reaches the configured duration. The queue uses the gateway's shared
clock, so embedded applications and deterministic tests can drive the exact
boundary.

When the setting is omitted, existing indefinite-retention behavior is kept.
Custom `DeadLetterSink` implementations remain responsible for their own
retention policy. The background cleanup cadence removes expired entries from
the built-in queue.

Retention never drains a live entry early. A2A callback failures are bounded in
the durable Task envelope and require an explicit retry or discard so age-based
cleanup cannot violate callback ordering. Operators can inspect or drain action
entries through `/v1/dlq/*` and inspect or discard A2A push entries through
`/v1/a2a/push-dlq`.
