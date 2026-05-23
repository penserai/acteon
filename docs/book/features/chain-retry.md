# Chain Step Retry Policies

When a chain step fails with a retryable error (connection failure, timeout, rate limit), Acteon can automatically retry the step before applying the `on_failure` policy. This avoids aborting or skipping a chain due to transient provider issues.

## Configuration

Add a `retry` block to any provider step in your chain definition:

```toml
[[chains.steps]]
name = "call-payment-api"
provider = "payment-svc"
action_type = "charge"
payload = '{"amount": "{{origin.payload.amount}}"}'

[chains.steps.retry]
max_retries = 3
backoff_ms = 500
strategy = "exponential"
jitter_ms = 100
```

## Retry Policy Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_retries` | `u32` | *required* | Maximum additional attempts (e.g., 3 means up to 4 total executions) |
| `backoff_ms` | `u64` | `1000` | Base delay in milliseconds between attempts |
| `strategy` | `string` | `"fixed"` | Backoff scaling: `fixed`, `linear`, or `exponential` |
| `jitter_ms` | `u64` | `null` | Random jitter range added to each delay (0 to `jitter_ms`) |

### Backoff Strategies

- **Fixed**: Every retry waits `backoff_ms`. Good for rate-limited APIs with a known cooldown.
- **Linear**: Delay is `backoff_ms * attempt`. Attempt 1 = 500ms, attempt 2 = 1000ms, attempt 3 = 1500ms.
- **Exponential**: Delay is `backoff_ms * 2^(attempt-1)`. Attempt 1 = 500ms, attempt 2 = 1000ms, attempt 3 = 2000ms.

## Interaction with `on_failure`

The step's `on_failure` policy (`abort`, `skip`, or `dlq`) only fires **after all retries are exhausted**. The decision flow is:

1. Step executes and fails with a retryable error.
2. If `current_attempt <= max_retries`, schedule a retry with the computed delay.
3. If retries are exhausted (or the error is non-retryable), apply `on_failure`.

Non-retryable errors (e.g., 400 Bad Request, configuration errors) skip the retry policy entirely and go straight to `on_failure`.

## Scope

Retry policies apply only to **provider steps**. Sub-chain steps and parallel step groups do not support retry policies -- use retry on the individual provider steps within those constructs instead.

## Execution History API

Each retry attempt is recorded in the chain's step history. Query it with:

```
GET /v1/chains/{chain_id}/history?namespace=ns&tenant=tenant-1
```

Example response:

```json
{
  "chain_id": "abc-123",
  "chain_name": "payment-pipeline",
  "status": "completed",
  "steps": [
    {
      "step_name": "call-payment-api",
      "attempts": [
        {
          "attempt": 1,
          "started_at": "2026-04-03T10:00:00Z",
          "completed_at": "2026-04-03T10:00:01Z",
          "success": false,
          "error": "connection refused"
        },
        {
          "attempt": 2,
          "started_at": "2026-04-03T10:00:02Z",
          "completed_at": "2026-04-03T10:00:02Z",
          "success": true,
          "error": null
        }
      ]
    }
  ]
}
```

Each attempt includes timing information and the error message (if any), making it straightforward to diagnose transient failures in production.
