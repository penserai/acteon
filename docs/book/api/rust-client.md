# Rust Client

The `acteon-client` crate provides a native Rust HTTP client for the Acteon API.

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
