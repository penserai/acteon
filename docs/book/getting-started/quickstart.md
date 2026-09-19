# Quick Start

Run these commands from the repository root after [building from source](installation.md).
You need Rust 1.88+, Cargo, and curl. The first build can take several minutes.

## 1. Start the Server

Use the included configuration to select in-memory state, two example rules,
and a **log provider** named `email`. It records successful dispatches in the
server log without sending email or contacting an external service.

```bash
cargo run --locked -p acteon-server -- -c examples/quickstart/acteon.toml
```

Keep the server running, and use a second terminal for the requests below.
The explicit config avoids loading the repository's separate `acteon.toml` demo.
The server listens on `127.0.0.1:8080`; no authentication is configured for this
local walkthrough. Stop it with Ctrl-C when finished.

The included rules suppress recipients ending in `@test.example.com` before
deduplicating `send_email` actions for 300 seconds. See
`examples/quickstart/rules/basic.yaml` to edit them, then restart the server.
In-memory state and deduplication history are cleared on restart.

## 2. Check Health

<!-- quickstart-check: health -->
```bash
curl --fail-with-body --silent --show-error "${ACTEON_URL:-http://127.0.0.1:8080}/health"
```

The JSON response has `"status": "ok"` and a `metrics` object.
`ACTEON_URL` is optional; set it if you start the server on a different port.

## 3. Dispatch an Action

Raw HTTP actions require an `id` and `created_at` in addition to namespace,
tenant, provider, action type, and payload. SDK constructors supply these fields
for you. This example uses a fixed ID and timestamp for reproducibility; use a
fresh UUID and current UTC timestamp for new actions in your application.

<!-- quickstart-check: dispatch -->
```bash
curl --fail-with-body --silent --show-error "${ACTEON_URL:-http://127.0.0.1:8080}/v1/dispatch" \
  -H 'Content-Type: application/json' \
  -d '{
    "id": "550e8400-e29b-41d4-a716-446655440001",
    "created_at": "2026-01-01T00:00:00Z",
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {"to": "user@example.com"},
    "dedup_key": "welcome-user@example.com"
  }'
```

Response:

```json
{"Executed":{"status":"success","body":{"provider":"email","logged":true},"headers":{}}}
```

There is no automatic fallback provider: `email` works here because the example
configuration explicitly registers it as a log provider. Configure a real
integration before using Acteon to send notifications.

## 4. Test Deduplication

Run the same dispatch command again within 300 seconds. It returns the JSON string:

```json
"Deduplicated"
```

## 5. Test Suppression

<!-- quickstart-check: suppression -->
```bash
curl --fail-with-body --silent --show-error "${ACTEON_URL:-http://127.0.0.1:8080}/v1/dispatch" \
  -H 'Content-Type: application/json' \
  -d '{
    "id": "550e8400-e29b-41d4-a716-446655440002",
    "created_at": "2026-01-01T00:00:00Z",
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {"to": "qa@test.example.com"}
  }'
```

Response:

```json
{"Suppressed":{"rule":"block-test-emails"}}
```

## 6. Check Metrics

<!-- quickstart-check: metrics -->
```bash
curl --fail-with-body --silent --show-error "${ACTEON_URL:-http://127.0.0.1:8080}/metrics"
```

After exactly the three dispatches above, `dispatched` is 3, `executed` is 1,
`deduplicated` is 1, and `suppressed` is 1. Other counters are also included.

## 7. Explore the API

Open [Swagger UI](http://127.0.0.1:8080/swagger-ui/) in your browser, or fetch
the OpenAPI document:

<!-- quickstart-check: openapi -->
```bash
curl --fail-with-body --silent --show-error "${ACTEON_URL:-http://127.0.0.1:8080}/api-doc/openapi.json"
```

## 8. Batch Dispatch

The batch endpoint accepts a JSON **array**, with a complete action in each entry.

<!-- quickstart-check: batch -->
```bash
curl --fail-with-body --silent --show-error "${ACTEON_URL:-http://127.0.0.1:8080}/v1/dispatch/batch" \
  -H 'Content-Type: application/json' \
  -d '[
    {
      "id": "550e8400-e29b-41d4-a716-446655440003",
      "created_at": "2026-01-01T00:00:00Z",
      "namespace": "notifications",
      "tenant": "tenant-1",
      "provider": "email",
      "action_type": "send_email",
      "payload": {"to": "alice@example.com"},
      "dedup_key": "welcome-alice@example.com"
    },
    {
      "id": "550e8400-e29b-41d4-a716-446655440004",
      "created_at": "2026-01-01T00:00:00Z",
      "namespace": "notifications",
      "tenant": "tenant-1",
      "provider": "email",
      "action_type": "send_email",
      "payload": {"to": "bob@example.com"},
      "dedup_key": "welcome-bob@example.com"
    }
  ]'
```

The response is an array containing two `Executed` results.

## Verify This Walkthrough

CI executes the marked curl blocks above against a fresh server, repeats the
dispatch for deduplication, and asserts outcomes and metrics. Run the same check:

```bash
cargo build --locked -p acteon-server
python3 scripts/ci/quickstart.py --server target/debug/acteon-server
```

## What's Next?

- [Configuration Reference](configuration.md) — TOML config options
- [Architecture](../concepts/architecture.md) — how Acteon works internally
- [Features](../features/index.md) — explore features in detail
