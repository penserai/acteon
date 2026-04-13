# Action Signing

Acteon supports **Ed25519 action signing** so dispatched actions carry a cryptographic `signature` and `signer_id` that downstream systems and compliance auditors can verify without trusting the gateway.

The feature is fully opt-in. Deployments without a `[signing]` section in the TOML config operate exactly as before — the two new fields on actions are always optional.

## How it works

1. The **client** computes the action's **canonical bytes** — a compact, sorted-key JSON serialization of every field except `signature` and `signer_id`.
2. The client signs the canonical bytes with its Ed25519 secret key and sets `signature` (base64) + `signer_id` on the action.
3. The **server** verifies the signature against its keyring, optionally enforces tenant/namespace scope restrictions, and optionally rejects replays via action-ID deduplication.
4. The **audit record** stores the `signature`, `signer_id`, and a `canonical_hash` (SHA-256 of the canonical bytes) for post-hoc verification.

## TOML configuration

```toml
[signing]
enabled = true
reject_unsigned = false          # true = reject actions without valid signature
reject_replay = false            # true = reject action IDs that have been seen before
replay_ttl_seconds = 86400       # TTL for replay-protection entries (default 24h)
server_key = "ENC[AES256-GCM,data:...]"   # Ed25519 secret for server-originated actions
server_signer_id = "acteon-server"         # signer_id for server key

[[signing.keyring]]
signer_id = "ci-bot"
public_key = "base64-or-hex-encoded-ed25519-public-key"
tenants = ["acme"]                   # optional scope restriction (default: ["*"])
namespaces = ["prod", "staging"]     # optional scope restriction (default: ["*"])

[[signing.keyring]]
signer_id = "deploy-service"
public_key = "..."
# tenants/namespaces omitted = allow all
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `enabled` | No | `false` | Master switch for signing |
| `reject_unsigned` | No | `false` | Reject actions without a valid `signature` + `signer_id` |
| `reject_replay` | No | `false` | Reject action IDs already dispatched (uses state store) |
| `replay_ttl_seconds` | No | `86400` | TTL for replay-protection entries |
| `server_key` | No | — | Ed25519 secret key for server-originated actions. Supports `ENC[...]` |
| `server_signer_id` | No | `"acteon-server"` | `signer_id` stamped on server-originated signatures |
| `keyring[].signer_id` | Yes | — | Must match the `signer_id` field on incoming actions |
| `keyring[].public_key` | Yes | — | Ed25519 public key (hex or base64) |
| `keyring[].tenants` | No | `["*"]` | Allowed tenants for this signer |
| `keyring[].namespaces` | No | `["*"]` | Allowed namespaces for this signer |

## Canonicalization

The canonical byte representation is the input to the Ed25519 signature. It is computed as:

1. Serialize the action to a JSON object.
2. Remove the `signature` and `signer_id` keys.
3. Collect the remaining keys into a sorted map (lexicographic order).
4. Serialize to **compact JSON** (no whitespace).

This format is designed for cross-language reproducibility: any JSON library that can emit compact, sorted-key JSON produces identical bytes.

```python
# Python example — computing canonical bytes
import json

action = {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "namespace": "prod",
    "tenant": "acme",
    "provider": "email",
    "action_type": "send",
    "payload": {"to": "user@example.com"},
    "created_at": "2026-04-12T00:00:00Z",
    "metadata": {}
}
# Remove signing fields, sort keys, compact
canonical = json.dumps(action, sort_keys=True, separators=(",", ":")).encode()
```

## Dispatch payload

A signed action includes the two extra fields:

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "namespace": "prod",
  "tenant": "acme",
  "provider": "email",
  "action_type": "send",
  "payload": {"to": "user@example.com"},
  "created_at": "2026-04-12T00:00:00Z",
  "signature": "base64-encoded-64-byte-ed25519-signature",
  "signer_id": "ci-bot"
}
```

## Verification endpoint

```
GET /v1/actions/{id}/verify
```

Looks up the audit record by action ID and returns:

```json
{
  "verified": true,
  "signer_id": "ci-bot",
  "algorithm": "Ed25519",
  "canonical_hash": "sha256-hex-of-canonical-bytes"
}
```

Callers can independently verify by computing `canonical_bytes` on the original action, hashing with SHA-256, and comparing to `canonical_hash`.

## Error behavior

| Scenario | HTTP status | Error message |
|----------|-------------|---------------|
| Invalid signature | 400 | `signature verification failed: invalid signature` |
| Unknown `signer_id` | 400 | `signature verification failed: unknown signer: X` |
| Signer not authorized for tenant/namespace | 400 | `signer 'X' is not authorized for tenant=Y namespace=Z` |
| Unsigned action + `reject_unsigned=true` | 400 | `unsigned action rejected: signing.reject_unsigned is enabled` |
| Replayed action ID + `reject_replay=true` | 409 | `replay rejected: action ID 'X' has already been dispatched` |

## Rust client

The Rust client supports signing via the `signing` feature flag:

```toml
[dependencies]
acteon-client = { version = "0.1", features = ["signing"] }
```

```rust
use acteon_client::ActeonClient;
use acteon_core::Action;
use acteon_crypto::signing::parse_signing_key;

let client = ActeonClient::new("http://localhost:8080");
let key = parse_signing_key("hex-or-base64-key", "ci-bot")?;
let action = Action::new("prod", "acme", "email", "send", payload);

let outcome = client.dispatch_signed(&action, &key).await?;
```

## Polyglot SDKs

The Python, Node.js, Go, and Java SDKs carry `signature` and `signer_id` as optional fields on the Action model for passthrough to the server. Client-side signing is not implemented in the polyglot SDKs — sign on the server side or use the Rust client.
