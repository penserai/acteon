# Cross-SDK contract fixtures

Shared, language-neutral fixtures pinning the wire contracts that the
worker SDKs and the Rust server must agree on. A workflow execution can
migrate between workers written in different languages mid-flight, so
checkpoint-key derivation and directive shapes are a compatibility
surface, not an implementation detail.

| File | Pins |
|---|---|
| `workflow-contract.json` | Workflow directives (`complete` / `fail` / `sleep` / `await_signal`), checkpoint-key derivation (`step:{name}#{k}`, `sleep#{k}`, `signal:{name}#{k}`, `child:{workflow}#{k}`), integer-seconds coercions, the timed-out marker, and the reserved `__workflow__` / `__child:` constants |

Consumers (a drift on any side fails that side's suite):

- `crates/core/tests/workflow_contract.rs` — the server parses every
  SDK-emittable directive (and rejects the malformed ones loudly).
- `clients/python/tests/test_contract.py` — Python SDK.
- `clients/nodejs/src/contract.test.ts` — Node.js SDK.

When adding a workflow runner to another SDK (Go, Java), add a consumer
for this file alongside it. When changing a wire shape, update the
fixture and every consumer in the same PR.

`platform-api.json` is generated from the server router by
`scripts/sdk/platform_catalog.py`; its CI check keeps all five operation
catalogs synchronized. Transport tests exercise authentication, paths, queries,
bodies, response envelopes, and HTTP failures.

`dispatch-outcomes.json` covers the governance outcomes that were previously
missing from non-Rust SDKs, plus the bare-string deduplication outcome in batches.
Rust round-trips these through `acteon_core::ActionOutcome`; Python, TypeScript,
Go, and Java verify every field and the single/batch outcome discriminator.

`execution-permits.json` defines the explicit permit-reference header payload.
All five SDKs verify single/batch request construction, retained credentials and
legacy header omission. Refusal tests preserve HTTP statuses instead of treating
server error envelopes as transport or deserialization failures.

`agent-services.json` pins task identities, response provenance headers, and
foreign model metadata across all five native SDKs. Contract tests verify that
concurrent jobs use their own context, mutable task data cannot change the
observation identity, missing headers cannot be filled from metadata, and HTTP
failures and redirects are not retried. Rust additionally exercises the real
server's authenticated agent requester and configured CORS response exposure.

The agent-service fixture also supplies per-job future-start stop acknowledgements.
All five native SDK contracts verify the original task and request-local context,
retain provider task status, and reject failed or false acknowledgements without
retries. The fixture distinguishes restriction-only and uncertain provider-abort
states; SDKs reject malformed attempt identities and proof digests. Browser contracts
cover explicit same-job recovery and later completion.
