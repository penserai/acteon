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

## Shared authentication authority across replicas

For a replicated Redis deployment, enable a shared configuration authority to
prevent a server with an older auth file from accepting credentials under
obsolete roles or grants. Each logical auth file has one source ID and an
explicit monotonically increasing version. Every replica uses the same source,
control scope, Redis key prefix and fingerprint key.

```toml title="acteon.toml"
[state]
backend = "redis"
url = "redis://localhost:6379"
prefix = "acteon"

[auth]
enabled = true
config_path = "auth.toml"
watch = true

[auth.authority]
namespace = "auth-control"
tenant = "deployment"
source_id = "workforce-auth"
bootstrap = false
```

Add a positive **top-level** version to the decrypted auth file, before any
TOML table headers. Every user and API key in this mode needs a stable principal
binding as described below.

```toml title="auth.toml"
authority_revision = 1

[settings]
jwt_secret = "REPLACE_WITH_YOUR_JWT_SECRET"
jwt_expiry_seconds = 3600

# Existing [[users]] and [[api_keys]] entries follow.
```

Set `ACTEON_AUTH_AUTHORITY_KEY` to a separate random secret of at least 32 bytes,
in addition to the existing `ACTEON_AUTH_KEY` used for decryption. Keep the
fingerprint key consistent across replicas. The coordinator stores a keyed
fingerprint of the decrypted security configuration; it does not store the
password/key hashes, signing secret, role/grant contents or raw credentials.

On the first reviewed startup only, set `bootstrap = true` to initialize the
dedicated control coordinator. Then set it to `false` for normal startup.
Missing, deleted or unsupported state fails closed in normal operation. Keep
this scope separate from execution coordinators and other auth sources; resetting
it is recovery work, not an ordinary reload operation.

A reload publishes the whole source version before installing its local tables.
Increment `authority_revision` for changes to roles, grants, principal bindings,
credentials or JWT settings. Equivalent reordered definitions can reuse the
current version. Different content at that version conflicts, and an older
version is refused. A replica using old tables denies authenticated requests
until it installs the current configuration. A failed or lost publication
acknowledgment can be reconciled by retrying the identical version/content;
previous local tables cannot authenticate if the newer version already committed.

Login, JWT validation and API-key lookup check source freshness and current
principal disablement. Existing JWT sessions still refresh from the current
user's role/grants; password changes alone do not revoke all existing sessions.
Removed users are denied. JWT signing settings remain fixed during a process's
lifetime: changes require a restart with the new configuration version.

This guard observes authority when authenticating a request. Requests already
authenticated may be in flight, and queued work does not inherit an ongoing
freshness guarantee. It does not install execution permits, terminal credential
retirement, team mandates or per-effect enforcement. Those require their own
execution integration. The control record is bounded and retains publication
history; publication at capacity is refused and needs reviewed recovery.

The [server configuration page](../admin-ui/index.md) displays whether shared
configuration authority is configured. All five SDKs continue using their
existing authentication headers and the generic `config_get_config` operation;
no client-supplied authority reference is accepted.

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

## Stable principals and credential rotation

Bind a credential to a stable actor with an optional `principal` entry in
`auth.toml`. Credential names identify keys or login accounts; the principal
identifies the person, agent, or service using them.

```toml
[[api_keys]]
name = "diagnostic-key-2026-10"
key_hash = "<sha256-of-your-key>"
role = "executor"
principal = { id = "diagnostic-agent", kind = "agent" }

[[api_keys.grants]]
tenants = ["production"]
namespaces = ["observability"]
providers = ["diagnostics"]
actions = ["inspect"]
```

The same entry is supported on `[[users]]`. Kinds are `human`, `agent`, `service`,
and `system`; they describe the actor and grant no privileges. Keep the principal
ID and kind unchanged when rotating a key or changing its credential name. Each
credential still needs its own role and scoped grants. An agent principal does
not automatically create a registry entry or bind a bus sender; bus identity
still requires the configured grant's `agent_id`.

Inspect the current binding with an authenticated request:

```bash
curl http://localhost:8080/v1/auth/identity \
  -H "Authorization: Bearer $ACTEON_API_KEY"
```

```json
{
  "credential_id": "diagnostic-key-2026-10",
  "auth_method": "api_key",
  "role": "executor",
  "principal": { "id": "diagnostic-agent", "kind": "agent" }
}
```

All roles can inspect their own identity. The response contains no key hash or
secret. Existing configurations remain valid and return `principal: null` when
unbound; anonymous development mode also has no principal binding.

Durable dispatch receipts and chains retain the originally authenticated
principal and credential provenance. For bound callers, reusing an idempotency
key after credential rotation resolves the same semantic request under the same
principal, while preserving the original receipt. A different principal or
changed payload conflicts. Current transport authorization still applies, so a
narrower replacement credential cannot use a receipt to bypass its grants.
Enabling a binding on an old unbound request changes its semantic caller; use a
new idempotency key or explicitly reconcile the old work.

Use principal-aware server versions on every replica processing bound work.
Drain or park bound work before downgrading: older chain readers can discard
principal metadata, and older receipt readers cannot validate bound digests.

JWT sessions are bound to the principal at issuance. Changing or removing a
user's binding invalidates its existing sessions; log in again after an
intentional binding migration. Role/grant reloads under the same binding apply
to subsequent requests. Duplicate usernames, duplicate API-key hashes, invalid
roles, and conflicting kinds for one principal ID reject a reload atomically,
leaving the previous configuration active.

| SDK | Inspect the current binding |
|---|---|
| Rust | `client.identity().await?` |
| Python | `client.identity()` or `await client.identity()` |
| TypeScript | `await client.identity()` |
| Go | `client.Identity(ctx)` |
| Java | `client.identity()` |

The returned principal is identity metadata. Execution permits, delegated
authority, and reauthorization at each deferred effect remain separate platform
capabilities; this binding does not establish those guarantees.

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
