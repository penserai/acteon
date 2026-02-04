# acteon-client

Native Rust HTTP client for the Acteon action gateway.

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
