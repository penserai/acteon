# Agentic Bus — Phase 3

> **Scope:** JSON-Schema registry, publish-edge validation, typed SDK
> helpers. Agents, conversations, and tool-call envelopes come later.
> See the [master plan](../concepts/bus-master-plan.md).

Phase 1 gave us topics + publish + SSE subscribe. Phase 2 added durable
subscriptions with ack, lag, and DLQ. Phase 3 moves Acteon up from
"arbitrary JSON on the wire" to a schema-aware control plane: register
a schema, bind a topic to it, and let the publish edge enforce the
contract for you.

## What ships in Phase 3

| Surface | Shape |
|---|---|
| Core type | `acteon_core::Schema` — `{subject, version, namespace, tenant, format, body, labels, created_at}` |
| State | `KeyKind::BusSchema`; key id = `"{subject}:{version}"` |
| Validator | `acteon_bus::SchemaValidator` — compiled-schema cache keyed by `(namespace, tenant, subject, version)`. Uses the `jsonschema` crate (draft 2020-12). |
| HTTP | `POST/GET /v1/bus/schemas`, `GET /v1/bus/schemas/{ns}/{t}/{subject}`, `GET|DELETE /v1/bus/schemas/{ns}/{t}/{subject}/{version}`, `PUT|DELETE /v1/bus/topics/{ns}/{t}/{name}/schema` |
| Rust client | `register_bus_schema`, `list_bus_schemas`, `get_bus_schema_versions`, `get_bus_schema`, `delete_bus_schema`, `bind_topic_schema`, `unbind_topic_schema`, `publish_typed` |
| Tests | 7 validator unit tests (register/validate/versioning/remove/compile-fail) |
| Simulation | `bus_schema_simulation.rs` — registers v1 and v2, demonstrates acceptance, rejection-with-paths, and version independence |

## Model

- **Subjects are logical names.** `"orders"`, `"tool-calls"`,
  `"agent-events"`. Scoped per `(namespace, tenant)`.
- **Versions are monotonic integers, assigned by the server.**
  `POST /v1/bus/schemas` with `subject = "orders"` picks `max(version) + 1`
  (or 1 if none exist). Schemas are immutable once registered.
- **Topics opt into validation.** A `Topic` has
  `schema_subject: Option<String>` and `schema_version: Option<i32>`.
  Bound → validate on every publish; unbound → no validation.
- **No compatibility checker in V1.** `POST` accepts any body that
  compiles as a JSON Schema. Backward/forward compat enforcement is a
  future concern; ship what we need now.

## API shape

### Register a schema version

```http
POST /v1/bus/schemas
{
  "subject": "orders",
  "namespace": "agents",
  "tenant": "demo",
  "body": {
    "type": "object",
    "required": ["id", "qty"],
    "properties": {
      "id": {"type": "string"},
      "qty": {"type": "integer", "minimum": 1}
    }
  }
}
→ 201 SchemaResponse { subject: "orders", version: 1, ... }
```

Post the same subject again and you get version 2. The server compiles
the body first — a malformed JSON-Schema document returns `400` before
touching state.

### Bind a topic

```http
PUT /v1/bus/topics/agents/demo/orders/schema
{ "subject": "orders", "version": 1 }
→ 200 { "topic": "agents.demo.orders", "subject": "orders", "version": 1 }
```

A topic can bind to any registered version. Pin to a specific version
for strict control; pass `"latest"` on the SDK's `get_bus_schema` if
you want to read the latest but still pin the binding explicitly.

### Publish validation

Once bound, every publish against that topic runs through the compiled
validator. Conforming payloads produce as before. Non-conforming
payloads return `400`:

```http
POST /v1/bus/publish
{
  "topic": "agents.demo.orders",
  "payload": { "id": "ord-1" }
}
→ 400 {
  "error": "payload does not match schema 'orders' v1",
  "subject": "orders",
  "version": 1,
  "issues": [
    { "path": "", "message": "\"qty\" is a required property" }
  ]
}
```

Up to 10 issues are returned per response. `path` is a JSON Pointer
into the payload (e.g. `/items/3/qty`), so clients can pinpoint the
offending field without re-running a local validator.

### Unbind

```http
DELETE /v1/bus/topics/agents/demo/orders/schema
→ 204
```

Subsequent publishes bypass validation. The schema object itself is
not deleted — call `DELETE /v1/bus/schemas/...` if you want that too.

### Delete a schema version

```http
DELETE /v1/bus/schemas/agents/demo/orders/1
→ 204
```

Fails with `409 Conflict` if any topic currently pins this version.
Unbind first, then delete.

## SDK example

```rust
use acteon_client::{ActeonClient, RegisterBusSchema};

let client = ActeonClient::new("http://localhost:3000")?;

// Register.
let schema = client.register_bus_schema(&RegisterBusSchema {
    subject: "orders".to_string(),
    namespace: "agents".to_string(),
    tenant: "demo".to_string(),
    body: serde_json::json!({ "type": "object", "required": ["id"] }),
    ..Default::default()
}).await?;

// Bind.
client.bind_topic_schema("agents", "demo", "orders", "orders", schema.version).await?;

// Typed publish — serializes your value, validates, produces.
#[derive(serde::Serialize)]
struct Order { id: String, qty: i32 }

client.publish_typed(&acteon_client::PublishTyped {
    value: &Order { id: "ord-1".into(), qty: 2 },
    topic: Some("agents.demo.orders"),
    ..Default::default()
}).await?;
```

## Validator cache semantics

Compiled validators live in an in-memory cache on the server
(`AppState::bus_schema_validator`). On cold-start or after an explicit
`remove`, the publish path reads the schema row from state and
recompiles on demand. This means:

- **State is the source of truth.** Restart the server and validation
  keeps working.
- **Cache writes are lazy.** The first publish after restart pays one
  recompile per subject; subsequent publishes hit the warm cache.
- **Unbinding doesn't invalidate the cache.** Binding a topic to a new
  subject simply stops consulting the old one.

## How to try it

```bash
# No broker required — the simulation runs entirely against the
# in-memory validator.
cargo run -p acteon-simulation --features bus --example bus_schema_simulation
```

For an end-to-end HTTP flow, start the server with the bus feature
enabled and use the SDK methods above.

## What comes next (Phase 4)

- `Agent` type — identity, capabilities, heartbeat, and the shared
  inbox topic keyed by `agent_id`.
- Per-agent schema selection for inbound tool-call envelopes.
