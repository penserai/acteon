# Quick Start

This guide walks you through running Acteon and dispatching your first action in under five minutes.

## 1. Start the Server

No external dependencies needed — the in-memory backend is the default:

```bash
cargo run -p acteon-server
```

You'll see:

```
INFO acteon_server: Starting Acteon server on 127.0.0.1:8080
INFO acteon_server: State backend: memory
INFO acteon_server: Swagger UI available at http://127.0.0.1:8080/swagger-ui/
```

## 2. Check Health

```bash
curl http://localhost:8080/health
```

```json
{
  "status": "ok",
  "metrics": {
    "dispatched": 0,
    "executed": 0,
    "deduplicated": 0,
    "suppressed": 0,
    "rerouted": 0,
    "throttled": 0,
    "failed": 0
  }
}
```

## 3. Dispatch an Action

Without any rules loaded, all actions are executed directly:

```bash
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {
      "to": "user@example.com",
      "subject": "Hello from Acteon!",
      "body": "Your first action was dispatched successfully."
    }
  }'
```

Response:

```json
{
  "outcome": "executed",
  "response": {
    "status": "success",
    "body": {}
  }
}
```

!!! note
    Without a registered email provider, the server uses a no-op default provider. To connect real providers, configure them in your server setup code or use the built-in integrations (Email, Slack).

## 4. Add Rules

Create a rules directory and add your first rule file:

```bash
mkdir -p rules
```

```yaml title="rules/basic.yaml"
rules:
  - name: dedup-emails
    priority: 10
    description: "Deduplicate email sends within 5 minutes"
    condition:
      all:
        - field: action.action_type
          eq: "send_email"
        - field: action.payload.to
          contains: "@"
    action:
      type: deduplicate
      ttl_seconds: 300

  - name: block-test-emails
    priority: 1
    description: "Block emails to test addresses"
    condition:
      field: action.payload.to
      ends_with: "@test.example.com"
    action:
      type: suppress
```

Now start the server with rules:

```bash
cargo run -p acteon-server -- -c acteon.toml
```

With the config:

```toml title="acteon.toml"
[rules]
directory = "./rules"
```

## 5. Test Deduplication

Send the same action twice:

```bash
# First dispatch — executes
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {"to": "user@example.com"},
    "dedup_key": "welcome-user@example.com"
  }'

# Second dispatch — deduplicated!
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {"to": "user@example.com"},
    "dedup_key": "welcome-user@example.com"
  }'
```

The second request returns:

```json
{
  "outcome": "deduplicated"
}
```

## 6. Test Suppression

```bash
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {"to": "qa@test.example.com"}
  }'
```

Response:

```json
{
  "outcome": "suppressed",
  "rule": "block-test-emails"
}
```

## 7. Check Metrics

```bash
curl http://localhost:8080/metrics
```

```json
{
  "dispatched": 3,
  "executed": 1,
  "deduplicated": 1,
  "suppressed": 1,
  "rerouted": 0,
  "throttled": 0,
  "failed": 0
}
```

## 8. Explore the Swagger UI

Open [http://localhost:8080/swagger-ui/](http://localhost:8080/swagger-ui/) in your browser to interactively explore all API endpoints with full request/response schemas.

## 9. Batch Dispatch

Send multiple actions at once:

```bash
curl -X POST http://localhost:8080/v1/dispatch/batch \
  -H "Content-Type: application/json" \
  -d '{
    "actions": [
      {
        "namespace": "notifications",
        "tenant": "tenant-1",
        "provider": "email",
        "action_type": "send_email",
        "payload": {"to": "alice@example.com"}
      },
      {
        "namespace": "notifications",
        "tenant": "tenant-1",
        "provider": "email",
        "action_type": "send_email",
        "payload": {"to": "bob@example.com"}
      }
    ]
  }'
```

## What's Next?

- [Configuration Reference](configuration.md) — all TOML config options
- [Architecture](../concepts/architecture.md) — how Acteon works internally
- [Features](../features/index.md) — explore every feature in detail
