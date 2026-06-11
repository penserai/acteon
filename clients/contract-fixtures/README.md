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
