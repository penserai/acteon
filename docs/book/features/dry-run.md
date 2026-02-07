# Dry-Run Mode

Dry-run mode lets you evaluate the full rule pipeline for an action without
actually executing it, recording state changes, or emitting audit records.
This is essential for:

- **Testing rule changes** before deploying
- **Debugging** why an action was suppressed, rerouted, or throttled
- **Building rule-authoring tools** with instant feedback

## How It Works

Pass `?dry_run=true` as a query parameter on the dispatch endpoint:

```
POST /v1/dispatch?dry_run=true
POST /v1/dispatch/batch?dry_run=true
```

The gateway will:

1. Evaluate all rules (including CEL expressions) to produce a verdict
2. Run the LLM guardrail check (if configured)
3. Return a `DryRun` outcome describing what *would* happen

The gateway will **skip**:

- Distributed lock acquisition
- Provider execution
- State mutations (dedup keys, group state, chain state, etc.)
- Audit record emission
- Rate-limit counter updates

## Response Format

The `DryRun` outcome contains:

| Field | Type | Description |
|-------|------|-------------|
| `verdict` | string | The verdict tag: `allow`, `suppress`, `deny`, `reroute`, `throttle`, `deduplicate`, `modify`, `group`, `state_machine`, `request_approval`, `chain` |
| `matched_rule` | string? | Name of the matched rule, if any |
| `would_be_provider` | string | The provider that would handle the action (reflects rerouting) |

### Example: Action would be allowed

```bash
curl -X POST 'http://localhost:8080/v1/dispatch?dry_run=true' \
  -H 'Content-Type: application/json' \
  -d '{
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {"to": "user@example.com"}
  }'
```

```json
{
  "DryRun": {
    "verdict": "allow",
    "matched_rule": null,
    "would_be_provider": "email"
  }
}
```

### Example: Action would be suppressed

```json
{
  "DryRun": {
    "verdict": "suppress",
    "matched_rule": "block-spam",
    "would_be_provider": "email"
  }
}
```

### Example: Action would be rerouted

```json
{
  "DryRun": {
    "verdict": "reroute",
    "matched_rule": "reroute-urgent",
    "would_be_provider": "sms"
  }
}
```

## Client SDK Usage

### Rust

```rust
use acteon_client::ActeonClient;
use acteon_core::Action;

let client = ActeonClient::new("http://localhost:8080");
let action = Action::new("ns", "tenant", "email", "send", serde_json::json!({}));

let outcome = client.dispatch_dry_run(&action).await?;
println!("{:?}", outcome);
```

### Python

```python
from acteon_client import ActeonClient

client = ActeonClient("http://localhost:8080")
action = Action(namespace="ns", tenant="t1", provider="email",
                action_type="send", payload={})
outcome = client.dispatch(action, dry_run=True)
if outcome.is_dry_run():
    print(f"Verdict: {outcome.verdict_details['verdict']}")
```

### Node.js / TypeScript

```typescript
const outcome = await client.dispatch(action, { dryRun: true });
if (outcome.type === "dry_run") {
  console.log(`Verdict: ${outcome.verdict}`);
}
```

### Go

```go
outcome, err := client.DispatchDryRun(ctx, action)
if outcome.IsDryRun() {
    fmt.Println("Verdict:", outcome.Verdict)
}
```

### Java

```java
ActionOutcome outcome = client.dispatchDryRun(action);
if (outcome.isDryRun()) {
    System.out.println("Verdict: " + outcome.getVerdict());
}
```

## Simulation Framework

The simulation harness also supports dry-run mode:

```rust
use acteon_simulation::prelude::*;
use acteon_core::Action;

let harness = SimulationHarness::start(
    SimulationConfig::builder()
        .nodes(1)
        .add_recording_provider("email")
        .add_rule_yaml(r#"
            rules:
              - name: block-spam
                priority: 1
                condition:
                  field: action.action_type
                  eq: "spam"
                action:
                  type: suppress
        "#)
        .build()
).await.unwrap();

let action = Action::new("ns", "t1", "email", "spam", serde_json::json!({}));
let outcome = harness.dispatch_dry_run(&action).await.unwrap();

outcome.assert_dry_run();
// Provider was NOT called
harness.provider("email").unwrap().assert_not_called();
```

## Batch Dry-Run

Batch dispatch also supports dry-run. All actions in the batch are evaluated
independently and none are executed:

```bash
curl -X POST 'http://localhost:8080/v1/dispatch/batch?dry_run=true' \
  -H 'Content-Type: application/json' \
  -d '[
    {"namespace": "ns", "tenant": "t1", "provider": "email", "action_type": "send", "payload": {}},
    {"namespace": "ns", "tenant": "t1", "provider": "email", "action_type": "spam", "payload": {}}
  ]'
```

```json
[
  {"DryRun": {"verdict": "allow", "matched_rule": null, "would_be_provider": "email"}},
  {"DryRun": {"verdict": "suppress", "matched_rule": "block-spam", "would_be_provider": "email"}}
]
```
