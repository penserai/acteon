# Polyglot Clients

Acteon provides official client SDKs for multiple programming languages. All clients support dispatch, batch operations, rule management, and audit querying.

## Available Clients

| Language | Package | Location |
|----------|---------|----------|
| **Rust** | `acteon-client` | `crates/client/` |
| **Python** | `acteon-client` | `clients/python/` |
| **Node.js/TypeScript** | `@acteon/client` | `clients/nodejs/` |
| **Go** | `github.com/penserai/acteon/clients/go/acteon` | `clients/go/` |
| **Java** | `com.acteon:acteon-client` | `clients/java/` |

## Quick Examples

=== "Rust"

    ```rust
    use acteon_client::ActeonClient;
    use acteon_core::Action;

    let client = ActeonClient::new("http://localhost:8080");
    let action = Action::new(
        "ns", "tenant", "email", "send",
        json!({"to": "user@example.com"}),
    );
    let outcome = client.dispatch(&action).await?;
    ```

=== "Python"

    ```python
    from acteon import ActeonClient, Action

    client = ActeonClient("http://localhost:8080")
    action = Action("ns", "tenant", "email", "send", {"to": "user@example.com"})
    outcome = client.dispatch(action)
    ```

=== "TypeScript"

    ```typescript
    import { ActeonClient, createAction } from "@acteon/client";

    const client = new ActeonClient("http://localhost:8080");
    const action = createAction("ns", "tenant", "email", "send", {
      to: "user@example.com",
    });
    const outcome = await client.dispatch(action);
    ```

=== "Go"

    ```go
    import "github.com/penserai/acteon/clients/go/acteon"

    client := acteon.NewClient("http://localhost:8080")
    action := acteon.NewAction("ns", "tenant", "email", "send",
        map[string]any{"to": "user@example.com"})
    outcome, err := client.Dispatch(ctx, action)
    ```

=== "Java"

    ```java
    import com.acteon.ActeonClient;
    import com.acteon.Action;

    ActeonClient client = new ActeonClient("http://localhost:8080");
    Action action = new Action("ns", "tenant", "email", "send",
        Map.of("to", "user@example.com"));
    ActionOutcome outcome = client.dispatch(action);
    ```

## Common Features

All clients support:

| Feature | Description |
|---------|-------------|
| **Health check** | Verify server availability |
| **Single dispatch** | Dispatch one action |
| **Batch dispatch** | Dispatch multiple actions |
| **Rule listing** | List loaded rules |
| **Rule reload** | Reload rules from disk |
| **Rule toggle** | Enable/disable rules |
| **Audit query** | Search audit records |
| **Audit lookup** | Get record by action ID |
| **Webhook helpers** | Convenience builders for webhook actions |

## Webhook Helpers

All clients provide convenience types and factory functions for creating webhook-targeted actions without manually constructing the payload.

=== "Rust"

    ```rust
    use acteon_client::webhook;

    // Simple webhook
    let action = webhook::action("notifications", "tenant-1")
        .url("https://hooks.example.com/alert")
        .body(serde_json::json!({"message": "Alert fired"}))
        .build();

    // With all options
    let action = webhook::action("notifications", "tenant-1")
        .url("https://hooks.example.com/alert")
        .method("PUT")
        .body(serde_json::json!({"severity": "critical"}))
        .header("X-Api-Key", "secret")
        .dedup_key("alert-123")
        .build();
    ```

=== "Python"

    ```python
    from acteon_client import create_webhook_action, WebhookPayload

    # Simple webhook
    action = create_webhook_action(
        namespace="notifications",
        tenant="tenant-1",
        url="https://hooks.example.com/alert",
        body={"message": "Alert fired"},
    )

    # With all options
    action = create_webhook_action(
        namespace="notifications",
        tenant="tenant-1",
        url="https://hooks.example.com/alert",
        body={"severity": "critical"},
        method="PUT",
        headers={"X-Api-Key": "secret"},
        dedup_key="alert-123",
    )
    ```

=== "TypeScript"

    ```typescript
    import { createWebhookAction } from "@acteon/client";

    // Simple webhook
    const action = createWebhookAction(
      "notifications", "tenant-1",
      "https://hooks.example.com/alert",
      { message: "Alert fired" }
    );

    // With all options
    const action = createWebhookAction(
      "notifications", "tenant-1",
      "https://hooks.example.com/alert",
      { severity: "critical" },
      {
        method: "PUT",
        headers: { "X-Api-Key": "secret" },
        dedupKey: "alert-123",
      }
    );
    ```

=== "Go"

    ```go
    // Simple webhook
    action := acteon.NewWebhookAction(
        "notifications", "tenant-1",
        "https://hooks.example.com/alert",
        map[string]any{"message": "Alert fired"},
    )

    // With all options
    action := acteon.NewWebhookActionWithOptions(
        "notifications", "tenant-1",
        "https://hooks.example.com/alert", "PUT",
        map[string]any{"severity": "critical"},
        map[string]string{"X-Api-Key": "secret"},
    ).WithDedupKey("alert-123")
    ```

=== "Java"

    ```java
    // Simple webhook
    Action action = WebhookAction.create(
        "notifications", "tenant-1",
        "https://hooks.example.com/alert",
        Map.of("message", "Alert fired")
    );

    // With all options
    Action action = WebhookAction.builder()
        .namespace("notifications")
        .tenant("tenant-1")
        .url("https://hooks.example.com/alert")
        .method("PUT")
        .body(Map.of("severity", "critical"))
        .header("X-Api-Key", "secret")
        .dedupKey("alert-123")
        .build();
    ```

## Testing with Polyglot Simulation

The `polyglot_client_simulation` example tests all language clients against a running server:

```bash
cargo run -p acteon-simulation --example polyglot_client_simulation
```

This starts an in-memory server and runs each client's test suite, verifying compatibility across all languages.

### Prerequisites

| Language | Requirements |
|----------|-------------|
| Python | Python 3.11+, `httpx` package |
| Node.js | Node.js 18+, `npm install` in `clients/nodejs` |
| Go | Go 1.22+ |
| Java | Java 21+, jbang (optional) |
