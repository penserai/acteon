# Authentication

Acteon supports API key and JWT-based authentication with role-based access control.

## Enabling Authentication

```toml title="acteon.toml"
[auth]
enabled = true
config_path = "auth.toml"
watch = true                    # Hot-reload on file changes
```

## Authentication Methods

### API Key

Include the API key in the request header:

```bash
curl -H "Authorization: Bearer your-api-key" http://localhost:8080/v1/dispatch
```

### JWT Token

Obtain a JWT via the login endpoint:

```bash
# Login
curl -X POST http://localhost:8080/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "secret"}'
```

Response:

```json
{
  "token": "eyJhbGciOiJIUzI1NiIs...",
  "expires_in": 3600
}
```

Use the token for subsequent requests:

```bash
curl -H "Authorization: Bearer eyJhbGciOiJIUzI1NiIs..." \
  http://localhost:8080/v1/dispatch
```

### Logout

Revoke a JWT token:

```bash
curl -X POST http://localhost:8080/v1/auth/logout \
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiIs..."
```

## Hot Reload

When `watch = true`, changes to the auth configuration file are automatically detected and applied without server restart:

```toml
[auth]
enabled = true
config_path = "auth.toml"
watch = true
```

Existing JWT sessions use the current user's role and grants after reload.
Removing a user rejects that user's existing sessions. API-key changes also
apply on subsequent requests. Reload does not stop a provider call already
in flight or establish authorization for each step of an existing chain.

## Execution and administration roles

Use `executor` for agent runtimes and services that submit actions. Keep
`operator` and `admin` credentials for trusted administration.

| Role | Scoped action dispatch | Policy and registry administration | Observation |
|---|---|---|---|
| `admin` | Yes | Yes | Yes |
| `operator` | Yes | Yes | Yes |
| `executor` | Yes | No | Subject to endpoint grants |
| `viewer` | No | No | Subject to endpoint grants |

Roles set an endpoint permission ceiling. Grants separately constrain tenant,
namespace, provider and action type; a wildcard action grant cannot give an
executor management permissions. The server checks a reviewed permission
inventory before protected handlers run. New operations fail closed until
their permission assignment is registered and verified in CI.

For example, add a hashed API key to `auth.toml`:

```toml
[[api_keys]]
name = "diagnostic-runtime"
key_hash = "REPLACE_WITH_SHA256_OF_YOUR_KEY"
role = "executor"

[[api_keys.grants]]
tenants = ["acme"]
namespaces = ["operations"]
providers = ["diagnostics"]
actions = ["investigate"]
```

The key can submit `diagnostics/investigate` actions in that scope. It cannot
reload rules, increase quotas, edit chain definitions, register agents or
endpoints, change agent administrative state, approve bus tool calls, or
control managed stages. A denied endpoint returns HTTP 403 before parsing its
request body or performing its operation.

Execution also includes granted bus publication, existing conversation
messaging, and A2A task submission/continuation/cancellation. These require
their existing provider/action grants: `bus/publish`, `bus/agent`,
`bus/conversation`, and `a2a/rpc`, as applicable. Bus heartbeat by an executor
is restricted to its own grant-bound `agent_id`. Subscription consumption
uses `bus/subscribe` and remains available to roles with StreamSubscribe.
Registry/card/schema administration and A2A push-notification configuration
mutation require a management role, over REST and JSON-RPC alike.

Workflow starts, queue operations, schedule changes, execution signals/resets,
group flushing, dead-letter draining, and embedding preview are currently
operator operations. Their authorization needs additional execution-level
provenance before they can safely become executor tools. Dispatching a
configured chain remains supported; the role boundary does not add per-step
permit checks or delegated authority. Grants on a parent tenant continue to
include its dotted child tenants.

Keep signed approval URLs in a trusted host and give them only to the intended
human reviewer. They are capabilities with a separate authentication contract;
an agent receiving an approval URL can use it regardless of its role. The
[agent coordination guide](../guides/agent-swarm-coordination.md) runs a verified
example with an execution-only credential.

## Security Features

- **Password hashing** — Argon2 for secure password storage
- **JWT signing** — HMAC-SHA256 for token integrity
- **Token revocation** — Immediate logout support
- **HMAC-signed approval URLs** — Tamper-proof approval/rejection links
- **Role-based access control** — Grant-level authorization

## Approval URL Signing

Approval URLs are HMAC-signed with configurable keys:

```toml
[server]
# approval_hmac_keys = [
#   { kid = "key-1", secret = "base64-encoded-secret" }
# ]
```

The signature includes namespace, tenant, approval ID, action (approve/reject), and expiration timestamp.
