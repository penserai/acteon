# Dead-letter retention

Dead-letter queues can be bounded without changing retry behavior. Configure a
retention window under `[executor]`:

```toml
[executor]
dlq_enabled = true
dlq_retention_seconds = 604800 # seven days
```

The setting applies to the built-in action DLQ and the A2A push-delivery DLQ.
Entries are eligible for removal when their age reaches the configured number
of seconds. The in-memory queue is cleaned on the gateway background cadence;
state-backed A2A push entries use the state backend's normal TTL handling.

Omit the setting to retain the existing indefinite behavior. Custom Rust
`DeadLetterSink` implementations manage their own retention. Operators can
still inspect or drain action entries with `/v1/dlq/stats` and
`/v1/dlq/drain`, or inspect and delete A2A push entries under
`/v1/a2a/{namespace}/{tenant}/push-dlq`.
