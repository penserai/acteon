# acteon-client (Python)

Python client for the Acteon action gateway.

## Installation

```bash
pip install acteon-client
```

Or install from source:

```bash
cd clients/python
pip install -e .
```

## Quick Start

```python
from acteon_client import ActeonClient, Action

# Create a client
client = ActeonClient("http://localhost:8080")

# Check health
if client.health():
    print("Server is healthy")

# Dispatch an action
action = Action(
    namespace="notifications",
    tenant="tenant-1",
    provider="email",
    action_type="send_notification",
    payload={"to": "user@example.com", "subject": "Hello"},
)

outcome = client.dispatch(action)
print(f"Outcome: {outcome.outcome_type}")

# Close the client
client.close()
```

## Context Manager

```python
with ActeonClient("http://localhost:8080") as client:
    outcome = client.dispatch(action)
```

## Async Client

```python
import asyncio
from acteon_client import AsyncActeonClient, Action

async def main():
    async with AsyncActeonClient("http://localhost:8080") as client:
        action = Action(
            namespace="notifications",
            tenant="tenant-1",
            provider="email",
            action_type="send_notification",
            payload={"to": "user@example.com"},
        )
        outcome = await client.dispatch(action)
        print(f"Outcome: {outcome.outcome_type}")

asyncio.run(main())
```

## Batch Dispatch

```python
actions = [
    Action(namespace="ns", tenant="t1", provider="email", action_type="send", payload={"i": i})
    for i in range(10)
]

results = client.dispatch_batch(actions)
for result in results:
    if result.success:
        print(f"Success: {result.outcome.outcome_type}")
    else:
        print(f"Error: {result.error.message}")
```

## Rule Management

```python
# List all rules
rules = client.list_rules()
for rule in rules:
    print(f"{rule.name}: priority={rule.priority}, enabled={rule.enabled}")

# Reload rules from disk
result = client.reload_rules()
print(f"Loaded {result.loaded} rules")

# Disable a rule
client.set_rule_enabled("block-spam", False)
```

## Time-Based Rules

Rules can use `time.*` fields to match on the current UTC time at dispatch. Configure these in your YAML or CEL rule files — no client-side changes needed.

```yaml
# rules/business_hours.yaml
rules:
  - name: suppress-outside-hours
    priority: 1
    condition:
      any:
        - field: time.hour
          lt: 9
        - field: time.hour
          gte: 17
    action:
      type: suppress

  - name: suppress-weekends
    priority: 2
    condition:
      field: time.weekday_num
      gt: 5
    action:
      type: suppress
```

Use dry-run to test what a time-based rule would do right now:

```python
outcome = client.dispatch(action, dry_run=True)
print(f"Verdict: {outcome.verdict}")        # e.g. "suppress"
print(f"Matched rule: {outcome.matched_rule}")  # e.g. "suppress-outside-hours"
```

Available `time` fields: `hour` (0–23), `minute`, `second`, `day`, `month`, `year`, `weekday` (`"Monday"`…`"Sunday"`), `weekday_num` (1=Mon…7=Sun), `timestamp`.

## Audit Trail

```python
from acteon_client import AuditQuery

# Query audit records
query = AuditQuery(tenant="tenant-1", limit=10)
page = client.query_audit(query)
print(f"Found {page.total} records")
for record in page.records:
    print(f"  {record.action_id}: {record.outcome}")

# Get specific record
record = client.get_audit_record("action-id-123")
if record:
    print(f"Found: {record.outcome}")
```

## Configuration

```python
client = ActeonClient(
    "http://localhost:8080",
    timeout=60.0,        # Request timeout in seconds
    api_key="your-key",  # Optional API key
)
```

API keys are sent via the `Authorization: Bearer <key>` header. The server
accepts both JWTs and raw API keys on that header. API keys are scoped by
tenant, namespace, provider, and action type on the server side — see the
[API Key Scoping](https://penserai.github.io/acteon/features/api-key-scoping/)
documentation for the grant model and hierarchical tenant matching.

## Task-Queue Worker

`Worker` polls a durable task queue and dispatches each task to a handler
registered for its `action_type`. Returning completes the task; raising
fails it as **retryable by default** (bounded by the task's `max_attempts`).
Raise `NonRetryableError` to fail permanently. Long-running handlers are
heartbeat-extended automatically at half the lease interval, and `async def`
handlers are supported.

```python
from acteon_client import ActeonClient, NonRetryableError, Worker

client = ActeonClient("http://localhost:8080", api_key="your-key")
worker = Worker(client, "jobs", "tenant-1", queue="emails", max_concurrent=4)

def send_email(payload):
    if "@" not in payload["to"]:
        raise NonRetryableError("malformed address")  # never retried
    return {"message_id": deliver(payload)}           # completes the task

worker.register("send_email", send_email)
worker.run()  # blocks; call worker.stop() from a signal handler to drain

# Producers enqueue work with:
client.enqueue_task("emails", "jobs", "tenant-1", "send_email",
                    {"to": "user@example.com"}, max_attempts=5)
```

`worker.run_once()` polls and processes a single batch — useful in tests
and cron-style invocations.

## Workflows

Workflows are checkpoint-based: the registered function re-runs from the
top on every continuation, and `ctx` replays recorded checkpoints by name —
completed `ctx.step(...)` calls return their stored result instantly, and
suspension points (`ctx.sleep`, `ctx.wait_for_signal`) only suspend the
first time through. Code paths up to a suspension point must therefore be
deterministic; side effects belong inside `ctx.step`.

```python
from acteon_client import ActeonClient, Worker

client = ActeonClient("http://localhost:8080", api_key="your-key")
worker = Worker(client, "jobs", "tenant-1", queue="wf-queue")

def onboarding(ctx, input):
    account = ctx.step("provision", lambda: provision(input["user"]))
    ctx.sleep(24 * 3600)                                   # durable timer
    approval = ctx.wait_for_signal("approved", timeout_seconds=86_400)
    if approval is None:                                   # timed out
        return {"status": "expired"}
    child_id = ctx.start_child("welcome_email", {"account": account})
    outcome = ctx.wait_for_child(child_id)
    return {"status": "done", "email": outcome}

worker.register_workflow("onboarding", onboarding)
worker.run()
```

Drive executions from any client:

```python
execution = client.start_workflow(
    "jobs", "tenant-1", "onboarding", "wf-queue", {"user": "u-1"}
)
client.signal_workflow(execution.execution_id, "approved",
                       "jobs", "tenant-1", payload={"by": "ops"})
execution = client.get_workflow_execution(execution.execution_id, "jobs", "tenant-1")
print(execution.status, execution.result)
```

## Error Handling

```python
from acteon_client import ActeonError, ConnectionError, ApiError, HttpError

try:
    outcome = client.dispatch(action)
except ConnectionError as e:
    print(f"Connection failed: {e}")
    if e.is_retryable():
        # Retry logic
        pass
except ApiError as e:
    print(f"API error [{e.code}]: {e.message}")
    if e.is_retryable():
        # Retry logic
        pass
except HttpError as e:
    print(f"HTTP {e.status}: {e.message}")
```

## API Reference

### ActeonClient Methods

| Method | Description |
|--------|-------------|
| `health()` | Check server health |
| `dispatch(action)` | Dispatch a single action |
| `dispatch_batch(actions)` | Dispatch multiple actions |
| `list_rules()` | List all loaded rules |
| `reload_rules()` | Reload rules from disk |
| `set_rule_enabled(name, enabled)` | Enable/disable a rule |
| `query_audit(query)` | Query audit records |
| `get_audit_record(action_id)` | Get specific audit record |
| `fetch_signing_keys()` | Fetch the server's active signing keyring (JWKS-style discovery) |

### Action Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `namespace` | str | Yes | Logical grouping |
| `tenant` | str | Yes | Tenant identifier |
| `provider` | str | Yes | Target provider |
| `action_type` | str | Yes | Type of action |
| `payload` | dict | Yes | Action-specific data |
| `id` | str | No | Auto-generated UUID |
| `dedup_key` | str | No | Deduplication key |
| `metadata` | dict | No | Key-value metadata |

## License

Apache-2.0
