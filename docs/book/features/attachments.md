# Attachments

Attachments let you include files with action dispatches. Email providers send
them as MIME attachments, Slack and Discord upload them as files, and webhook
providers include them as multipart form parts. Providers that don't support
attachments simply ignore them.

## Attachment Model

Each attachment carries five fields:

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | User-defined identifier for referencing this attachment in chains and templates |
| `name` | string | Human-readable display name |
| `filename` | string | Filename with extension (e.g. `"report.pdf"`) |
| `content_type` | string | MIME type (e.g. `"application/pdf"`) |
| `data_base64` | string | Base64-encoded file content |

The `id` field is set by you, not auto-generated. Use it to reference specific
attachments across chain steps, sub-chains, and template profiles.

## Dispatching with Attachments

Include attachments in the `attachments` array of a dispatch request:

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
      "subject": "Monthly Report",
      "body": "Please find the report attached."
    },
    "attachments": [
      {
        "id": "monthly-report",
        "name": "Monthly Report",
        "filename": "report-2026-02.pdf",
        "content_type": "application/pdf",
        "data_base64": "JVBERi0xLjQK..."
      },
      {
        "id": "summary-csv",
        "name": "Summary Data",
        "filename": "summary.csv",
        "content_type": "text/csv",
        "data_base64": "bmFtZSxhbW91bnQK..."
      }
    ]
  }'
```

## Provider Behavior

Each provider handles resolved attachments differently:

### Email (SMTP / SES)

Attachments are sent as RFC 2045 MIME parts inside a `multipart/mixed`
envelope. The `filename` and `content_type` map directly to the MIME part
headers. This is how standard email clients display file attachments.

### Slack

Attachments are uploaded as files using Slack's file upload API. Each
attachment appears as a downloadable file in the channel.

### Discord

Attachments are sent as file uploads in the Discord message. Each attachment
appears as a downloadable file in the channel.

### Webhook

Attachments are included as `multipart/form-data` parts alongside the JSON
payload. Each part uses the `filename` as the form field name.

### Other Providers

Providers that return `false` from `supports_attachments()` ignore attachments
silently. The action is dispatched normally with just the payload.

## Resource Limits

The gateway enforces two limits on attachments:

| Limit | Default | Description |
|-------|---------|-------------|
| `max_attachments_per_action` | 10 | Maximum number of attachments per dispatch |
| `max_inline_bytes` | 10 MB | Maximum decoded size per attachment |

These are configured in the server's `[attachments]` section:

```toml
[attachments]
max_inline_bytes = 10485760    # 10 MB
max_attachments = 10
```

The gateway validates both limits after decoding the base64 content. Actions
that exceed either limit are rejected with an `ATTACHMENT_ERROR` outcome.

## Template Integration

Attachment metadata is available inside [payload templates](payload-templates.md)
via two context variables:

### `attachments` -- List

An ordered list of attachment metadata objects (without `data_base64`):

```jinja
{% for att in attachments %}
- {{ att.filename }} ({{ att.content_type }})
{% endfor %}
```

Each object has four fields: `id`, `name`, `filename`, `content_type`.

### `attachments_by_id` -- Map

A map from attachment `id` to the same metadata, for direct lookup:

```jinja
Report file: {{ attachments_by_id["monthly-report"].filename }}
```

### Conditional rendering

```jinja
{% if attachments %}
{{ attachments | length }} file(s) attached:
{% for att in attachments %}
  - {{ att.name }} ({{ att.filename }})
{% endfor %}
{% else %}
No files attached.
{% endif %}
```

Binary content (`data_base64`) is intentionally excluded from the template
context to prevent multi-megabyte strings inside the rendering engine.

## Audit Trail

Attachment metadata (not binary data) is recorded in the audit trail for every
dispatched action. Each audit record includes an `attachment_metadata` array:

```json
{
  "attachment_metadata": [
    {
      "id": "monthly-report",
      "name": "Monthly Report",
      "filename": "report-2026-02.pdf",
      "content_type": "application/pdf",
      "size_bytes": 142857
    }
  ]
}
```

The `size_bytes` field reflects the decoded binary size (the actual file size),
not the base64-encoded string length.

## Client SDK Usage

### Rust

```rust
use acteon_core::{Action, Attachment};

let action = Action::new("notifications", "tenant-1", "email", "send_email",
    serde_json::json!({"to": "user@example.com", "subject": "Report"}),
)
.with_attachments(vec![
    Attachment {
        id: "report".into(),
        name: "Monthly Report".into(),
        filename: "report.pdf".into(),
        content_type: "application/pdf".into(),
        data_base64: base64_encoded_data,
    },
]);
```

### Python

```python
from acteon_client import ActeonClient, Attachment

client = ActeonClient("http://localhost:8080")
client.dispatch(
    namespace="notifications",
    tenant="tenant-1",
    provider="email",
    action_type="send_email",
    payload={"to": "user@example.com", "subject": "Report"},
    attachments=[
        Attachment(
            id="report",
            name="Monthly Report",
            filename="report.pdf",
            content_type="application/pdf",
            data_base64=base64_encoded_data,
        ),
    ],
)
```

### Node.js / TypeScript

```typescript
import { ActeonClient, Attachment } from "acteon-client";

const client = new ActeonClient("http://localhost:8080");
await client.dispatch({
  namespace: "notifications",
  tenant: "tenant-1",
  provider: "email",
  actionType: "send_email",
  payload: { to: "user@example.com", subject: "Report" },
  attachments: [
    {
      id: "report",
      name: "Monthly Report",
      filename: "report.pdf",
      content_type: "application/pdf",
      data_base64: base64EncodedData,
    },
  ],
});
```

### Go

```go
client := acteon.NewClient("http://localhost:8080")
client.Dispatch(ctx, acteon.DispatchRequest{
    Namespace:  "notifications",
    Tenant:     "tenant-1",
    Provider:   "email",
    ActionType: "send_email",
    Payload:    map[string]any{"to": "user@example.com", "subject": "Report"},
    Attachments: []acteon.Attachment{
        acteon.NewAttachment("report", "Monthly Report", "report.pdf",
            "application/pdf", base64EncodedData),
    },
})
```

### Java

```java
ActeonClient client = new ActeonClient("http://localhost:8080");
client.dispatch(DispatchRequest.builder()
    .namespace("notifications")
    .tenant("tenant-1")
    .provider("email")
    .actionType("send_email")
    .payload(Map.of("to", "user@example.com", "subject", "Report"))
    .attachments(List.of(
        new Attachment("report", "Monthly Report", "report.pdf",
            "application/pdf", base64EncodedData)
    ))
    .build());
```
