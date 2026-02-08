# Event Streaming

Real-time event streaming lets dashboards, monitoring tools, and automation
subscribe to action outcomes as they happen -- without polling.

Acteon exposes a **Server-Sent Events (SSE)** endpoint at `GET /v1/stream`
that pushes events as actions flow through the gateway pipeline.

## How It Works

1. A client opens an SSE connection to `GET /v1/stream`
2. As actions are dispatched, the gateway broadcasts events to all subscribers
3. Events are filtered **server-side** based on the caller's tenant grants and
   optional query-parameter filters
4. Each event is delivered as a JSON-encoded SSE frame

```
event: action_dispatched
id: 550e8400-e29b-41d4-a716-446655440000
data: {"id":"550e8400-...","timestamp":"2026-02-07T14:30:00Z","type":"action_dispatched","outcome":{"Executed":{"status":"Success","body":null,"headers":{}}},"provider":"email","namespace":"alerts","tenant":"acme","action_type":"send_email","action_id":"661f9511-..."}

event: group_flushed
id: 771f0622-f30c-52e5-b827-557766551111
data: {"id":"771f0622-...","timestamp":"2026-02-07T14:30:05Z","type":"group_flushed","group_id":"grp-abc","event_count":5,"namespace":"alerts","tenant":"acme"}
```

## Event Types

| SSE `event:` tag | Description |
|------------------|-------------|
| `action_dispatched` | An action was processed through the dispatch pipeline |
| `group_flushed` | A batch of grouped events was flushed |
| `timeout` | A state machine timeout fired |
| `chain_advanced` | A task chain step was advanced |
| `approval_required` | An action requires human approval |
| `lagged` | Warning: the client fell behind and events were skipped |

## Query Parameters

All filters are optional. When multiple filters are specified, they are
combined with AND logic.

| Parameter | Type | Description |
|-----------|------|-------------|
| `namespace` | string | Only receive events from this namespace |
| `action_type` | string | Only receive events for this action type |
| `outcome` | string | Only receive dispatch events with this outcome category (e.g., `executed`, `suppressed`, `failed`, `throttled`, `rerouted`, `deduplicated`) |
| `event_type` | string | Only receive events of this type (e.g., `action_dispatched`, `group_flushed`, `timeout`, `chain_advanced`, `approval_required`) |

## Authentication

The SSE endpoint sits behind the standard auth layer. Authenticate with
a Bearer token or API key, the same as any other protected endpoint:

```bash
curl -N -H "Authorization: Bearer <token>" \
  "http://localhost:8080/v1/stream?namespace=alerts"
```

## Tenant Isolation

Events are filtered based on the caller's tenant grants:

- **Scoped callers** only receive events for their authorized tenants
- **Wildcard callers** (admin) receive events for all tenants
- Tenant filtering happens server-side before events reach the client

## Connection Limits

To prevent resource exhaustion, a configurable per-tenant limit on concurrent
SSE connections is enforced (default: 10). When the limit is reached, new
connections receive HTTP 429.

Configure in the server config:

```yaml
server:
  max_sse_connections_per_tenant: 10
```

## Security

Stream events are **sanitized** before broadcast:

- `ProviderResponse.body` is replaced with `null` (may contain PII or secrets)
- `ProviderResponse.headers` are cleared (may contain auth tokens)
- Approval URLs are redacted to `[redacted]` (HMAC-signed tokens)

This means SSE events carry enough metadata for monitoring (outcome status,
provider, timing) without exposing sensitive payload data.

## Backpressure

When a slow client can't keep up with the event rate, it receives a special
`lagged` event indicating how many events were dropped:

```
event: lagged
data: {"skipped":42}
```

The stream continues from the current position. This is preferable to
disconnecting the client entirely.

## Reconnection

SSE has built-in reconnection support. Each event carries a unique `id` field.
On reconnect, browsers and SSE-capable clients send the `Last-Event-ID` header
automatically.

Note: Events that occurred between disconnect and reconnect are **not**
replayed from a buffer. For gap-free event history, use the
[Audit Trail](audit-trail.md) API.

## Keep-Alive

The server sends a keep-alive comment every 15 seconds to prevent
proxy/load-balancer timeout disconnections.

## Examples

### curl

```bash
# All events
curl -N -H "Authorization: Bearer <token>" \
  http://localhost:8080/v1/stream

# Only executed email actions in the alerts namespace
curl -N -H "Authorization: Bearer <token>" \
  "http://localhost:8080/v1/stream?namespace=alerts&action_type=send_email&outcome=executed"

# Only group flush and timeout events
curl -N -H "Authorization: Bearer <token>" \
  "http://localhost:8080/v1/stream?event_type=group_flushed"
```

### Rust

```rust
use acteon_client::{ActeonClient, StreamFilter};
use futures::StreamExt;

let client = ActeonClient::new("http://localhost:8080")
    .with_bearer_token("my-token");

let filter = StreamFilter::new()
    .namespace("alerts")
    .action_type("send_email");

let mut stream = client.stream(&filter).await?;

while let Some(item) = stream.next().await {
    match item? {
        acteon_client::StreamItem::Event(event) => {
            println!("Event: {} in {}", event.id, event.namespace);
        }
        acteon_client::StreamItem::Lagged { skipped } => {
            eprintln!("Warning: missed {skipped} events");
        }
        acteon_client::StreamItem::KeepAlive => {}
    }
}
```

### Python

```python
import requests

url = "http://localhost:8080/v1/stream"
headers = {"Authorization": "Bearer <token>"}
params = {"namespace": "alerts", "outcome": "failed"}

with requests.get(url, headers=headers, params=params, stream=True) as resp:
    for line in resp.iter_lines(decode_unicode=True):
        if line.startswith("data:"):
            import json
            event = json.loads(line[5:].strip())
            print(f"Event: {event['id']} - {event['type']}")
```

### Node.js

```javascript
import { EventSource } from "eventsource";

const es = new EventSource(
  "http://localhost:8080/v1/stream?namespace=alerts",
  { headers: { Authorization: "Bearer <token>" } }
);

es.addEventListener("action_dispatched", (e) => {
  const event = JSON.parse(e.data);
  console.log(`Dispatched: ${event.action_id} -> ${event.provider}`);
});

es.addEventListener("lagged", (e) => {
  const { skipped } = JSON.parse(e.data);
  console.warn(`Missed ${skipped} events`);
});
```

### Go

```go
req, _ := http.NewRequest("GET", "http://localhost:8080/v1/stream?namespace=alerts", nil)
req.Header.Set("Authorization", "Bearer <token>")

resp, _ := http.DefaultClient.Do(req)
defer resp.Body.Close()

scanner := bufio.NewScanner(resp.Body)
for scanner.Scan() {
    line := scanner.Text()
    if strings.HasPrefix(line, "data:") {
        data := strings.TrimPrefix(line, "data:")
        fmt.Println("Event:", strings.TrimSpace(data))
    }
}
```

## Limitations

- **Live monitoring only**: Events that occur while no client is connected are
  not persisted. Use the [Audit Trail](audit-trail.md) for historical data.
- **Process-local**: In multi-instance deployments, SSE connections only see
  events from the instance they're connected to. Use sticky sessions at the
  load balancer level.
- **HTTP/1.1 browser limit**: Browsers allow at most 6 concurrent SSE
  connections per origin. This is not a limitation for server-side clients.
- **No binary support**: Events are JSON-encoded text. Binary payloads are
  not supported via SSE.
