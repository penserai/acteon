# API Key Scoping

Acteon API keys and JWT users are authorized via a list of **grants**. Each
grant specifies which tenants, namespaces, providers, and action types the
principal is allowed to dispatch, audit, or replay. A request is allowed
when **every** dimension on at least one of the caller's grants matches the
action.

This page covers:

- The grant model (tenant / namespace / provider / action-type)
- **Hierarchical tenant matching** — a grant on `acme` also covers `acme.us-east`
- How to define scoped API keys in `auth.toml`
- How SDKs and `curl` authenticate against the gateway

## The grant model

A grant looks like this:

```toml
[[api_keys.grants]]
tenants    = ["acme"]
namespaces = ["notifications"]
providers  = ["email", "sms"]
actions    = ["send_email", "send_sms"]
```

An action is authorized iff:

1. The grant's `tenants` list contains `"*"`, an exact match for the action's
   tenant, or a **parent tenant** (see hierarchical matching below), **and**
2. The grant's `namespaces` list contains `"*"` or an exact match for the
   action's namespace, **and**
3. The grant's `providers` list contains `"*"` or an exact match for the
   action's provider, **and**
4. The grant's `actions` list contains `"*"` or an exact match for the
   action's `action_type`.

A caller may have multiple grants. Each action is checked against every
grant in order, and the first one that matches authorizes the request.

### The four dimensions

| Dimension | Meaning | Wildcard |
|-----------|---------|----------|
| `tenants` | Tenant IDs (or parent-tenant prefixes for hierarchical matching) | `"*"` |
| `namespaces` | Namespace IDs | `"*"` |
| `providers` | Provider IDs (e.g. `"email"`, `"sms"`, `"slack"`, `"webhook"`) | `"*"` |
| `actions` | Action type strings (e.g. `"send_email"`, `"create_ticket"`) | `"*"` |

The `providers` field defaults to `["*"]` when omitted, so existing
`auth.toml` files written before provider scoping was added continue to
work unchanged.

## Hierarchical tenant matching

Tenant IDs support hierarchical scoping via dotted notation. A grant on
tenant `"acme"` automatically covers any tenant starting with `acme.` —
including `acme.us-east`, `acme.us-east.prod`, `acme.eu-west`, and so on.

This lets operators write a single grant for a parent organization rather
than enumerating every region or environment.

### Examples

| Grant pattern | Action tenant | Matches? |
|---------------|---------------|----------|
| `"acme"` | `"acme"` | ✅ exact |
| `"acme"` | `"acme.us-east"` | ✅ hierarchical (child) |
| `"acme"` | `"acme.us-east.prod"` | ✅ hierarchical (grandchild) |
| `"acme"` | `"acme-corp"` | ❌ no dot separator |
| `"acme"` | `"acmecorp"` | ❌ no dot separator |
| `"acme.us-east"` | `"acme"` | ❌ child grant does not cover parent |
| `"acme.us-east"` | `"acme.eu-west"` | ❌ siblings do not cover each other |

Matching is **one-way**: a grant scoped to a child (e.g. `"acme.us-east"`)
cannot dispatch actions for the parent (`"acme"`) or sibling regions
(`"acme.eu-west"`).

Matching is also **dot-strict**: a grant on `"acme"` will not match
`"acme-corp"` or `"acmecorp"`. The character immediately after the pattern
must be `.` for hierarchical matching to apply.

## Defining scoped API keys

Keys live in `auth.toml` (path configured via `[auth].config_path` in
`acteon.toml`; defaults to `auth.toml` relative to `acteon.toml`):

```toml
# ─── auth.toml ────────────────────────────────────────────

[settings]
jwt_secret = "ENC[AES256-GCM,...]"    # Encrypt via `acteon-server encrypt`
jwt_expiry_seconds = 3600

# ─── Admin user with full access ─────────────────────────
[[users]]
username = "admin"
password_hash = "ENC[AES256-GCM,...]"  # Argon2 hash, then encrypted
role = "admin"
[[users.grants]]
tenants    = ["*"]
namespaces = ["*"]
providers  = ["*"]
actions    = ["*"]

# ─── Scoped API key for a single product team ────────────
[[api_keys]]
name = "acme-notifications-team"
key_hash = "ENC[AES256-GCM,...]"       # SHA-256 hex of raw key, then encrypted
role = "operator"

# Team can dispatch email and sms from any sub-tenant of "acme".
[[api_keys.grants]]
tenants    = ["acme"]                  # Covers acme, acme.us-east, acme.eu-west, ...
namespaces = ["notifications"]
providers  = ["email", "sms"]
actions    = ["send_email", "send_sms"]

# ─── Scoped API key for a single region ──────────────────
[[api_keys]]
name = "acme-us-east-oncall"
key_hash = "ENC[AES256-GCM,...]"
role = "operator"
[[api_keys.grants]]
tenants    = ["acme.us-east"]          # Only acme.us-east and its children
namespaces = ["alerts"]
providers  = ["pagerduty", "slack"]
actions    = ["*"]

# ─── Read-only auditor ───────────────────────────────────
[[api_keys]]
name = "compliance-auditor"
key_hash = "ENC[AES256-GCM,...]"
role = "viewer"                         # Viewer role → no dispatch permission
[[api_keys.grants]]
tenants    = ["*"]
namespaces = ["*"]
providers  = ["*"]
actions    = ["*"]
```

### Generating a key

1. Generate a raw API key — e.g., `openssl rand -hex 32`.
2. Compute its SHA-256 hash — e.g., `printf '%s' $KEY | sha256sum`.
3. Encrypt the hash with `acteon-server encrypt` (reads from stdin) and paste
   the resulting `ENC[...]` value into `key_hash`.
4. Distribute the **raw key** to the caller (never the hash).

`auth.toml` is hot-reloaded: changes to API keys and users are picked up
without restarting the server. The JWT secret is immutable after startup
to avoid invalidating active sessions.

## Authenticating

Acteon accepts API keys via two equivalent mechanisms. Both are
recommended for operational tooling.

### `Authorization: Bearer` (preferred)

This is what the SDKs send by default. Gateway accepts both JWTs and raw
API keys here — it tries JWT validation first, and falls back to API-key
lookup on failure.

```bash
curl -H "Authorization: Bearer $ACTEON_API_KEY" \
     -H "Content-Type: application/json" \
     -d '{"namespace":"notifications","tenant":"acme","provider":"email","action_type":"send_email","payload":{}}' \
     https://acteon.example.com/v1/dispatch
```

### `X-API-Key` (legacy)

Still supported for curl examples, scripts, and tools that reserve the
`Authorization` header for other purposes:

```bash
curl -H "X-API-Key: $ACTEON_API_KEY" \
     -H "Content-Type: application/json" \
     -d '...' \
     https://acteon.example.com/v1/dispatch
```

### SDK usage

All five polyglot SDKs accept an API key and send it via the
`Authorization: Bearer` header.

```rust
// Rust
use acteon_client::ActeonClient;
let client = ActeonClient::builder("http://localhost:8080")
    .api_key("my-raw-key")
    .build()?;
```

```python
# Python
from acteon_client import ActeonClient
client = ActeonClient("http://localhost:8080", api_key="my-raw-key")
```

```typescript
// Node.js
import { ActeonClient } from "@acteon/client";
const client = new ActeonClient("http://localhost:8080", { apiKey: "my-raw-key" });
```

```go
// Go
import "github.com/acteon/acteon/clients/go/acteon"
client := acteon.NewClient("http://localhost:8080", acteon.WithAPIKey("my-raw-key"))
```

```java
// Java
ActeonClient client = new ActeonClient("http://localhost:8080", "my-raw-key");
```

## Enforcement points

Grants are enforced at the server ingress on every request that mutates
or reads scoped data:

| Endpoint | What's checked |
|----------|----------------|
| `POST /v1/dispatch` | Each action against the full `(tenant, namespace, provider, action_type)` tuple |
| `POST /v1/dispatch/batch` | Every action in the batch; **one failure rejects the whole batch** |
| `GET /v1/audit/{id}` | The audit record's tenant/namespace/provider/action_type |
| `POST /v1/audit/{id}/replay` | Same as dispatch |
| `POST /v1/audit/replay` | Each record individually; out-of-scope records are skipped |
| `GET /v1/analytics` | Tenant filter auto-injected for single-tenant callers |
| `GET /v1/rules/coverage` | Tenant filter auto-injected for single-tenant callers |

Role-based permissions (admin / operator / viewer) are enforced separately
and orthogonally — a viewer cannot dispatch even with a matching grant, and
an operator cannot reload rules even with `actions = ["*"]`.

## Related features

- [Compliance Mode](compliance-mode.md) — tamper-evident audit chain
- [Audit Trail](audit-trail.md) — the scoped read surface
- [Rule Coverage](rule-coverage.md) — tenant-scoped coverage analysis
