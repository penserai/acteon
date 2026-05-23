# Action Signing

Acteon supports **Ed25519 action signing** so dispatched actions carry a cryptographic `signature` and `signer_id` that downstream systems and compliance auditors can verify without trusting the gateway.

The feature is fully opt-in. Deployments without a `[signing]` section in the TOML config operate exactly as before — the two new fields on actions are always optional.

## How it works

1. The **client** computes the action's **canonical bytes** — a compact, sorted-key JSON serialization of every field except `signature` and `signer_id`.
2. The client signs the canonical bytes with its Ed25519 secret key and sets `signature` (base64) + `signer_id` on the action.
3. The **server** verifies the signature against its keyring, optionally enforces tenant/namespace scope restrictions, and optionally rejects replays via action-ID deduplication.
4. The **audit record** stores the `signature`, `signer_id`, and a `canonical_hash` (SHA-256 of the canonical bytes) for post-hoc verification.

## Bootstrapping with `acteon keys`

The `acteon` CLI has a `keys` subcommand that runs entirely
client-side and produces ready-to-paste TOML keyring entries. It's
the recommended bootstrap path — much less error-prone than piping
`openssl genpkey` and base64 by hand.

```text
$ acteon keys generate ci-bot --kid k1
# Acteon signing keypair
# signer_id = ci-bot
# kid       = k1

SECRET (capture into a secret manager NOW — printed once):
  2479b27ff6d897b985ff9fc8cf388e9067eeebabc463aa0964a82535f5ad7573

PUBLIC:
  2d991475ae2989b0febf8c9f1eb2f2c3d0e7b7b38b6ba3c6cd244639c375738a

# Append to your server config:
[[signing.keyring]]
signer_id = "ci-bot"
kid = "k1"
public_key = "LZkUda4pibD+v4yfHrLyw9Dnt7OLa6PGzSRGOcN1c4o="
# tenants = ["*"]      # uncomment to scope
# namespaces = ["*"]   # uncomment to scope
```

### Stream layout and `--secret-out`

The output is deliberately split across stdout and stderr so you can
pipe the config into a file without dragging the secret with it:

- **stdout**: only the ready-to-paste `[[signing.keyring]]` TOML
  block (public material only). Safe to redirect —
  `acteon keys generate ci-bot >> config.toml` appends the entry
  to your config.
- **stderr**: the human-readable header, the secret key (printed
  once), the public key, and the "append this" hint.

For automated / CI bootstrap where both streams may be log-captured,
pass `--secret-out <path>` and the secret is written to that file
(mode 0600 on Unix) without touching stdout or stderr at all:

```text
$ acteon keys generate ci-bot --kid k1 --secret-out /run/secrets/ci-bot.key
```

Capture that file into your secret manager, then paste the public
key block from stdout into the server config.

Two more subcommands, both read-only on disk:

| Command | Purpose |
|---|---|
| `acteon keys list <config.toml>` | Print every `[[signing.keyring]]` entry currently registered, including kid and scope. Useful before a rotation to see what's already there. |
| `acteon keys rotate <config.toml> --signer-id ci-bot` | Read the existing config, find the highest `kid` for the signer, generate a new keypair under the next sequential `kid`, and print the new TOML block to append. |

`rotate` never modifies the config file — operators paste the
output themselves so the config edit stays an explicit, auditable
step.

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
kid = "k1"                           # optional key id, defaults to "k0"
public_key = "base64-or-hex-encoded-ed25519-public-key"
tenants = ["acme"]                   # optional scope restriction (default: ["*"])
namespaces = ["prod", "staging"]     # optional scope restriction (default: ["*"])

# A second key for the same signer — staged ahead of a rotation.
# Both `k1` and `k2` are active until the operator removes `k1`.
[[signing.keyring]]
signer_id = "ci-bot"
kid = "k2"
public_key = "..."
tenants = ["acme"]
namespaces = ["prod", "staging"]

[[signing.keyring]]
signer_id = "deploy-service"
public_key = "..."
# tenants/namespaces/kid omitted = allow all + default kid "k0"
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
| `keyring[].kid` | No | `"k0"` | Key identifier within `signer_id`. Multiple entries with the same `signer_id` and different `kid`s enable staged rotation. |
| `keyring[].public_key` | Yes | — | Ed25519 public key (hex or base64) |
| `keyring[].tenants` | No | `["*"]` | Allowed tenants for this signer |
| `keyring[].namespaces` | No | `["*"]` | Allowed namespaces for this signer |

## Canonicalization

The canonical byte representation is the input to the Ed25519 signature. It is computed as:

1. Serialize the action to a JSON object.
2. Remove the `signature`, `signer_id`, and `kid` keys.
3. Collect the remaining keys into a sorted map (lexicographic order).
4. Serialize to **compact JSON** (no whitespace).

`kid` is excluded from the canonical bytes so that a signature
produced before a rotation stays valid against the same key after
the rotation — only the routing identifier changes, not the bytes
that were signed.

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

A signed action includes the signing fields:

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
  "signer_id": "ci-bot",
  "kid": "k2"
}
```

The `kid` field is optional. When present, the server selects the
exact `(signer_id, kid)` pair from its keyring — fail-fast on a
stale or never-issued kid. When absent, the server falls back to
trying every active key registered under `signer_id` and accepts
the first match. Legacy single-key signers can omit `kid` entirely.

## Key rotation

Acteon supports rotating an Ed25519 key without coordinated
downtime by allowing **multiple active keys per signer**. The
rotation pattern:

1. **Generate a new keypair** for the same `signer_id` with a fresh
   `kid` (e.g., `k2` if the current key is `k1`). The
   `acteon keys rotate <config.toml> --signer-id <signer>` command
   reads the existing config and picks the next sequential `kid`
   automatically.
2. **Add the new public key to `signing.keyring`** alongside the
   existing entry. Both keys are now active. Restart the server
   (or wait for the next config reload).
3. **Verify discovery** by hitting `GET /.well-known/acteon-signing-keys`
   — the response now contains two entries for the same `signer_id`.
4. **Migrate signers** to use the new private key + send `kid: "k2"`
   on the dispatch. Existing in-flight signatures stamped with
   `k1` (or no kid at all) continue to verify against `k1`.
5. **Wait** until the longest-lived in-flight signed action has
   been processed.
6. **Remove `k1`** from `signing.keyring` and restart. The
   discovery endpoint now reports only `k2`. Any signature still
   referencing `k1` is rejected.

The audit record stores both `signer_id` and `kid` on every
signed dispatch, so operators can trace which key produced any
given signature even after the rotation has completed.

## Discovery endpoint

```
GET /.well-known/acteon-signing-keys
```

Public (no authentication) endpoint that publishes the active
verifier set. Response shape:

```json
{
  "keys": [
    {
      "signer_id": "ci-bot",
      "kid": "k1",
      "algorithm": "Ed25519",
      "public_key": "base64-encoded-32-byte-public-key",
      "tenants": ["acme"],
      "namespaces": ["prod", "staging"]
    },
    {
      "signer_id": "ci-bot",
      "kid": "k2",
      "algorithm": "Ed25519",
      "public_key": "base64-encoded-32-byte-public-key",
      "tenants": ["acme"],
      "namespaces": ["prod", "staging"]
    }
  ],
  "count": 2
}
```

Use cases:

- **Side-loaded verification** — a downstream service can fetch
  the keyring at runtime instead of being deployed with hardcoded
  public keys.
- **Detect a rotation in progress** — when a signer has more than
  one entry in the response, the operator is staging a rotation;
  clients should start sending the new `kid`.
- **Audit verification without server cooperation** — clients can
  verify stored audit records against the current public set
  rather than calling `GET /v1/actions/{id}/verify`.

The endpoint never returns private key material. Operators who
prefer a private verifier set can disable signing globally
(`signing.enabled = false`) — the response then becomes an empty
list.

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

## Querying audit records by signer

`GET /v1/audit` accepts two optional query parameters that narrow
the result set by the signing metadata stored on each record:

| Parameter | Matches |
|---|---|
| `signer_id` | Records whose `signer_id` equals the given value. Unsigned records never match. |
| `kid` | Records whose `kid` equals the given value. Combine with `signer_id` to pin the query to a specific `(signer, key)` pair. Legacy signatures with no `kid` never match a `kid` filter. |

**Incident response example.** If a key tagged `ci-bot/k1` is
suspected of compromise, list every action it signed before you
rotated — across all tenants at once — with:

```
GET /v1/audit?signer_id=ci-bot&kid=k1
```

The same filter is exposed on the admin UI's "Audit Trail" page
as free-text inputs next to the tenant/namespace filters.

Signature metadata (`signer_id`, `kid`, `signature`,
`canonical_hash`) is surfaced in the audit record detail drawer
when present, so operators can spot-check individual records
without leaving the UI.

## Batch dispatch

`POST /v1/dispatch/batch` verifies each action independently. A
single failed signature rejects only its own entry — the rest of
the batch continues through the pipeline. The response array
preserves the input ordering so callers can match error entries
to submitted actions by index. Per-action metrics and
`tracing::warn` logs fire exactly as they do for single
dispatches, so observability stays consistent whether an operator
dispatches one action or a thousand.

Two edge cases:

1. **All actions rejected.** The gateway dispatch call is skipped
   entirely — no semaphore permit is consumed, no rules run. The
   response is still the same length as the input, with one error
   per entry.
2. **Unexpected crypto error on any action.** The whole batch
   fails with HTTP 500. Internal crypto errors indicate a bug or
   misconfiguration rather than a caller issue, so partial
   success would mask an incident the operator should
   investigate.

## Error behavior

| Scenario | HTTP status | Error message |
|----------|-------------|---------------|
| Invalid signature **or** unknown `(signer_id, kid)` | 400 | `signature verification failed for signer 'X'` |
| Signer not authorized for tenant/namespace | 400 | `signer 'X' is not authorized for tenant=Y namespace=Z` |
| Unsigned action + `reject_unsigned=true` | 400 | `unsigned action rejected: signing.reject_unsigned is enabled` |
| Replayed action ID + `reject_replay=true` | 409 | `replay rejected: action ID 'X' has already been dispatched` |
| Unexpected crypto error (bug or misconfig) | 500 | `signature verification failed with an unexpected crypto error: <detail>` |

Note that `InvalidSignature` and `UnknownSigner` return the **same**
wire message. This is intentional: a probing client should not be
able to use error responses to distinguish "this signer doesn't
exist" from "this signer exists but the signature is wrong". The
full variant (along with `signer_id`, `kid`, and scope) is written
to the server's `tracing::warn` log so operators can still debug a
failed dispatch from logs, and each variant still increments its
own Prometheus counter (`signing_unknown_signer_total` vs.
`signing_invalid_total`) for alerting.

## Metrics

The gateway tracks every branch of the signature verification path
as a Prometheus counter. They're exposed at `GET /metrics/prometheus`
(scraped by the Docker-compose monitoring profile) and as JSON at
`GET /metrics` / `GET /health`.

| Metric | Counted on |
|---|---|
| `acteon_signing_verified_total` | Cryptographically valid signature + scope-authorized |
| `acteon_signing_unsigned_allowed_total` | Unsigned action passed through because `signing.reject_unsigned` is off |
| `acteon_signing_invalid_total` | Signature present but Ed25519 verification failed |
| `acteon_signing_unknown_signer_total` | `signer_id` (or `(signer_id, kid)` during a rotation) not in the keyring |
| `acteon_signing_scope_denied_total` | Crypto valid but signer not authorized for the action's tenant/namespace |
| `acteon_signing_unsigned_rejected_total` | Unsigned action blocked by `signing.reject_unsigned` |
| `acteon_replay_rejected_total` | Action ID already seen inside the replay TTL window (independent of signing) |

Note that `acteon_replay_rejected_total` does **not** carry a
`signing_` prefix. Replay protection is driven by
`signing.reject_replay` in the config but runs independently of
signature verification — unsigned actions are subject to the same
deduplication window.

**What to alert on.** Verified signatures are the happy path — a
healthy deployment should see them trend with dispatch volume.
Spikes in `signing_invalid` or `signing_unknown_signer` after a
rotation usually mean a client didn't pick up the new `kid` yet;
monitor with:

```promql
rate(acteon_signing_unknown_signer_total[5m]) > 0.1
```

Sustained non-zero `signing_scope_denied` suggests a scoping
misconfiguration or an attempted cross-tenant attack — treat it
as a security signal and page on it:

```promql
increase(acteon_signing_scope_denied_total[15m]) > 0
```

`acteon_replay_rejected_total` fires when a client retries a
previously-dispatched action id within the TTL. Low rates are
noise (e.g. clients with misconfigured retries), sudden bursts
can indicate a replay attack.

To compute the signed-vs-unsigned traffic ratio during a rollout:

```promql
rate(acteon_signing_verified_total[5m])
  /
(rate(acteon_signing_verified_total[5m]) + rate(acteon_signing_unsigned_allowed_total[5m]))
```

**Grafana**: the bundled `acteon-overview` dashboard has an
"Action Signing" row with a time-series panel of the verification
rates and a stat panel for the totals.

**Dashboard UI**: the admin UI dashboard renders a compact "Sig
Verified / Sig Rejected" stat-card pair whenever signing is
configured on the server — including the first run with zero
traffic — so operators can confirm the config was picked up
without having to dispatch a test action first. The cards stay
hidden on deployments that don't enable signing at all.

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

### Discovery endpoint helper

All four polyglot SDKs (plus the Rust client) expose a
`fetch_signing_keys()` / `fetchSigningKeys()` / `FetchSigningKeys()`
method that hits `GET /.well-known/acteon-signing-keys` and returns
the parsed keyring. Use it to verify dispatched actions
independently, or to detect a rotation in progress — when a signer
has more than one entry, the operator is staging a new key and
clients should start sending the new `kid`.

```python
# Python
resp = client.fetch_signing_keys()
for key in resp.keys:
    print(f"{key.signer_id}/{key.kid}: {key.public_key}")
```

```typescript
// Node.js
const resp = await client.fetchSigningKeys();
for (const key of resp.keys) {
  console.log(`${key.signer_id}/${key.kid}: ${key.public_key}`);
}
```

```go
// Go
resp, err := client.FetchSigningKeys(ctx)
if err != nil { /* ... */ }
for _, k := range resp.Keys {
    fmt.Printf("%s/%s: %s\n", k.SignerID, k.Kid, k.PublicKey)
}
```

```java
// Java
SigningKeysResponse resp = client.fetchSigningKeys();
for (SigningKeyEntry key : resp.getKeys()) {
    System.out.printf("%s/%s: %s%n", key.getSignerId(), key.getKid(), key.getPublicKey());
}
```

The response has an empty `keys` list when signing is disabled on
the server.
