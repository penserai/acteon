# Governed Model Provider

The `governed-model` provider exposes a JSON inference runtime through the same
dispatch pipeline as every other Acteon provider. Rules and chains can invoke a
small classifier, scorer, or other non-autoregressive model without adding
model-specific code to the gateway.

The provider accepts an Action payload, calls a configured HTTP endpoint,
validates the raw response against a content-addressed JSON Schema, and returns
the validated JSON as the provider response body. Circuit breakers, retries,
quotas, audit records, and chain data flow apply normally.

## Configuration

```toml
[[providers]]
name = "metrics-detector"
type = "governed-model"

model.endpoint = "http://laya:8000/v1/systemone"
model.health_endpoint = "http://laya:8000/health"
model.lock_file = "/etc/acteon/models/model.lock.json"
model.contracts_root = "/etc/acteon/models"
model.response_contract = "laya-response"

# Optional locked request material. These fields overwrite caller values.
model.request_contract = "metrics"
model.request_contract_field = "questions"
model.model_field = "model"

model.bearer_token = "ENC[...]"
model.timeout_seconds = 30
model.max_response_bytes = 1048576
model.verify_identity_each_call = true
```

The lock's `contracts` map must contain `laya-response`, whose file is a JSON
Schema, and `metrics`, whose file is the static request contract. Contract paths
are resolved from `contracts_root`; when omitted, that root is the lock file's
parent directory. Both files are checked against their SHA-256 values before the
provider is registered.

`request_contract` and `request_contract_field` are optional but must be set
together. With the example above, this caller payload:

```json
{
  "state": {"pool_used_ratio": 0.98},
  "questions": {"forged": true},
  "model": "unapproved-model"
}
```

is sent to the runtime with the locked `metrics` contract in `questions` and
the locked model name in `model`. Caller-supplied values at those fields cannot
select a different contract or model. Other payload fields are preserved. If no
injection fields are configured, the payload is sent unchanged.

## Runtime identity

The health endpoint must return:

```json
{
  "loaded": ["typed-decisions"],
  "revisions": {
    "typed-decisions": "55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851"
  }
}
```

Acteon requires exactly the model and revision named by the lock. It checks the
endpoint at startup and, by default, before each inference request. Startup
fails if the runtime is unavailable, the health document is malformed, or the
identity has drifted. Set `verify_identity_each_call = false` only when the
deployment already makes model replacement impossible and periodic provider
health checks are sufficient.

The serving container remains responsible for verifying its installed runtime
packages and model artifact files against the lock. Acteon verifies the
contracts and served identity it can observe, while the runtime verifies files
inside its own trust boundary.

## Response contract and evidence

The inference response is bounded before parsing. It must be valid JSON and
must satisfy the locked response schema before it can enter a rule or the next
chain step. Redirects and ambient proxy settings are disabled for both model
and health requests.

A successful provider response includes these audit headers:

| Header | Meaning |
|---|---|
| `acteon-model-lock-digest` | SHA-256 of the exact model lock bytes |
| `acteon-model-repository` | Locked model repository |
| `acteon-model-name` | Locked served model name |
| `acteon-model-revision` | Locked immutable revision |
| `acteon-model-request-contract` | Locked injected request contract, when configured |
| `acteon-model-response-contract` | Locked schema contract name |
| `acteon-model-elapsed-ms` | End-to-end inference latency |

Schema violations and invalid JSON are non-retryable provider failures. Network
errors, HTTP 5xx responses, and HTTP 429 responses retain retryable provider
semantics, so chain retry and circuit-breaker policy can handle transient model
runtime failures.

## Use in a chain

A direct provider step calls the governed model and makes its validated response
available to the next step:

```toml
[[chains.definitions]]
name = "classify-and-route"

[[chains.definitions.steps]]
name = "classify"
provider = "metrics-detector"
action_type = "model.classify"

[[chains.definitions.steps]]
name = "route-verdict"

[chains.definitions.steps.dispatch]
provider = "verdict-router"
action_type = "detector.verdict"
dedup_key = "{{prev.body.incident_key}}"
```

The first step has the lower cost and bounded output of a System One model. The
full-pipeline dispatch step then sends only that typed result back through
Acteon's deterministic rules, where policy can suppress noise, start another
chain, invoke an agent, or require approval.
