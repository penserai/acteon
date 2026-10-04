# Governed agent coordination

Give specialist agents a shared way to request work, with explicit authority,
policy, approvals, execution history, and recovery. Acteon governs the operations
submitted through its interfaces. Your agent runtime supplies reasoning and tools;
your providers perform the integrations.

This guide starts with a complete local policy exercise, then shows how to connect
it to an agent team. Its providers only log: no LLM subscription, cloud credentials,
email account, or production system is required to verify the flow.

## What you will prove

| Request | Expected result |
|---|---|
| Research within the caller's scope | Executed by a log provider |
| Repeat the same research key | Deduplicated |
| Read credentials or delete a database | Suppressed |
| An unrecognized operation | Suppressed by the default rule |
| A research operation aimed at the deploy provider | Suppressed; provider identity is part of the rule |
| Dispatch into another team's tenant | HTTP 403 |
| Propose a deployment | Pending approval; execution follows signed approval |
| Request a research workflow | A two-step chain completes |
| Inspect usage policy | The static hourly quota is loaded |

The example uses real server authentication, YAML rules, gateway dispatch,
approvals, chain advancement, and quota loading. It does not claim to detect every
prompt injection or to sandbox an agent's direct tool access.

## Run the complete example

From the repository root, with Rust 1.88+ and Python 3:

```bash
cargo build --locked -p acteon-server
python3 scripts/ci/agent_guide.py --server target/debug/acteon-server
```

The verifier starts an isolated server on an available loopback port, creates an
encryption key for the test process, exercises the table above, and stops the
server. It loads the same checked-in configuration shown below. CI runs this check
so a parser change or broken example fails before publication.

To explore manually:

```bash
export ACTEON_AUTH_KEY="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
export ACTEON_AGENT_KEY="acteon-policy-demo"
export ACTEON_URL="http://127.0.0.1:8080"
cargo run --locked -p acteon-server -- \
  -c examples/agent-swarm-coordination/policy-demo/acteon.toml
```

The checked-in API key and JWT secret are **public local-demo credentials**.
Replace them for a deployment. In-memory state and audit records disappear on
restart; use persistent backends for durable production operation.

In a second terminal, export the same URL and demo API key and submit an action:

```bash
export ACTEON_URL="http://127.0.0.1:8080"
export ACTEON_AGENT_KEY="acteon-policy-demo"
curl --fail-with-body -sS "$ACTEON_URL/v1/dispatch" \
  -H "Authorization: Bearer $ACTEON_AGENT_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "550e8400-e29b-41d4-a716-446655440020",
    "created_at": "2026-10-03T00:00:00Z",
    "namespace": "agent-swarm",
    "tenant": "researcher",
    "provider": "research",
    "action_type": "search",
    "dedup_key": "research:ticket-42",
    "payload": {"query": "Summarize the attached incident evidence"}
  }'
```

The first response contains `Executed`; repeating it returns the JSON string
`"Deduplicated"`. The wire format uses externally tagged `ActionOutcome` variants,
not an `outcome` property containing a lowercase status. SDK helpers provide their
language's parsed representation; see [client coverage](../api/sdk-coverage.md).

## Complete server configuration

Paths below assume commands run from the repository root. The auth path is
resolved relative to the server configuration file. Quota policies live in a
separate file, selected by `[quotas].policies_file`. Chain definitions use
`[[chains.definitions]]`; the background processor advances them in the configured
namespace and tenant.

<!-- agent-guide-file: acteon.toml -->
```toml
# Run from the repository root. All providers log locally.
[server]
host = "127.0.0.1"
port = 8080

[state]
backend = "memory"

[audit]
enabled = true
backend = "memory"
store_payload = true

[auth]
enabled = true
config_path = "auth.toml"
watch = false

[background]
enabled = true
namespace = "agent-swarm"
tenant = "researcher"

[rules]
directory = "examples/agent-swarm-coordination/policy-demo/rules"

[quotas]
enabled = true
policies_file = "examples/agent-swarm-coordination/policy-demo/quotas.toml"
watch = false

[[providers]]
name = "research"
type = "log"

[[providers]]
name = "notify"
type = "log"

[[providers]]
name = "deploy"
type = "log"

[[chains.definitions]]
name = "research-pipeline"
on_failure = "abort"
timeout_seconds = 60

[[chains.definitions.steps]]
name = "capture"
provider = "research"
action_type = "capture_summary"
payload_template = { query = "{{origin.payload.query}}" }

[[chains.definitions.steps]]
name = "notify"
provider = "notify"
action_type = "send_message"
payload_template = { summary = "{{steps.capture.body}}" }
```

### Caller identity

The local test key belongs to the `researcher` principal and can dispatch only within the
listed scope. Its `executor` role permits action submission without access to
policy or registry administration. Giving another agent a different tenant label in its payload does
not establish identity: the server compares that scope with the authenticated
caller's grants. Create separate keys and grants for separate principals.

<!-- agent-guide-file: auth.toml -->
```toml
# Public local-demo credentials. Replace before deployment.
[settings]
jwt_secret = "acteon-policy-demo-local-only-secret"

[[api_keys]]
name = "researcher"
key_hash = "e5522a60c16c993c80c7c251fe4dc4ad5f1de7926c57ec5401d3d42891633ad8"
role = "executor"

[[api_keys.grants]]
tenants = ["researcher"]
namespaces = ["agent-swarm"]
providers = ["research", "notify", "deploy"]
actions = ["*"]
```

### Static quotas

This file contains policy entries, distinct from the server's `[quotas]` settings.
Quotas apply before rule selection. You can also manage policies through the API;
see [tenant quotas](../features/tenant-quotas.md).

<!-- agent-guide-file: quotas.toml -->
```toml
[[quotas]]
namespace = "agent-swarm"
tenant = "researcher"
max_actions = 50
window = "hourly"
overage_behavior = "block"
```

### Policy and rule ordering

Rules select the first matching verdict by priority. They are **not a sequence of
middleware checks**: an early allow or throttle verdict does not then evaluate a
later approval or deduplication rule. This example keeps forbidden actions and
deployments ahead of permitted operations, and ends with default suppression.

<!-- agent-guide-file: rules/policy.yaml -->
```yaml
rules:
  - name: block-forbidden
    priority: 1
    condition:
      field: action.action_type
      in_list: [delete_database, read_credentials]
    action:
      type: suppress
  - name: approve-deploy
    priority: 2
    condition:
      all:
        - field: action.namespace
          eq: agent-swarm
        - field: action.provider
          eq: deploy
        - field: action.action_type
          eq: deploy
    action:
      type: request_approval
      notify_provider: notify
      message: "Review the proposed deployment"
      timeout_seconds: 600
  - name: research-workflow
    priority: 3
    condition:
      all:
        - field: action.namespace
          eq: agent-swarm
        - field: action.provider
          eq: research
        - field: action.action_type
          eq: research_request
    action:
      type: chain
      chain: research-pipeline
  - name: dedup-research
    priority: 4
    condition:
      all:
        - field: action.namespace
          eq: agent-swarm
        - field: action.provider
          eq: research
        - field: action.action_type
          eq: search
    action:
      type: deduplicate
      ttl_seconds: 600
  - name: deny-unrecognized
    priority: 100
    condition:
      field: action.namespace
      eq: agent-swarm
    action:
      type: suppress
```

The YAML approval action is `request_approval`, with `notify_provider` and
`timeout_seconds`. A rule that starts a chain uses `chain`, not `chain_name`.
The exact configuration is exercised by the verifier above.

## Approvals and execution boundaries

A `deploy` request targets the `deploy` log provider and returns `PendingApproval`.
Its signed `approve_url` and `reject_url` authorize those decisions until expiry.
Treat these links as capabilities; present them to the intended reviewer. The
local verifier uses the returned approval URL to exercise execution after consent.
In an agent integration, the human reviewer owns that decision.

A `research_request` returns `ChainStarted` with a `chain_id`. Inspect it with:

```bash
curl --fail-with-body -sS \
  "$ACTEON_URL/v1/chains/$CHAIN_ID?namespace=agent-swarm&tenant=researcher" \
  -H "Authorization: Bearer $ACTEON_AGENT_KEY"
```

Set `CHAIN_ID` to the returned ID. Acceptance does not mean completion: poll until
the chain reaches a terminal status. The two configured steps call providers
directly. Use a [full-pipeline dispatch step](../features/chains.md#full-pipeline-dispatch-steps)
when a step must emit a fresh action for rule evaluation and approval policy.
A provider named `internal` with action type `wait_approval` does not create a
built-in human gate. Use approval rules or an explicitly designed durable signal
flow instead.

## Connect a real agent team

Use a scoped `executor` credential for each agent runtime and keep `operator`
credentials for trusted administration. Expose the intended action-submission
tool through a host adapter, fix its
namespace and tenant in the adapter, and keep management endpoints inaccessible
to that agent. Network access and credentials must enforce this boundary.

The host must also retain approval capability URLs and send them only to the
human reviewer. Return the pending status and approval ID to the agent, not the
signed approve/reject URLs. Giving an agent those capabilities lets it decide its
own approval.

Choose the integration that matches the work:

- **REST or SDKs:** submit scoped actions and interpret their actual outcomes.
- **MCP:** expose supported Acteon tools to an agent host through the
  [MCP server](../api/mcp-server.md).
- **A2A:** use agent discovery and task lifecycles through the
  [A2A interface](../features/a2a.md).
- **Agentic Bus:** use identities, conversations, typed tool messages, and
  scoped streams through the [bus](../concepts/agentic-bus.md).
- **Swarm orchestrator:** configure roles, agent engines, evaluation, critique,
  and recovery in the [Agent Swarm runtime](../features/agent-swarm.md).
- **Ambient swarm provider:** submit a prepared goal as an action and track the
  returned run ID through the [swarm provider](../features/swarm-provider.md).

For a coding-agent hook, intercept the tool before execution, send a complete
Action, and fail closed on transport errors or unrecognized outcomes. A pending
approval means the tool must remain blocked. Do not both execute the provider's
operation and allow the agent to repeat that same side effect locally. Choose
whether Acteon executes the operation or authorizes a separate host-controlled
step, and implement a durable handoff for the latter.

The older hook demonstration in `examples/agent-swarm-coordination/hooks` is an
integration sketch. The tested `policy-demo` above is the runnable entry point;
host-specific hooks need their own wire-format and fail-closed tests before use.

## Add model-assisted decisions deliberately

Use [governed model providers](../features/governed-model-provider.md) for locked
request material and schema-validated inference. [LLM guardrails](../features/llm-guardrails.md)
and [semantic routing](../features/semantic-routing.md) have separate configuration
and evaluation semantics. A schema-valid answer can still be wrong. Deterministic
permissions, scoped credentials, resource limits and approvals remain necessary.

The [observability detector](neural-observability-detector.md) demonstrates this
with real local model calls and deterministic corroboration. It uses recording
providers for investigation and incident effects, so it does not demonstrate a
live autonomous agent or production remediation.

For swarm quality, define an evaluator with explicit acceptance criteria, retain
its evidence, and bound recovery attempts. An adversarial reviewer should challenge
the result against those criteria; it must not gain permission to bypass the same
policy applied to the primary agents. See the orchestrator's evaluation and
recovery documentation for engine-specific configuration.

## Operate the system

Use persistent [state and audit backends](../backends/index.md), configure
retention, and inspect execution history and action audit separately. The
[governance model](../concepts/governance.md) describes the policy boundary;
[operations](../operations/index.md) covers recovery and intervention.

Replay and idempotency reduce repeated work within their documented boundaries.
They cannot undo external effects. Agent processes and worker code that directly
access external systems remain outside Acteon's control unless their credentials
and tool paths enforce the intended integration.
