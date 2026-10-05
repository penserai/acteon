# Rust Client

The `acteon-client` crate provides a native Rust HTTP client for the Acteon API.

## Complete platform API

The generated operation catalog exposes all 193 finite HTTP operations, including receipt sessions, managed stages, workflows, execution controls, inference profiles, and stream windows. Use an authenticated client and a configured, existing stage for this example:

```rust
let status = client.platform_request(
    acteon_client::PlatformOperation::BusStagesStatus,
    &[("namespace", "observability"), ("tenant", "demo"), ("id", "log-detector")],
    &[], None,
).await?;
```

Path parameters are escaped individually. Query and body fields use the server's wire names; JSON response envelopes are preserved. Calls do not retry automatically. Keep request IDs stable across retries and return opaque receipt IDs unchanged. HTTP access does not imply a code-defined workflow runner in every language.

See [SDK coverage and wire contracts](https://penserai.github.io/acteon/api/sdk-coverage/) for return types, native streaming APIs, runtime availability, and server feature requirements. The catalog is generated from the registered server routes and checked in CI.

## Typed governance management

Use `governance`, `publish_governance_permit`, and `intervene_governance` to inspect permitted routes, issue bounded permits, and close/reopen resources or revoke permits, subjects, and credentials. Requests and responses have native typed models. These methods require an authenticated operator and an independently configured execution manager for the requested namespace and tenant; an executor credential alone does not grant management authority.

Calls preserve HTTP refusals and do not retry automatically. Keep `change_id` stable when reconciling a lost response. A receipt records persisted authority; closure affects subsequent starts and does not promise cancellation of work already in flight.

See [governance management](https://penserai.github.io/acteon/features/governance/) for deployment policy and wire examples, and [the governed city simulation](https://penserai.github.io/acteon/guides/governed-city/) for real authenticated requests.

## Installation

```toml title="Cargo.toml"
[dependencies]
acteon-client = { path = "crates/client" }
acteon-core = { path = "crates/core" }
```

## Quick Start

```rust
use acteon_client::ActeonClient;
use acteon_core::Action;

#[tokio::main]
async fn main() -> Result<(), acteon_client::Error> {
    let client = ActeonClient::new("http://localhost:8080");

    // Health check
    if client.health().await? {
        println!("Server is healthy");
    }

    // Dispatch
    let action = Action::new(
        "notifications", "tenant-1", "email", "send_email",
        serde_json::json!({"to": "user@example.com", "subject": "Hello"}),
    );
    let outcome = client.dispatch(&action).await?;
    println!("Outcome: {:?}", outcome);

    Ok(())
}
```

## Builder Configuration

```rust
use acteon_client::ActeonClientBuilder;
use std::time::Duration;

let client = ActeonClientBuilder::new("http://localhost:8080")
    .timeout(Duration::from_secs(60))
    .api_key("your-api-key")
    .build()?;
```

### Custom HTTP Client

```rust
let http_client = reqwest::Client::builder()
    .danger_accept_invalid_certs(true)
    .build()?;

let client = ActeonClientBuilder::new("https://localhost:8443")
    .client(http_client)
    .build()?;
```

## Methods

### Health & Metrics

```rust
let healthy = client.health().await?;
```

### Action Dispatch

```rust
// Single action
let outcome = client.dispatch(&action).await?;

// Batch dispatch
let results = client.dispatch_batch(&[action1, action2, action3]).await?;
for result in results {
    match result {
        BatchResult::Success(outcome) => println!("OK: {:?}", outcome),
        BatchResult::Error { error } => println!("Error: {}", error.message),
    }
}
```

### Rule Management

```rust
// List rules
let rules = client.list_rules().await?;
for rule in rules {
    println!("{}: priority={}, enabled={}", rule.name, rule.priority, rule.enabled);
}

// Reload from disk
let result = client.reload_rules().await?;
println!("Loaded {} rules", result.loaded);

// Enable/disable
client.set_rule_enabled("block-spam", false).await?;
```

### Audit Trail

```rust
use acteon_client::AuditQuery;

let page = client.query_audit(&AuditQuery {
    tenant: Some("tenant-1".into()),
    outcome: Some("executed".into()),
    limit: Some(100),
    ..Default::default()
}).await?;

if let Some(record) = client.get_audit_record("action-id").await? {
    println!("Found: {} -> {}", record.action_type, record.outcome);
}
```

### Events (State Machines)

```rust
// List events
let events = client.list_events(&EventQuery::default()).await?;

// Get event state
let event = client.get_event("fingerprint", "ns", "tenant").await?;

// Transition event
let result = client.transition_event("fingerprint", "acknowledged", "ns", "tenant").await?;
```

### Approvals

```rust
// Approve
client.approve("ns", "tenant-1", "approval-id", "signature", "expires").await?;

// Reject
client.reject("ns", "tenant-1", "approval-id", "signature", "expires").await?;

// List pending
let approvals = client.list_approvals("ns", "tenant-1").await?;

// Get status
let status = client.get_approval("ns", "tenant-1", "approval-id").await?;
```

### Event Groups

```rust
// List groups
let groups = client.list_groups().await?;

// Get details
let group = client.get_group("group-key").await?;

// Force flush
client.flush_group("group-key").await?;
```

### Event Streaming

```rust
use acteon_client::{StreamFilter, StreamItem};
use futures::StreamExt;

let filter = StreamFilter::new()
    .namespace("alerts")
    .action_type("send_email")
    .outcome("executed");

let mut stream = client.stream(&filter).await?;

while let Some(item) = stream.next().await {
    match item? {
        StreamItem::Event(event) => {
            println!("{}: {} in {}", event.id, event.namespace, event.tenant);
        }
        StreamItem::Lagged { skipped } => {
            eprintln!("Warning: missed {skipped} events");
        }
        StreamItem::KeepAlive => {} // Connection still alive
    }
}
```

See [Event Streaming](../features/event-streaming.md) for full documentation.

## Error Handling

```rust
use acteon_client::Error;

match client.dispatch(&action).await {
    Ok(outcome) => println!("OK: {:?}", outcome),
    Err(e) => {
        if e.is_retryable() {
            println!("Retryable: {}", e);
        } else if let Some(code) = e.api_code() {
            println!("API error [{}]: {}", code, e);
        } else {
            println!("Error: {}", e);
        }
    }
}
```

### Error Types

| Error | Retryable | Description |
|-------|-----------|-------------|
| `Connection` | Yes | Network failure |
| `Http { status, message }` | 5xx only | HTTP error |
| `Api { code, message, retryable }` | Depends | Server-reported error |
| `Deserialization` | No | Response parse error |
| `Configuration` | No | Client setup error |

## Method Reference

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
| `list_events(query)` | List events |
| `get_event(fp, ns, tenant)` | Get event state |
| `transition_event(fp, state, ns, tenant)` | Transition event |
| `approve(ns, tenant, id, sig, exp)` | Approve action |
| `reject(ns, tenant, id, sig, exp)` | Reject action |
| `list_approvals(ns, tenant)` | List pending approvals |
| `get_approval(ns, tenant, id)` | Get approval status |
| `list_groups()` | List event groups |
| `get_group(key)` | Get group details |
| `flush_group(key)` | Force flush group |
| `stream(filter)` | Subscribe to SSE event stream |

## Explicit permits

Use `PermitReference { id, accepted_revision }` with `dispatch_with_permits` or
`dispatch_batch_with_permits` when the server enables execution authority.
The client sends references in the permit header, separate from action metadata,
while retaining its configured credentials. See [Execution permits](../features/execution-permits.md)
for current backend, route and replay guarantees.

## Agent workforce

Typed workforce methods inspect teams, memberships, ownership, assignments and
mandates, and submit all ten workforce changes. They require independently
declared workforce management bounds and current operator authentication.
Public values do not establish representation or caller authority.

```rust
use acteon_client::{WorkforceChange, WorkforceChangeRequest, WorkforceTeam, TeamRef};

let roster = client.workforce("prod", "acme").await?;
let receipt = client.change_workforce(&WorkforceChangeRequest {
    namespace: "prod".into(), tenant: "acme".into(),
    change_id: "enroll-reliability-1".into(), reason: "Establish incident response".into(),
    change: WorkforceChange::PutTeam { team: WorkforceTeam {
        team: TeamRef::new("prod", "acme", "reliability")?, revision: 1, name: "Reliability".into(),
    } },
}).await?;
```

Preserve the exact request and change ID after a lost acknowledgment. Mutations
retain HTTP refusal status and are not automatically retried. See the
[workforce model and API](https://penserai.github.io/acteon/features/workforce/)
for represented permits, dependencies, revisions and intervention semantics.
