# Dead-letter retention

Dead-letter queues can be bounded without changing retry behavior. Configure a
retention window under `[executor]`:

```toml
[executor]
dlq_enabled = true
dlq_retention_seconds = 604800 # seven days
```

The setting applies to the built-in action DLQ. Entries are eligible for
removal when their age reaches the configured number of seconds, and the
in-memory queue is cleaned on the gateway background cadence.

Omit the setting to retain the existing indefinite behavior. Custom Rust
`DeadLetterSink` implementations manage their own retention. A2A callback dead
letters are bounded per Task and remain until an operator explicitly retries or
discards the blocked sequence; automatic age-based deletion would silently
change ordered delivery semantics. Operators can inspect or drain action
entries with `/v1/dlq/stats` and `/v1/dlq/drain`, or inspect and discard A2A
push entries under `/v1/a2a/push-dlq`.
