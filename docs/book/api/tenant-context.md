# Credential-derived tenant context

Every authenticated user and service key belongs to exactly one namespace and
tenant. The server attaches that context to actions, timeline changes, queries,
resource lookups, and streams. API callers provide business inputs and resource
IDs; they never supply tenant metadata to select which records to access.

## Provision credentials

Set `scope` on every `[[users]]` and `[[api_keys]]` entry in the encrypted auth
configuration. A scope must contain an exact, nonempty namespace and tenant.
Wildcards and path separators are invalid. At least one grant must cover the
configured scope.

```toml
[settings]
jwt_secret = "REPLACE_WITH_YOUR_JWT_SECRET"
jwt_expiry_seconds = 3600

[[users]]
username = "operator"
password_hash = "REPLACE_WITH_ARGON2_HASH"
role = "operator"
scope = { namespace = "payments", tenant = "bank-a" }
[[users.grants]]
namespaces = ["payments"]
tenants = ["bank-a"]
providers = ["*"]
actions = ["*"]

[[api_keys]]
name = "transaction-pipeline"
key_hash = "REPLACE_WITH_SHA256_OF_YOUR_KEY"
role = "executor"
scope = { namespace = "payments", tenant = "bank-a" }
[[api_keys.grants]]
namespaces = ["payments"]
tenants = ["bank-a"]
providers = ["*"]
actions = ["*"]
```

Grants restrict operations within this exact context. A wildcard or hierarchical
parent grant never expands a credential into another namespace or tenant. Use
separate provisioned credentials for services that operate in multiple tenants.
The JWT scope is signed, required, and checked against the current user entry.
Changing a user's scope invalidates previously issued tokens.

`GET /v1/auth/identity` returns the authenticated context:

```json
{
  "credential_id": "transaction-pipeline",
  "auth_method": "api_key",
  "role": "executor",
  "scope": { "namespace": "payments", "tenant": "bank-a" },
  "principal": null
}
```

## Request contract

Dispatch an action using only its business fields:

```bash
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Authorization: Bearer $ACTEON_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"id":"review-42","created_at":"2026-10-25T12:00:00Z","provider":"email","action_type":"send_email","payload":{"to":"ops@example.com","subject":"Review required"}}'
```

Query timeline events within the same authenticated context:

```bash
curl -X POST http://localhost:8080/v1/timeline/query \
  -H "Authorization: Bearer $ACTEON_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query":{"eventGroup":"deployment-42","filter":"attributes.region == \"eu-west-1\"","limit":50}}'
```

Pagination cursors and resource IDs remain query inputs. Namespace, tenant,
`scope`, and `tenant_scope` are rejected as query parameters or root scope fields
on tenant request bodies, even if they equal the credential context. Tenant URL
segments have been removed: for example, agent services use
`/a2a/agents/{agent}/v1/message:send` and bus subscriptions use
`/v1/bus/subscriptions/{id}`. Approval decisions use authenticated operator or
admin credentials at `/v1/approvals/{id}/approve` or `/reject`.

Response records still contain ownership metadata. Arbitrary payloads and
custom timeline attributes can contain business fields named `tenant` or
`namespace`; the server preserves those values without treating them as
authority. Signed federation declarations and retained receipts likewise keep
their independently verified provenance.

The admin UI displays the authenticated context as read-only information.
Streaming requests carry the same authorization header as ordinary API calls.

## Deployment changes

This is a breaking contract. Provision a scope on every user and key, issue new
JWTs, remove tenant metadata from request bodies and query strings, and update
resource URLs and SDK calls. There is no fallback to request-selected tenants or
JWTs without scope claims.

With authentication disabled, all HTTP requests use the fixed `default/default`
context. Tenant selection is still unavailable. Trusted internal gateway calls
and configured ingest sources retain explicit context because the server owns
those bindings; an event payload cannot override them.
