# acteon-client

Native Rust HTTP client for the Acteon action gateway.

## Complete platform API

The generated operation catalog exposes all 198 finite HTTP operations, including receipt sessions, managed stages, workflows, execution controls, inference profiles, and stream windows. Use an authenticated client and a configured, existing stage for this example:

```rust
let status = client.platform_request(
    acteon_client::PlatformOperation::BusStagesStatus,
    &[("namespace", "observability"), ("tenant", "demo"), ("id", "log-detector")],
    &[], None,
).await?;
```

Path parameters are escaped individually. Query and body fields use the server's wire names; JSON response envelopes are preserved. Calls do not retry automatically. Keep request IDs stable across retries and return opaque receipt IDs unchanged. HTTP access does not imply a code-defined workflow runner in every language.

See [SDK coverage and wire contracts](https://penserai.github.io/acteon/api/sdk-coverage/) for return types, native streaming APIs, runtime availability, and server feature requirements. The catalog is generated from the registered server routes and checked in CI.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
acteon-client = { path = "../acteon-client" }  # or from registry
acteon-core = { path = "../acteon-core" }      # for Action type
```

## Quick Start

```rust
use acteon_client::ActeonClient;
use acteon_core::Action;

#[tokio::main]
async fn main() -> Result<(), acteon_client::Error> {
    // Create a client
    let client = ActeonClient::new("http://localhost:8080");

    // Check server health
    if client.health().await? {
        println!("Server is healthy");
    }

    // Dispatch an action
    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_notification",
        serde_json::json!({
            "to": "user@example.com",
            "subject": "Hello",
            "body": "World"
        }),
    );

    let outcome = client.dispatch(&action).await?;
    println!("Outcome: {:?}", outcome);

    Ok(())
}
```

## Features

### Action Dispatch

```rust
// Single action
let outcome = client.dispatch(&action).await?;

// Batch dispatch
let actions = vec![action1, action2, action3];
let results = client.dispatch_batch(&actions).await?;

for result in results {
    match result {
        BatchResult::Success(outcome) => println!("Success: {:?}", outcome),
        BatchResult::Error { error } => println!("Error: {}", error.message),
    }
}
```

### Rule Management

```rust
// List all rules
let rules = client.list_rules().await?;
for rule in rules {
    println!("{}: priority={}, enabled={}", rule.name, rule.priority, rule.enabled);
}

// Reload rules from disk
let result = client.reload_rules().await?;
println!("Loaded {} rules", result.loaded);

// Enable/disable a rule
client.set_rule_enabled("block-spam", false).await?;
```

### Audit Trail

```rust
use acteon_client::AuditQuery;

// Query audit records
let query = AuditQuery {
    tenant: Some("tenant-1".to_string()),
    limit: Some(10),
    ..Default::default()
};

let page = client.query_audit(&query).await?;
println!("Found {} records (total: {})", page.records.len(), page.total);

// Get specific record
if let Some(record) = client.get_audit_record("action-id").await? {
    println!("Found: {:?}", record);
}
```

## Configuration

Use the builder pattern for advanced configuration:

```rust
use acteon_client::ActeonClientBuilder;
use std::time::Duration;

let client = ActeonClientBuilder::new("http://localhost:8080")
    .timeout(Duration::from_secs(60))
    .api_key("your-api-key")
    .build()?;
```

API keys are sent via the `Authorization: Bearer <key>` header. The server
accepts both JWTs and raw API keys on that header. API keys are scoped by
tenant, namespace, provider, and action type on the server side — see the
[API Key Scoping](https://penserai.github.io/acteon/features/api-key-scoping/)
documentation for the grant model and hierarchical tenant matching.

### Custom reqwest Client

For advanced HTTP configuration (TLS, proxies, etc.):

```rust
use reqwest::Client;
use acteon_client::ActeonClientBuilder;

let http_client = Client::builder()
    .danger_accept_invalid_certs(true)  // for testing
    .build()?;

let client = ActeonClientBuilder::new("https://localhost:8443")
    .client(http_client)
    .build()?;
```

## Error Handling

```rust
use acteon_client::Error;

match client.dispatch(&action).await {
    Ok(outcome) => println!("Success: {:?}", outcome),
    Err(e) => {
        if e.is_retryable() {
            println!("Retryable error: {}", e);
        } else if let Some(code) = e.api_code() {
            println!("API error [{}]: {}", code, e);
        } else {
            println!("Error: {}", e);
        }
    }
}
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

### Error Types

| Error | Description | Retryable |
|-------|-------------|-----------|
| `Connection` | Network failure | Yes |
| `Http { status, message }` | HTTP error | 5xx only |
| `Api { code, message, retryable }` | Server error | Depends |
| `Deserialization` | Parse error | No |
| `Configuration` | Client setup error | No |

## License

Apache-2.0

## Dispatch outcomes

All clients recognize the server's 17 dispatch variants, including `Grouped`, `StateChanged`, `PendingApproval`, `ChainStarted`, `CircuitOpen`, `RecurringCreated`, `Silenced`, and `Muted`. Single and batch dispatch preserve their fields, including approval capabilities and chain IDs. A pending approval or a started chain is not a completed provider execution; inspect the outcome before advancing an agent workflow.

## Execution-only credentials

Configure runtime API keys or JWT users with `role = "executor"` in the server's
`auth.toml`, and limit their namespace, tenant, provider and action grants.
Use the client's existing API-key or bearer-token configuration; the role is
established by the server, not by client-supplied action fields. Administrative
methods return HTTP 403 for executor credentials. Keep operator credentials and
signed human approval URLs in a trusted host.
See [authentication and role boundaries](https://penserai.github.io/acteon/api/authentication/).

## Execution permits

When the server enables execution authority, effectful dispatch requires explicit
references to permits already issued to the authenticated principal. These
options select permits; the server checks current credentials, scope, revisions,
qualified effects and budget before execution. They do not issue permits or
replace authentication. Existing dispatch methods omit the header by default.

```rust
use acteon_client::PermitReference;
let permits = [PermitReference { id: "maya-incident".into(), accepted_revision: 1 }];
let outcome = client.dispatch_with_permits(&action, &permits).await?;
let batch = client.dispatch_batch_with_permits(&[action], &permits).await?;
```

The same references apply to every action in a batch. Dry runs can omit them.
Missing/invalid headers, authorization denials and conflicting replay requests
preserve their HTTP status in the client's HTTP error type. Permit admission
refusals can also be returned as a `Failed` action outcome; inspect the outcome.
Do not generate a new action ID merely to bypass a replay conflict.

## Authenticated individual-agent services

These native helpers target a configured governed agent service. The server must
have the service installed, and the client must retain the original requester
credential and invocation permits. This is a separate surface from tenant-level
legacy A2A tasks.

```rust
use acteon_core::{TaskMessage, TaskRole};

let message = TaskMessage::text("incident-42", TaskRole::User, "Notify the incident owner");
let receipt = client.agent_service_send_message("prod", "acme", "notifier", &message).await?;
let task = client.agent_service_get_task(&receipt).await?;
```

`AgentServiceReceipt` supports Serde persistence in host state. Observation uses
its original task identity, independently of mutable `receipt.task` data.

Acceptance does not prove that provider execution has completed. Read the task's
status and artifacts through the receipt. Its source context comes only from the
admission response header; never obtain it from model output or task metadata.
Keep it with the original route and task ID in host state, outside model messages.
It is not a bearer credential: the original requester must still authenticate.

The helpers do not add retry loops. After a lost response, explicitly reuse the
same message ID and unchanged input; a new ID represents new work. SDK default
transports reject service redirects. A custom transport must keep retries and
redirects disabled for this surface. HTTP failures preserve their status and
server error body. Observation never resumes or starts provider work.


### Stop future starts for an accepted agent-service task

Use the retained host receipt to restrict the original recipient execution subtree.
The helper sends that job's original identity and source context in one request:

```rust
let stopped = client.agent_service_stop_task(&receipt).await?;
println!("Future starts blocked: {}", stopped.future_starts_blocked);
println!("Provider intervention: {:?}", stopped.provider_abort);
```

The acknowledgement always confirms a durable restriction on future starts.
`provider_abort` separately reports `restricted_only`, `uncertain`, or finality
accepted by the qualified reconciliation verifier; it is absent when no registered
attempt needs intervention. A stop never refunds spent budget. Uncertain work retains
its capacity, and the returned task retains actual provider status and artifacts.
After a timeout or unavailable acknowledgement, retry this stop explicitly with the
same receipt. The helper never retries automatically or takes provenance from mutable
task data.


Governed registry metadata uses `registry_projection` and `mutate_registry` with the exported `GovernanceRegistry*` wire models. Preserve the same mutation request/change ID for explicit recovery. Helpers require matching completed applied receipts and never automatically retry.
