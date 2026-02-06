# Basic Usage Examples

## Example 1: Gateway with Deduplication and Suppression

This example builds a gateway with an in-memory backend, loads YAML rules, and demonstrates deduplication and suppression.

### Rules

```yaml title="rules/basic.yaml"
rules:
  - name: block-spam
    priority: 1
    description: "Block actions with spam action type"
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress

  - name: dedup-email
    priority: 10
    description: "Deduplicate email actions within 5 minutes"
    condition:
      all:
        - field: action.action_type
          eq: "send_email"
        - field: action.payload.to
          contains: "@"
    action:
      type: deduplicate
      ttl_seconds: 300
```

### Code

```rust
use std::sync::Arc;
use std::time::Duration;

use acteon_core::{Action, ActionOutcome};
use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_provider::{DynProvider, ProviderError};
use acteon_rules::RuleFrontend;
use acteon_rules_yaml::YamlFrontend;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use async_trait::async_trait;

// Define a mock provider
struct MockEmailProvider;

#[async_trait]
impl DynProvider for MockEmailProvider {
    fn name(&self) -> &str { "email" }

    async fn execute(&self, action: &Action)
        -> Result<acteon_core::ProviderResponse, ProviderError>
    {
        println!("Sending email to {}",
            action.payload.get("to")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"));
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({"sent": true}),
        ))
    }

    async fn health_check(&self) -> Result<(), ProviderError> { Ok(()) }
}

#[tokio::main]
async fn main() {
    // Load rules
    let frontend = YamlFrontend;
    let rules = frontend.parse_file("rules/basic.yaml")
        .expect("failed to parse rules");

    // Build gateway
    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .rules(rules)
        .provider(Arc::new(MockEmailProvider))
        .executor_config(ExecutorConfig {
            max_retries: 1,
            execution_timeout: Duration::from_secs(5),
            max_concurrent: 10,
            ..ExecutorConfig::default()
        })
        .build()
        .expect("failed to build gateway");

    // Scenario 1: Deduplication
    let email = Action::new(
        "notifications", "tenant-1", "email", "send_email",
        serde_json::json!({"to": "user@example.com", "subject": "Hello!"}),
    ).with_dedup_key("email-user@example.com-hello");

    let outcome1 = gateway.dispatch(email.clone(), None).await.unwrap();
    println!("First:  {:?}", outcome1);  // Executed

    let outcome2 = gateway.dispatch(email, None).await.unwrap();
    println!("Second: {:?}", outcome2);  // Deduplicated

    // Scenario 2: Suppression
    let spam = Action::new(
        "notifications", "tenant-1", "email", "spam",
        serde_json::json!({"to": "victim@example.com"}),
    );

    let outcome3 = gateway.dispatch(spam, None).await.unwrap();
    println!("Spam:   {:?}", outcome3);  // Suppressed

    // Print metrics
    let m = gateway.metrics().snapshot();
    println!("Dispatched: {}, Executed: {}, Dedup: {}, Suppressed: {}",
        m.dispatched, m.executed, m.deduplicated, m.suppressed);
}
```

### Running

```bash
cargo run -p acteon-gateway --example basic
```

### Output

```
Sending email to user@example.com
First:  Executed(...)
Second: Deduplicated
Spam:   Suppressed { rule: "block-spam" }
Dispatched: 3, Executed: 1, Dedup: 1, Suppressed: 1
```

---

## Example 2: HTTP Server with curl

### Start Server

```bash
mkdir -p rules
# Create rules/basic.yaml with the rules above

cat > acteon.toml << 'EOF'
[rules]
directory = "./rules"
EOF

cargo run -p acteon-server -- -c acteon.toml
```

### Dispatch Actions

```bash
# Normal email — executes
curl -s -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {"to": "user@example.com"},
    "dedup_key": "demo-key"
  }' | jq .

# Same email — deduplicated
curl -s -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {"to": "user@example.com"},
    "dedup_key": "demo-key"
  }' | jq .

# Spam — suppressed
curl -s -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "spam",
    "payload": {"to": "victim@example.com"}
  }' | jq .

# Check metrics
curl -s http://localhost:8080/metrics | jq .

# List rules
curl -s http://localhost:8080/v1/rules | jq .
```

---

## Example 3: Batch Dispatch

```bash
curl -s -X POST http://localhost:8080/v1/dispatch/batch \
  -H "Content-Type: application/json" \
  -d '{
    "actions": [
      {
        "namespace": "notifications",
        "tenant": "tenant-1",
        "provider": "email",
        "action_type": "send_email",
        "payload": {"to": "alice@example.com", "subject": "Batch 1"}
      },
      {
        "namespace": "notifications",
        "tenant": "tenant-1",
        "provider": "email",
        "action_type": "send_email",
        "payload": {"to": "bob@example.com", "subject": "Batch 2"}
      },
      {
        "namespace": "notifications",
        "tenant": "tenant-2",
        "provider": "email",
        "action_type": "send_email",
        "payload": {"to": "charlie@example.com", "subject": "Batch 3"}
      }
    ]
  }' | jq .
```
