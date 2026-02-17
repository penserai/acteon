# AI Agent Swarm Coordination

This guide shows how to use Acteon as a **safety and orchestration layer** for
multi-agent AI systems. By routing every agent action through Acteon's dispatch
pipeline, you gain centralized permission control, prompt injection defense, rate
limiting, approval gates, failure isolation, and full observability -- without
modifying the agents themselves.

!!! tip "Runnable Example"
    Want to try this hands-on? The
    [`examples/agent-swarm-coordination/`](https://github.com/penserai/acteon/tree/main/examples/agent-swarm-coordination)
    directory contains a complete, runnable setup with Claude Code hooks, a
    PostgreSQL-backed Acteon server, deterministic safety rules, rate limiting,
    approval gates, and Discord notifications. Follow the README there to have a
    governed agent session running in minutes.

```mermaid
flowchart LR
    subgraph Agents
        A1[Researcher Agent]
        A2[Code Agent]
        A3[Deploy Agent]
        A4[Support Agent]
    end

    subgraph Acteon
        GW[Gateway]
        RE[Rule Engine]
        LLM[LLM Guardrails]
        WASM[WASM Plugins]
        QU[Quotas]
        AP[Approvals]
        CH[Chains]
    end

    subgraph Providers
        P1[GitHub API]
        P2[Cloud Deploy]
        P3[Slack]
        P4[Email]
        P5[Database]
    end

    A1 & A2 & A3 & A4 -->|dispatch| GW
    GW --> RE --> LLM --> WASM --> QU --> AP
    AP --> CH
    CH --> P1 & P2 & P3 & P4 & P5
```

Every agent call to an external service becomes an Acteon `dispatch`. The
pipeline evaluates rules, runs LLM and WASM guardrails, checks quotas, and
gates high-risk operations behind human approval -- all before a single byte
leaves your network.

---

## 1. Agent Identity and Tenant Isolation

Each agent operates as a **tenant** within a shared Acteon namespace. This gives
you per-agent audit trails, per-agent quotas, and per-agent rules -- all with
zero changes to the agents themselves.

### Dispatch with Agent Metadata

Agents identify themselves through the `tenant` field and carry role information
in `metadata`:

```bash
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer agent-researcher-key" \
  -d '{
    "namespace": "agent-swarm",
    "tenant": "researcher-agent",
    "provider": "github",
    "action_type": "search_code",
    "payload": {
      "query": "authentication bypass",
      "repositories": ["myorg/backend"]
    },
    "metadata": {
      "agent_id": "researcher-01",
      "agent_role": "researcher",
      "session_id": "sess-abc123",
      "parent_task": "security-audit-42"
    }
  }'
```

### Authentication Configuration

Create a separate API key per agent with scoped permissions:

```toml title="auth.toml"
# Read-only agent -- can dispatch but cannot manage rules or config
[[api_keys]]
key = "agent-researcher-key"
role = "operator"
tenant = "researcher-agent"
description = "Researcher agent -- search and read only"

# Deployment agent -- elevated privileges, still scoped to its tenant
[[api_keys]]
key = "agent-deploy-key"
role = "operator"
tenant = "deploy-agent"
description = "Deploy agent -- can trigger deployments"

# Human supervisor -- full admin access
[[api_keys]]
key = "supervisor-admin-key"
role = "admin"
description = "Human supervisor with full access"
```

!!! tip "Hot Reload"
    With `watch = true` in your `[auth]` config, you can add or revoke agent
    API keys without restarting the server. See
    [Authentication](../api/authentication.md) for details.

### Why Tenants?

Using tenants for agent isolation gives you:

| Capability | How |
|-----------|-----|
| Per-agent audit trails | Query audits by `tenant=researcher-agent` |
| Per-agent rate limits | Quota policies scoped to a tenant |
| Per-agent rules | Conditions on `action.tenant` |
| Per-agent state | Dedup keys and throttle counters are tenant-scoped |
| Cross-agent visibility | A supervisor can query all tenants in the namespace |

---

## 2. Permission Control with Rules

Rules form the **capability matrix** that determines what each agent is allowed
to do. By writing rules that match on `action.tenant` (agent identity),
`action.action_type` (operation), and `action.provider` (target service), you
build fine-grained permission boundaries.

### Deny-by-Default Capability Matrix

Start with a catch-all suppress rule at the lowest priority, then explicitly
allow operations per agent:

=== "YAML"

    ```yaml title="rules/agent-permissions.yaml"
    rules:
      # ── Deny by default ──────────────────────────────
      - name: deny-all-agents
        priority: 100
        description: "Block any agent action not explicitly allowed"
        condition:
          field: action.namespace
          eq: "agent-swarm"
        action:
          type: suppress
          reason: "No matching permission rule"

      # ── Researcher: search and read ──────────────────
      - name: allow-researcher-search
        priority: 10
        condition:
          all:
            - field: action.tenant
              eq: "researcher-agent"
            - field: action.action_type
              in: ["search_code", "read_file", "list_issues"]
        action:
          type: allow

      # ── Code agent: create PRs and branches ─────────
      - name: allow-code-agent-pr
        priority: 10
        condition:
          all:
            - field: action.tenant
              eq: "code-agent"
            - field: action.action_type
              in: ["create_branch", "create_pr", "push_commit"]
        action:
          type: allow

      # ── Deploy agent: staging only without approval ──
      - name: allow-deploy-staging
        priority: 10
        condition:
          all:
            - field: action.tenant
              eq: "deploy-agent"
            - field: action.action_type
              eq: "deploy"
            - field: action.payload.environment
              eq: "staging"
        action:
          type: allow

      # ── Deploy agent: production requires approval ───
      - name: approve-deploy-production
        priority: 5
        condition:
          all:
            - field: action.tenant
              eq: "deploy-agent"
            - field: action.action_type
              eq: "deploy"
            - field: action.payload.environment
              eq: "production"
        action:
          type: require_approval
          message: "Production deployment by deploy-agent requires human approval"
          ttl_seconds: 1800

      # ── Support agent: messaging only ────────────────
      - name: allow-support-messaging
        priority: 10
        condition:
          all:
            - field: action.tenant
              eq: "support-agent"
            - field: action.action_type
              in: ["send_email", "send_slack", "create_ticket"]
        action:
          type: allow
    ```

=== "CEL"

    ```cel
    // Deny by default
    action.namespace == "agent-swarm"
      ? suppress("No matching permission rule")
      : allow()

    // Researcher: search and read
    action.tenant == "researcher-agent"
      && action.action_type in ["search_code", "read_file", "list_issues"]
      ? allow()

    // Code agent: create PRs and branches
    action.tenant == "code-agent"
      && action.action_type in ["create_branch", "create_pr", "push_commit"]
      ? allow()
    ```

!!! info "Rule Priority"
    Lower priority numbers evaluate first. The deny-all rule at priority 100
    only fires if no higher-priority rule matched. See
    [Rule System](../concepts/rules.md) for details.

### Testing Permissions in the Playground

Use the [Rule Playground](../features/rule-playground.md) to verify your
capability matrix before deploying:

```bash
curl -X POST http://localhost:8080/v1/rules/evaluate \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "agent-swarm",
    "tenant": "researcher-agent",
    "provider": "github",
    "action_type": "delete_repository",
    "payload": {"repo": "myorg/backend"}
  }'
```

Expected response: the `deny-all-agents` rule fires, confirming the researcher
cannot delete repositories.

---

## 3. Prompt Injection Prevention

AI agents that process user input are vulnerable to prompt injection attacks
where malicious instructions are embedded in data. Acteon provides **four
layers of defense** that run in the dispatch pipeline before any action reaches
a provider -- from fast deterministic checks to expensive semantic analysis.

```mermaid
flowchart TD
    A[Agent dispatches action] --> B{Deterministic Rules}
    B -->|Blocked| X[Suppress]
    B -->|Clean| C{WASM Plugin}
    C -->|Injection detected| X
    C -->|Clean| D{LLM Guardrail}
    D -->|Suspicious| F[Route to review queue]
    D -->|Safe| E{Semantic Routing}
    E -->|Normal topic| G[Execute]
    E -->|Anomalous topic| F
```

### Layer 0: Deterministic Rules

The fastest and cheapest defense layer. Simple field-matching rules use
exact match, substring, regex, and list operators to block known-bad patterns
with zero external dependencies -- no WASM runtime, no LLM API, no embedding
service. These rules evaluate in microseconds and should catch the bulk of
trivial attacks before more expensive layers run.

```yaml title="rules/injection-defense.yaml"
rules:
  # ── Block known injection phrases ────────────────
  - name: block-injection-phrases
    priority: 1
    description: "Block payloads containing common injection strings"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - any:
            - field: action.payload.query
              contains: "ignore previous instructions"
            - field: action.payload.query
              contains: "ignore all prior"
            - field: action.payload.query
              contains: "you are now"
            - field: action.payload.query
              contains: "system prompt:"
            - field: action.payload.message
              contains: "ignore previous instructions"
            - field: action.payload.message
              contains: "you are now"
    action:
      type: suppress
      reason: "Known injection phrase detected"

  # ── Block encoded payload attacks ────────────────
  - name: block-encoded-injections
    priority: 1
    description: "Block base64-encoded or hex-encoded injection attempts"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - any:
            - field: action.payload.query
              regex: "base64[:\\.\\s]|\\\\x[0-9a-fA-F]{2}"
            - field: action.payload.message
              regex: "base64[:\\.\\s]|\\\\x[0-9a-fA-F]{2}"
            - field: action.payload.command
              regex: "base64[:\\.\\s]|\\\\x[0-9a-fA-F]{2}"
    action:
      type: suppress
      reason: "Encoded payload injection attempt"

  # ── Block dangerous shell patterns ───────────────
  - name: block-shell-injection
    priority: 1
    description: "Block shell metacharacters and command chaining in payloads"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          eq: "execute_command"
        - field: action.payload.command
          regex: "(;|&&|\\|\\||\\$\\(|`).*(curl|wget|nc|ncat|bash|sh|python)"
    action:
      type: suppress
      reason: "Shell injection pattern detected"

  # ── Block sensitive path access ──────────────────
  - name: block-sensitive-paths
    priority: 1
    description: "Prevent agents from accessing credential and config files"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - any:
            - field: action.payload.file_path
              regex: "(\\.env|\\.ssh/|credentials|\\bsecrets\\.)"
            - field: action.payload.command
              regex: "cat.*(\\. env|\\.ssh/|/etc/shadow|credentials)"
    action:
      type: suppress
      reason: "Sensitive path access blocked"

  # ── Block suspicious URL exfiltration ────────────
  - name: block-data-exfiltration
    priority: 1
    description: "Block curl/wget to unknown external hosts"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          eq: "execute_command"
        - field: action.payload.command
          regex: "(curl|wget|fetch)\\s.*https?://"
        - not:
            field: action.payload.command
            regex: "https?://(localhost|127\\.0\\.0\\.1|internal\\.|api\\.github\\.com)"
    action:
      type: suppress
      reason: "Outbound HTTP to unknown host blocked"
```

!!! tip "Why Deterministic Rules Come First"
    These rules cost nothing to evaluate -- no API calls, no WASM fuel, no
    LLM tokens. By filtering known-bad patterns at priority 1, you avoid
    paying for WASM or LLM evaluation on trivially malicious payloads. The
    remaining layers handle the attacks that are too sophisticated for simple
    string matching.

See [YAML Rule Reference](../api/rule-reference.md) for the complete list of
operators (`eq`, `contains`, `starts_with`, `regex`, `in`, `gt`, `lt`, etc.)
and field paths.

### Layer 1: WASM Plugin for Advanced Pattern Detection

For patterns too complex for simple field matching -- multi-field correlation,
stateful detection, or custom scoring models -- a WASM plugin provides
near-native speed with full sandboxing.

Critically, WASM plugins run inside a **strict sandbox** powered by Wasmtime.
Each plugin invocation has no access to the filesystem, network, or host
environment. Resource limits cap memory usage (default 16 MB) and CPU time
(default 100 ms via fuel metering). This means even a maliciously crafted
plugin cannot exfiltrate data, open network connections, or starve the gateway
of resources -- making WASM the ideal execution environment for untrusted or
third-party detection logic.

```rust
// injection_detector plugin (compiled to .wasm)
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct ActionContext {
    payload: serde_json::Value,
}

#[derive(Serialize)]
struct PluginResult {
    verdict: bool,
    message: Option<String>,
}

static PATTERNS: &[&str] = &[
    "ignore previous instructions",
    "ignore all prior",
    "you are now",
    "system prompt:",
    "\\x00",  // null byte injection
    "base64:",
];

#[no_mangle]
pub extern "C" fn evaluate(input_ptr: *const u8, input_len: usize) -> i32 {
    let input = unsafe { std::slice::from_raw_parts(input_ptr, input_len) };
    let ctx: ActionContext = serde_json::from_slice(input).unwrap();

    let text = ctx.payload.to_string().to_lowercase();
    let injection_found = PATTERNS.iter().any(|p| text.contains(p));

    let result = PluginResult {
        verdict: !injection_found,  // true = safe, false = blocked
        message: if injection_found {
            Some("Potential prompt injection detected".into())
        } else {
            None
        },
    };

    let output = serde_json::to_vec(&result).unwrap();
    // The plugin writes `output` into its linear memory and returns the byte
    // length. Acteon reads that many bytes from the module's memory to parse
    // the JSON result. See the WASM Plugins guide for the full ABI contract.
    output.len() as i32
}
```

Register the plugin and wire it into a rule:

```yaml title="rules/injection-defense.yaml"
rules:
  - name: wasm-injection-scan
    priority: 1
    description: "Fast pattern-based injection detection"
    condition:
      wasm_plugin: "injection-detector"
      wasm_function: "evaluate"
    action:
      type: suppress
      reason: "Prompt injection pattern detected by WASM scanner"
```

See [WASM Plugins](../features/wasm-plugins.md) for the full plugin development
guide, including resource limit configuration and module import validation.

!!! note "Why WASM for Security Checks?"
    Unlike sidecar processes or HTTP-based validators, WASM plugins execute
    in-process with zero network overhead and complete isolation. A plugin
    cannot read environment variables, access the host filesystem, or make
    outbound connections. If a plugin exceeds its CPU fuel budget, Acteon
    terminates it immediately and returns a safe default. This makes WASM
    ideal for running security-critical checks like injection detection in
    an agent swarm where the detection logic itself must be tamper-proof.

### Layer 2: LLM Guardrail for Semantic Analysis

For attacks that evade both deterministic rules and WASM pattern matching
(paraphrased instructions, multi-turn manipulation), an LLM guardrail provides
deep semantic analysis:

```toml title="acteon.toml"
[llm_guardrail]
endpoint = "https://api.openai.com/v1/chat/completions"
model = "gpt-4"
api_key_env = "OPENAI_API_KEY"
policy = "block"
temperature = 0.0
max_tokens = 256
```

```yaml title="rules/injection-defense.yaml"
  - name: llm-injection-check
    priority: 2
    description: "Semantic injection detection via LLM"
    condition:
      field: action.action_type
      in: ["send_message", "execute_code", "call_api"]
    action:
      type: llm_guardrail
      evaluator_name: "injection-detector"
      block_on_flag: true
      send_to: "security-review-queue"
```

!!! warning "Cost Considerations"
    LLM guardrails add latency and cost per evaluation. Deterministic rules
    and the WASM plugin layer filter the obvious attacks first, so only
    surviving actions reach the LLM. The three-level
    [LLM policy resolution](../features/llm-guardrails.md) lets you configure
    different policies per action type.

### Layer 3: Semantic Routing for Anomaly Detection

Semantic routing catches off-topic actions that slip through all previous layers. If a
"researcher" agent suddenly starts dispatching actions that are semantically
close to "infrastructure modification" or "credential access," the rule fires:

```yaml title="rules/injection-defense.yaml"
  - name: detect-anomalous-research
    priority: 3
    description: "Flag researcher actions that drift into dangerous topics"
    condition:
      all:
        - field: action.tenant
          eq: "researcher-agent"
        - semantic_match:
            text_field: "action.payload.query"
            topic: "infrastructure modification, credential access, or privilege escalation"
            threshold: 0.75
    action:
      type: reroute
      target_provider: "security-review-queue"
```

```toml title="acteon.toml"
[embedding]
enabled = true
endpoint = "https://api.openai.com/v1/embeddings"
model = "text-embedding-3-small"
api_key_env = "OPENAI_API_KEY"
```

See [Semantic Routing](../features/semantic-routing.md) for embedding
configuration and cache tuning.

---

## 4. Rate Limiting and Quotas

Runaway agents can exhaust API rate limits, burn through cloud budgets, or
denial-of-service your own infrastructure. Acteon provides two complementary
mechanisms: **quota policies** for hard per-tenant caps and **throttle rules**
for per-action-type rate limits.

### Quota Policies

Quota policies enforce a maximum number of actions per tenant per time window.
They run in the pipeline **before** rule evaluation, so an over-quota agent is
blocked regardless of which rules would have matched.

```toml title="acteon.toml"
# Researcher agent: 500 actions per hour
[[quotas]]
id = "q-researcher-hourly"
namespace = "agent-swarm"
tenant = "researcher-agent"
max_actions = 500
window = "hourly"
overage_behavior = "block"
enabled = true
description = "Researcher agent hourly cap"

# Code agent: 200 actions per hour
[[quotas]]
id = "q-code-hourly"
namespace = "agent-swarm"
tenant = "code-agent"
max_actions = 200
window = "hourly"
overage_behavior = "block"
enabled = true
description = "Code agent hourly cap"

# Deploy agent: 50 actions per day (deployments are expensive)
[[quotas]]
id = "q-deploy-daily"
namespace = "agent-swarm"
tenant = "deploy-agent"
max_actions = 50
window = "daily"
overage_behavior = "block"
enabled = true
description = "Deploy agent daily cap"
```

| Agent | Window | Limit | Overage Behavior |
|-------|--------|-------|-----------------|
| researcher-agent | Hourly | 500 | Block |
| code-agent | Hourly | 200 | Block |
| deploy-agent | Daily | 50 | Block |
| support-agent | Hourly | 1000 | Warn (log only) |

See [Tenant Quotas](../features/tenant-quotas.md) for the full API and
available overage behaviors.

### Throttle Rules

Throttle rules provide finer-grained rate limiting at the action-type level.
Use them to prevent bursts even when the agent is under its quota:

```yaml title="rules/agent-throttle.yaml"
rules:
  - name: throttle-agent-api-calls
    priority: 8
    description: "Limit any agent to 10 external API calls per minute"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          in: ["call_api", "search_code", "query_database"]
    action:
      type: throttle
      max_count: 10
      window_seconds: 60
      message: "Agent API call rate limit exceeded"

  - name: throttle-agent-messages
    priority: 8
    description: "Limit outbound messages to 5 per minute per agent"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          in: ["send_email", "send_slack"]
    action:
      type: throttle
      max_count: 5
      window_seconds: 60
      message: "Agent messaging rate limit exceeded"
```

See [Throttling](../features/throttling.md) for details on window semantics
and `retry_after` behavior.

---

## 5. Approval Gates for High-Risk Operations

Some agent actions are too dangerous to execute autonomously. Acteon's approval
system holds these actions pending until a human reviewer approves or rejects
them via HMAC-signed URLs.

```mermaid
sequenceDiagram
    participant A as Deploy Agent
    participant G as Acteon Gateway
    participant S as State Store
    participant H as Human Supervisor

    A->>G: dispatch(deploy, env=production)
    G->>G: Rule match: approve-deploy-production
    G->>S: Create approval record
    G-->>A: PendingApproval (approval_id, expires_at)
    G->>H: Notification with approve/reject URLs

    alt Approved
        H->>G: POST /approve?sig=...
        G->>G: Execute deployment
        G-->>A: ActionOutcome::Executed
    else Rejected
        H->>G: POST /reject?sig=...
        G-->>A: ActionOutcome::Rejected
    else Expired (30 min)
        Note over G,S: TTL expires
        G->>S: Mark as expired
    end
```

### Tiered Autonomy Rules

Define escalation tiers based on risk. Low-risk actions execute immediately,
medium-risk actions are flagged, and high-risk actions require approval:

```yaml title="rules/agent-approvals.yaml"
rules:
  # ── Tier 1: Autonomous (low risk) ────────────────
  - name: auto-approve-read-ops
    priority: 5
    description: "Read operations execute without approval"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          in: ["search_code", "read_file", "list_issues", "query_database"]
    action:
      type: allow

  # ── Tier 2: Flagged (medium risk) ────────────────
  - name: flag-write-ops
    priority: 5
    description: "Write operations are logged with extra detail"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          in: ["create_pr", "push_commit", "create_ticket"]
    action:
      type: allow
      metadata:
        risk_tier: "medium"
        audit_detail: "full_payload"

  # ── Tier 3: Approval required (high risk) ────────
  - name: approve-destructive-ops
    priority: 3
    description: "Destructive operations require human approval"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          in: ["deploy", "delete_resource", "modify_permissions", "execute_code"]
    action:
      type: require_approval
      message: "High-risk agent operation requires human approval"
      ttl_seconds: 1800

  # ── Tier 4: Always blocked ──────────────────────
  - name: block-forbidden-ops
    priority: 1
    description: "Operations that agents must never perform"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          in: ["delete_database", "rotate_credentials", "modify_firewall"]
    action:
      type: suppress
      reason: "Operation is permanently forbidden for agents"
```

See [Human Approvals](../features/approvals.md) for approval URL signing,
notification targets, and auto-approve conditions.

---

## 6. Multi-Agent Orchestration with Chains

Complex agent tasks often require coordinating multiple agents in sequence.
Acteon's chain system orchestrates multi-step workflows where each step's
output feeds into the next, with configurable failure policies and sub-chain
composition.

### Research-Summarize-Notify Pipeline

```toml title="acteon.toml"
# Main pipeline: research → summarize → notify
[[chains]]
name = "research-pipeline"
on_failure = "abort"
timeout_seconds = 600

[[chains.steps]]
name = "search"
provider = "github"
action_type = "search_code"

[[chains.steps]]
name = "analyze"
provider = "llm"
action_type = "summarize"
delay_seconds = 2

[[chains.steps]]
name = "review"
sub_chain = "human-review-loop"

[[chains.steps]]
name = "notify"
provider = "slack"
action_type = "send_message"

# Sub-chain: human review with escalation
[[chains]]
name = "human-review-loop"
timeout_seconds = 300

[[chains.steps]]
name = "send-for-review"
provider = "slack"
action_type = "send_review_request"

[[chains.steps]]
name = "wait-approval"
provider = "internal"
action_type = "wait_approval"
on_failure = "dlq"
```

### Trigger Rule

A rule triggers the chain when a research request arrives:

```yaml title="rules/agent-chains.yaml"
rules:
  - name: trigger-research-pipeline
    priority: 5
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          eq: "research_request"
    action:
      type: chain
      chain_name: "research-pipeline"
```

### Inspecting the DAG

Visualize the chain execution graph, including sub-chains:

```bash
# By chain execution ID
curl http://localhost:8080/v1/chains/chn-abc123/dag

# By chain definition name
curl http://localhost:8080/v1/chains/definitions/research-pipeline/dag
```

```mermaid
flowchart TB
    subgraph research-pipeline
        S1[search] --> S2[analyze]
        S2 --> S3[review]
        S3 --> S4[notify]
    end
    subgraph human-review-loop
        R1[send-for-review] --> R2[wait-approval]
    end
    S3 -.->|sub-chain| R1
    R2 -.->|result| S3
```

See [Task Chains](../features/chains.md) and
[Sub-Chains](../features/sub-chains.md) for payload templates, failure
policies, and conditional branching.

---

## 7. Agent State Tracking

Track each agent's lifecycle through configurable state machines. This is
useful for enforcing workflows (e.g., an agent must complete research before
deploying) and for detecting stuck or runaway agents.

### Agent Task State Machine

```toml title="acteon.toml"
[[state_machines]]
name = "agent-task"
initial_state = "idle"
states = ["idle", "researching", "coding", "reviewing", "deploying", "completed", "stale"]

[[state_machines.transitions]]
from = "idle"
to = "researching"

[[state_machines.transitions]]
from = "researching"
to = "coding"

[[state_machines.transitions]]
from = "coding"
to = "reviewing"

[[state_machines.transitions]]
from = "reviewing"
to = "deploying"

[[state_machines.transitions]]
from = "deploying"
to = "completed"

# Any state can go back to idle
[[state_machines.transitions]]
from = "researching"
to = "idle"

[[state_machines.transitions]]
from = "coding"
to = "idle"

[[state_machines.transitions]]
from = "reviewing"
to = "idle"

# Timeout: agents stuck in any active state for 2 hours become stale
[[state_machines.timeouts]]
state = "researching"
after_seconds = 7200
transition_to = "stale"

[[state_machines.timeouts]]
state = "coding"
after_seconds = 7200
transition_to = "stale"

[[state_machines.timeouts]]
state = "deploying"
after_seconds = 3600
transition_to = "stale"
```

```mermaid
stateDiagram-v2
    [*] --> idle
    idle --> researching: start_research
    researching --> coding: start_coding
    coding --> reviewing: submit_review
    reviewing --> deploying: approved
    deploying --> completed: deploy_success
    researching --> idle: cancel / timeout
    coding --> idle: cancel / timeout
    reviewing --> idle: cancel / timeout
    researching --> stale: Timeout (2h)
    coding --> stale: Timeout (2h)
    deploying --> stale: Timeout (1h)
```

!!! info "Who Triggers Transitions?"
    Transitions happen when an action with the matching fingerprint is
    dispatched. The agent itself can dispatch a `cancel` action to abandon its
    current task, or an external supervisor can dispatch it on the agent's
    behalf. Timeouts (configured above) trigger automatically when an agent is
    stuck in a state for too long.

### Suppressing Out-of-Order Actions

Use the state machine to enforce workflow order. For example, block a deploy
attempt if the agent has not completed the review phase:

```yaml title="rules/agent-state.yaml"
rules:
  - name: block-deploy-without-review
    priority: 2
    description: "Agents must be in 'reviewing' state before deploying"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          eq: "deploy"
        - field: state.current
          not_eq: "reviewing"
    action:
      type: suppress
      reason: "Cannot deploy without completing code review"
```

See [State Machines](../features/state-machines.md) for fingerprinting,
timeout transitions, and notification triggers.

---

## 8. Failure Isolation

When an agent's target provider fails, circuit breakers prevent cascading
failures across the swarm. Requests to the failing provider are rejected
immediately (or rerouted to a fallback) until the provider recovers.

### Circuit Breaker Configuration

```toml title="acteon.toml"
[circuit_breaker]
enabled = true
failure_threshold = 5
success_threshold = 2
recovery_timeout_seconds = 60

# GitHub API: higher tolerance, webhook fallback
[circuit_breaker.providers.github]
failure_threshold = 10
recovery_timeout_seconds = 120
fallback_provider = "github-webhook-queue"

# Cloud deploy: lower tolerance, no fallback (fail fast)
[circuit_breaker.providers.cloud-deploy]
failure_threshold = 3
recovery_timeout_seconds = 300
```

### Fallback Chain

Combine circuit breakers with chains for graceful degradation:

```toml title="acteon.toml"
[[chains]]
name = "resilient-notify"
on_failure = "skip"

[[chains.steps]]
name = "try-slack"
provider = "slack"
action_type = "send_message"
on_failure = "skip"

[[chains.steps]]
name = "try-email"
provider = "email"
action_type = "send_message"
on_failure = "skip"

[[chains.steps]]
name = "try-webhook"
provider = "webhook"
action_type = "send_message"
on_failure = "dlq"
```

```mermaid
flowchart LR
    A[Agent notification] --> B[Slack]
    B -->|Circuit open| C[Email]
    C -->|Circuit open| D[Webhook]
    D -->|All failed| E[Dead Letter Queue]
    B -->|Success| F[Done]
    C -->|Success| F
    D -->|Success| F
```

See [Circuit Breaker](../features/circuit-breaker.md) for state transitions,
per-provider overrides, and health probes.

---

## 9. Deduplication and Idempotency

Agents frequently retry failed operations or re-execute tasks after restarts.
Without deduplication, this leads to duplicate emails, duplicate PRs, or
duplicate deployments.

### Dedup Rules for Agent Actions

```yaml title="rules/agent-dedup.yaml"
rules:
  - name: dedup-agent-emails
    priority: 15
    description: "Deduplicate agent email sends within 10 minutes"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          eq: "send_email"
    action:
      type: deduplicate
      ttl_seconds: 600

  - name: dedup-agent-deploys
    priority: 15
    description: "Deduplicate deployment attempts within 1 hour"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          eq: "deploy"
    action:
      type: deduplicate
      ttl_seconds: 3600

  - name: dedup-agent-tickets
    priority: 15
    description: "Deduplicate ticket creation within 30 minutes"
    condition:
      all:
        - field: action.namespace
          eq: "agent-swarm"
        - field: action.action_type
          eq: "create_ticket"
    action:
      type: deduplicate
      ttl_seconds: 1800
```

!!! tip "Dedup Keys"
    Agents should set a meaningful `dedup_key` on each action. A good pattern
    is `{agent_id}-{task_id}-{action_type}` -- this prevents duplicates from
    the same agent on the same task while allowing different agents or tasks
    to execute the same action type independently. The `task_id` must be
    stable across agent restarts (e.g., a database-assigned ID or an
    externally provided job reference), otherwise a restarted agent will
    generate a new key and bypass deduplication.

See [Deduplication](../features/deduplication.md) for client-side dedup key
strategies and TTL tuning.

---

## 10. Monitoring and Observability

Full visibility into agent activity is critical for debugging, compliance, and
cost management. Acteon provides four observability surfaces: audit trails,
real-time SSE streams, Prometheus metrics, and Grafana dashboards.

### Audit Trail Queries

Query the audit trail to inspect agent activity:

```bash
# All actions by a specific agent in the last hour
curl "http://localhost:8080/v1/audit?namespace=agent-swarm&tenant=researcher-agent&since=1h"

# All suppressed actions (permission denials)
curl "http://localhost:8080/v1/audit?namespace=agent-swarm&outcome=suppressed&limit=50"

# All actions requiring approval
curl "http://localhost:8080/v1/audit?namespace=agent-swarm&outcome=pending_approval"

# Actions by a specific session across all agents
curl "http://localhost:8080/v1/audit?namespace=agent-swarm&metadata.session_id=sess-abc123"
```

### Real-Time Event Stream

Subscribe to agent events via Server-Sent Events for real-time dashboards
or supervisor alerting:

```bash
# Stream all agent-swarm events
curl -N http://localhost:8080/v1/events/stream?namespace=agent-swarm

# Stream events for a specific agent
curl -N "http://localhost:8080/v1/events/stream?namespace=agent-swarm&tenant=deploy-agent"
```

Each event arrives as an SSE message:

```
data: {"namespace":"agent-swarm","tenant":"deploy-agent","action_type":"deploy","outcome":"pending_approval","rule":"approve-deploy-production","timestamp":"2026-02-16T10:30:00Z"}
```

### Prometheus Metrics

Acteon exports Prometheus metrics at `GET /metrics/prometheus`. Key metrics
for agent swarm monitoring:

| Metric | Type | Description |
|--------|------|-------------|
| `acteon_actions_dispatched_total` | counter | Total dispatches across all agents |
| `acteon_actions_suppressed_total` | counter | Actions blocked by permission rules |
| `acteon_actions_throttled_total` | counter | Actions rejected by rate limits |
| `acteon_actions_deduplicated_total` | counter | Duplicate actions filtered |
| `acteon_actions_pending_approval_total` | counter | Actions waiting for human approval |
| `acteon_provider_success_rate` | gauge | Per-provider success percentage |
| `acteon_provider_latency_p99_ms` | gauge | Per-provider 99th percentile latency |
| `acteon_wasm_invocations_total` | counter | WASM plugin evaluations (injection scans) |
| `acteon_wasm_errors_total` | counter | WASM plugin failures |
| `acteon_circuit_breaker_state` | gauge | Per-provider circuit state (0=closed, 1=open, 2=half-open) |

### Grafana Dashboards

Use the pre-built Grafana dashboards to monitor agent swarm health:

```bash
docker compose --profile monitoring up
```

The **Acteon Overview** dashboard shows:

- Total dispatches per minute (all agents combined)
- Outcome breakdown (executed vs. suppressed vs. throttled vs. approval)
- Top agents by dispatch volume
- Error rate and latency trends

The **Provider Health** dashboard shows:

- Per-provider success rate and latency percentiles
- Circuit breaker state history
- Fallback activation events

See [Grafana Dashboards](../features/grafana-dashboards.md) for custom panel
configuration and alerting rules.

---

## 11. Integration with Claude Code

[Claude Code](https://claude.com/claude-code) is Anthropic's CLI-based coding
agent. Its **hooks** system lets you intercept every tool call (file writes,
bash commands, web fetches) and route them through Acteon for permission checks,
injection scanning, and audit logging -- turning Claude Code into a governed
agent in your swarm.

### Architecture

```mermaid
sequenceDiagram
    participant U as User
    participant CC as Claude Code
    participant H as PreToolUse Hook
    participant A as Acteon Gateway
    participant T as Tool (Bash/Write/Edit)

    U->>CC: "deploy to staging"
    CC->>CC: Plans Bash tool call
    CC->>H: PreToolUse event (tool_name, tool_input)
    H->>A: POST /v1/dispatch (action)
    A->>A: Rules + WASM + LLM + Quotas
    alt Allowed
        A-->>H: outcome: executed
        H-->>CC: exit 0 (allow)
        CC->>T: Execute tool
    else Suppressed
        A-->>H: outcome: suppressed
        H-->>CC: exit 2 (block)
        CC->>CC: "Action blocked by policy"
    else Pending Approval
        A-->>H: outcome: pending_approval
        H-->>CC: exit 2 (block, wait for approval)
    end
```

Every tool call Claude Code makes passes through the hook before execution.
The hook dispatches the action to Acteon, which evaluates all the rules,
quotas, and guardrails configured in this guide. The hook then allows or
blocks the tool call based on the outcome.

### Hook: Route Tool Calls Through Acteon

Create a hook script that intercepts tool calls and dispatches them to Acteon
for evaluation:

```bash title=".claude/hooks/acteon-gate.sh"
#!/bin/bash
set -e

INPUT=$(cat)
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name')
TOOL_INPUT=$(echo "$INPUT" | jq -c '.tool_input // {}')
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id')

# Map tool calls to Acteon action types
case "$TOOL_NAME" in
  Bash)
    ACTION_TYPE="execute_command"
    ;;
  Write|Edit)
    ACTION_TYPE="write_file"
    ;;
  WebFetch|WebSearch)
    ACTION_TYPE="web_access"
    ;;
  Task)
    ACTION_TYPE="spawn_agent"
    ;;
  *)
    # Allow non-sensitive tools (Read, Grep, Glob) without checking
    exit 0
    ;;
esac

# Dispatch to Acteon for policy evaluation
RESPONSE=$(curl -s -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $ACTEON_AGENT_KEY" \
  -d "{
    \"namespace\": \"agent-swarm\",
    \"tenant\": \"claude-code-agent\",
    \"provider\": \"claude-code\",
    \"action_type\": \"$ACTION_TYPE\",
    \"payload\": $TOOL_INPUT,
    \"metadata\": {
      \"tool_name\": \"$TOOL_NAME\",
      \"session_id\": \"$SESSION_ID\",
      \"agent_role\": \"coding\"
    },
    \"dedup_key\": \"$SESSION_ID-$ACTION_TYPE-$(echo "$TOOL_INPUT" | md5sum | cut -d' ' -f1)\"
  }" 2>/dev/null) || {
    # If Acteon is unreachable, fail closed
    echo "Acteon gateway unreachable -- blocking action for safety" >&2
    exit 2
  }

OUTCOME=$(echo "$RESPONSE" | jq -r '.outcome // "unknown"')

case "$OUTCOME" in
  executed|deduplicated)
    exit 0  # Allow the tool call
    ;;
  pending_approval)
    APPROVAL_ID=$(echo "$RESPONSE" | jq -r '.approval_id // "unknown"')
    echo "Action held for human approval (ID: $APPROVAL_ID). Check your notification channel." >&2
    exit 2
    ;;
  *)
    REASON=$(echo "$RESPONSE" | jq -r '.rule_name // "policy violation"')
    echo "Blocked by Acteon: $REASON" >&2
    exit 2
    ;;
esac
```

Register the hook in your project settings:

```json title=".claude/settings.json"
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash|Write|Edit|WebFetch|WebSearch|Task",
        "hooks": [
          {
            "type": "command",
            "command": "\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/acteon-gate.sh",
            "timeout": 10,
            "statusMessage": "Checking Acteon policy..."
          }
        ]
      }
    ]
  }
}
```

!!! warning "Fail Closed"
    The hook script blocks the action if Acteon is unreachable. This is the
    safe default for agent swarms -- a network partition should not grant an
    agent more permissions than it normally has.

### Hook: Scan Prompts for Injection Attacks

User prompts submitted to Claude Code can contain injection payloads designed
to override the agent's instructions. A `UserPromptSubmit` hook scans the
prompt through Acteon before Claude processes it:

```bash title=".claude/hooks/prompt-scan.sh"
#!/bin/bash
set -e

INPUT=$(cat)
PROMPT=$(echo "$INPUT" | jq -r '.tool_input.prompt // empty')
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id')

# Skip empty prompts
[ -z "$PROMPT" ] && exit 0

RESPONSE=$(curl -s -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $ACTEON_AGENT_KEY" \
  -d "{
    \"namespace\": \"agent-swarm\",
    \"tenant\": \"claude-code-agent\",
    \"provider\": \"claude-code\",
    \"action_type\": \"process_prompt\",
    \"payload\": {\"prompt\": $(echo "$PROMPT" | jq -Rs .)},
    \"metadata\": {\"session_id\": \"$SESSION_ID\"}
  }" 2>/dev/null) || exit 0  # Fail open for prompts to avoid blocking the user

OUTCOME=$(echo "$RESPONSE" | jq -r '.outcome // "executed"')

if [ "$OUTCOME" = "suppressed" ]; then
  REASON=$(echo "$RESPONSE" | jq -r '.rule_name // "injection detected"')
  echo "Prompt blocked: $REASON" >&2
  exit 2
fi

exit 0
```

```json title=".claude/settings.json (add to hooks)"
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/prompt-scan.sh",
            "timeout": 5,
            "statusMessage": "Scanning prompt..."
          }
        ]
      }
    ]
  }
}
```

### Acteon Rules for Claude Code

Add rules that match on the `claude-code-agent` tenant to control what Claude
Code can do:

```yaml title="rules/claude-code.yaml"
rules:
  # ── Block dangerous shell commands ───────────────
  - name: block-destructive-commands
    priority: 1
    description: "Block rm -rf, DROP TABLE, and other destructive commands"
    condition:
      all:
        - field: action.tenant
          eq: "claude-code-agent"
        - field: action.action_type
          eq: "execute_command"
        - field: action.payload.command
          matches: "(rm\\s+-rf|DROP\\s+TABLE|mkfs|dd\\s+if=|:\\(\\)\\{|chmod\\s+-R\\s+777)"
    action:
      type: suppress
      reason: "Destructive shell command blocked"

  # ── Block writes to sensitive paths ──────────────
  - name: block-sensitive-file-writes
    priority: 1
    description: "Prevent writing to credentials, env files, or CI config"
    condition:
      all:
        - field: action.tenant
          eq: "claude-code-agent"
        - field: action.action_type
          eq: "write_file"
        - field: action.payload.file_path
          matches: "(\\.env|\\.ssh|credentials|\\bci/|\\.github/workflows)"
    action:
      type: suppress
      reason: "Write to sensitive path blocked"

  # ── Require approval for production deploys ──────
  - name: approve-claude-code-deploy
    priority: 3
    description: "Claude Code deploy commands need human approval"
    condition:
      all:
        - field: action.tenant
          eq: "claude-code-agent"
        - field: action.action_type
          eq: "execute_command"
        - field: action.payload.command
          matches: "(kubectl apply|terraform apply|docker push|git push)"
    action:
      type: require_approval
      message: "Claude Code is attempting a deploy operation"
      ttl_seconds: 600

  # ── Scan prompts for injection via WASM ──────────
  - name: scan-claude-code-prompts
    priority: 1
    description: "Scan user prompts for injection patterns"
    condition:
      all:
        - field: action.tenant
          eq: "claude-code-agent"
        - field: action.action_type
          eq: "process_prompt"
        - wasm_plugin: "injection-detector"
          wasm_function: "evaluate"
    action:
      type: suppress
      reason: "Prompt injection detected"

  # ── Throttle agent commands ──────────────────────
  - name: throttle-claude-code-commands
    priority: 8
    description: "Limit Claude Code to 30 commands per minute"
    condition:
      all:
        - field: action.tenant
          eq: "claude-code-agent"
        - field: action.action_type
          eq: "execute_command"
    action:
      type: throttle
      max_count: 30
      window_seconds: 60

  # ── Allow read-only operations ───────────────────
  - name: allow-claude-code-reads
    priority: 10
    condition:
      all:
        - field: action.tenant
          eq: "claude-code-agent"
        - field: action.action_type
          in: ["read_file", "search_code", "web_access"]
    action:
      type: allow

  # ── Allow normal writes and commands ─────────────
  - name: allow-claude-code-writes
    priority: 15
    condition:
      all:
        - field: action.tenant
          eq: "claude-code-agent"
        - field: action.action_type
          in: ["write_file", "execute_command", "spawn_agent"]
    action:
      type: allow
```

### CLAUDE.md Behavioral Constraints

Combine Acteon enforcement with `CLAUDE.md` instructions so the agent is
guided by policy even before hooks fire:

```markdown title="CLAUDE.md (add to existing)"
## Agent Safety Policy

All tool calls in this project are routed through the Acteon gateway for
policy enforcement. Do not attempt to bypass or work around blocked actions.

If an action is blocked:
1. Report the block reason to the user
2. Suggest an alternative approach that stays within policy
3. Do not retry the same blocked action

Constraints:
- Never write to .env, .ssh, or credential files
- Never run rm -rf, DROP TABLE, or filesystem format commands
- Always use the project's deploy pipeline -- never run kubectl/terraform directly
- All production operations require human approval through Acteon
```

### Coding Agent Attack Vectors

Coding agents face unique attack vectors beyond standard prompt injection.
The table below maps each vector to the Acteon defense layer that mitigates
it:

| Attack Vector | Description | Acteon Defense |
|--------------|-------------|----------------|
| **Prompt injection in code comments** | Malicious instructions hidden in source files the agent reads | WASM plugin scans `process_prompt` payloads; LLM guardrail for semantic analysis |
| **Dependency confusion** | Agent installs a malicious package via `npm install` or `pip install` | `PreToolUse` hook dispatches `execute_command` to Acteon; regex rule blocks unknown registries |
| **Credential exfiltration** | Agent `cat`s `.env` then sends contents via `curl` | Rule blocks writes/reads to credential paths; throttle limits outbound network calls |
| **Privilege escalation** | Agent runs `sudo`, `chmod 777`, or modifies CI pipelines | Regex rule on `execute_command` blocks dangerous patterns; writes to `.github/workflows` suppressed |
| **Hidden file modification** | Agent writes to dotfiles (`.bashrc`, `.gitconfig`) to persist access | `write_file` rule blocks dotfile paths; audit trail records all write attempts |
| **Supply chain via subagents** | A `Task` tool spawns a subagent that bypasses the parent's hooks | Hook matches `Task` tool and dispatches `spawn_agent` to Acteon for policy check |
| **Multi-turn manipulation** | Attacker gradually shifts the agent's behavior across many prompts | `UserPromptSubmit` hook scans each prompt; semantic routing detects topic drift |
| **Resource exhaustion** | Agent spawns infinite loops or fills disk with large files | Quota policy caps total actions per hour; throttle limits command frequency |

!!! tip "Defense in Depth"
    No single layer catches everything. The `PreToolUse` hook catches tool-level
    attacks, the `UserPromptSubmit` hook catches prompt-level attacks, and
    `CLAUDE.md` provides behavioral guidance that reduces the attack surface
    before any hook fires. Combined with Acteon's rule engine, WASM plugins,
    and LLM guardrails, you get overlapping coverage across all vectors.

---

## 12. Comparison: Claude Code + Acteon vs. OpenClaw vs. Manus

The AI agent landscape in 2026 is dominated by two platforms:
[OpenClaw](https://github.com/openclaw/openclaw), the open-source autonomous
agent that went viral with 160k+ GitHub stars, and
[Manus](https://manus.im), the closed-source multi-agent platform acquired by
Meta. Both can run multi-agent swarms, but neither provides the kind of
centralized, configurable safety layer that production deployments require.

Can you build a governed agent swarm with **Claude Code + Acteon** that matches
or exceeds what OpenClaw and Manus offer? The short answer: yes -- and with
significantly stronger safety guarantees.

### Architecture Comparison

```mermaid
flowchart TB
    subgraph OpenClaw
        OC_GW[Gateway<br/>WebSocket hub]
        OC_AG[Agent Runtime<br/>single Node process]
        OC_SB[Docker Sandbox<br/>opt-in]
        OC_GW --> OC_AG --> OC_SB
    end

    subgraph Manus
        MA_PL[Planner Agent]
        MA_EX[Executor Agents<br/>cloud VMs]
        MA_SB[Session Sandbox<br/>cloud-only]
        MA_PL --> MA_EX --> MA_SB
    end

    subgraph Claude Code + Acteon
        CC[Claude Code<br/>local agent]
        HK[Hooks<br/>PreToolUse / UserPromptSubmit]
        AG[Acteon Gateway<br/>rule engine + WASM + LLM]
        PR[Providers<br/>any target]
        CC --> HK --> AG --> PR
    end
```

### Feature Matrix

| Capability | OpenClaw | Manus | Claude Code + Acteon |
|-----------|----------|-------|---------------------|
| **Agent runtime** | Single Node.js process, Agent SDK swarms | Cloud VMs, multi-agent planner/executor | Claude Code (local), Claude Agent SDK for swarms |
| **Sandboxing** | Docker containers (opt-in via NanoClaw) | Cloud VM per session (no self-host) | Acteon WASM sandbox (in-process, no FS/network) + optional Docker |
| **Permission model** | Layered policy: tool profiles, group policy, allowlists | Prompt-level guardrails, role-based page access | Acteon rule engine: deny-by-default capability matrix per agent |
| **Prompt injection defense** | Application-level -- model-last security posture | Prompt-level ethical guardrails | Four layers: deterministic rules + WASM scan + LLM analysis + semantic routing |
| **Rate limiting** | No built-in per-agent quotas | No user-configurable rate limits | Per-tenant quotas + per-action-type throttle rules |
| **Human approval gates** | No built-in approval workflow | No built-in approval workflow | HMAC-signed approve/reject URLs with configurable TTLs |
| **Action audit trail** | Conversation logs, workspace files | Session history (cloud-only) | Structured audit with outcome tracking, payload storage, field redaction |
| **Circuit breakers** | No built-in provider failure isolation | Cloud infrastructure handles failover | Per-provider circuit breakers with fallback chains |
| **Multi-step orchestration** | Agent SDK tool chaining | Planner decomposes into sub-tasks | Chains with sub-chains, conditional branching, DAG visualization |
| **State machines** | No built-in workflow enforcement | No user-configurable state tracking | Configurable state machines with timeout transitions |
| **Observability** | Log files, basic metrics | Cloud dashboard (closed) | Prometheus metrics, Grafana dashboards, SSE streams, audit API |
| **Deployment** | Self-hosted (Node.js) or cloud | Cloud-only (no self-host) | Self-hosted (Rust binary), any infrastructure |
| **Source model** | Open source (ISC license) | Closed source | Acteon: open source (Apache 2.0), Claude Code: commercial |

### What OpenClaw Gets Right (and Where Acteon Fills the Gaps)

OpenClaw's gateway architecture is conceptually similar to Acteon: a central
hub that routes messages between channels and agents. Its layered security
model (Identity -> Scope -> Model) is sound. However, there are critical gaps:

- **Sandboxing is opt-in.** By default, agents run in the host Node.js process
  with full system access. NanoClaw fixes this with Docker containers, but it
  is a separate project and adds operational complexity. Acteon's WASM plugins
  run sandboxed by default -- no Docker required.

- **No configurable rule engine.** OpenClaw's tool access is controlled by
  static policy files. There is no way to write conditional rules that match
  on payload content, action type, and agent identity simultaneously. Acteon's
  rule engine supports arbitrary conditions in YAML or CEL with priority-based
  evaluation.

- **No approval workflows.** OpenClaw has no built-in mechanism to hold an
  action pending until a human approves it. With Acteon, any rule can gate an
  action behind HMAC-signed approval URLs.

- **No per-agent quotas.** A runaway OpenClaw agent can exhaust API rate limits
  without any governor. Acteon enforces per-tenant quotas that block or warn
  before limits are reached.

### What Manus Gets Right (and Where Acteon Fills the Gaps)

Manus excels at autonomous task execution. Its planner/executor architecture
decomposes complex goals into sub-tasks and executes them in parallel cloud
VMs. However:

- **Cloud-only, closed-source.** Manus cannot be self-hosted. Your data
  leaves your network and enters a third-party cloud environment. Acteon
  runs on your own infrastructure with no external dependencies.

- **No user-configurable policy engine.** Manus has prompt-level ethical
  guardrails and basic role access, but you cannot define custom rules that
  match on payload content or enforce per-agent rate limits. Acteon gives
  you a full rule engine, WASM plugins, and LLM guardrails.

- **No audit trail.** Manus provides session history in its cloud dashboard,
  but there is no structured, queryable audit log with outcome tracking and
  field-level redaction. Acteon stores every dispatch with full metadata in
  PostgreSQL, ClickHouse, or Elasticsearch.

- **No failure isolation.** When a Manus sub-agent's target service fails,
  there is no circuit breaker or fallback mechanism. Acteon's circuit breakers
  automatically stop requests to failing providers and optionally reroute to
  fallbacks.

### Building a Production Agent Swarm

Here is how the three approaches compare for a concrete scenario: a four-agent
swarm (researcher, coder, reviewer, deployer) working on a codebase.

| Concern | OpenClaw | Manus | Claude Code + Acteon |
|---------|----------|-------|---------------------|
| Agent isolation | Docker containers (NanoClaw) | Cloud VMs | Tenant isolation + WASM sandbox |
| "Coder can't deploy" | Tool profile policy file | Not configurable | Rule: `tenant=code-agent` + `action_type=deploy` -> suppress |
| "Deployer needs approval for prod" | Not supported | Not supported | Rule: `require_approval` with TTL and notification |
| "Researcher limited to 500 calls/hr" | Not supported | Not supported | Quota policy: `max_actions=500, window=hourly` |
| "Detect injection in code comments" | Model-last assumption | Prompt guardrails | WASM scan + LLM guardrail + semantic routing |
| "All actions auditable for 90 days" | Log files (unstructured) | Cloud dashboard | Structured audit with 90-day TTL and redaction |
| "Slack circuit breaker at 5 failures" | Not supported | Cloud infra | `circuit_breaker.providers.slack.failure_threshold = 5` |
| "Visualize agent workflow" | Not built-in | Task tree (closed UI) | Chain DAG API + admin UI |

!!! info "Not Mutually Exclusive"
    You can run OpenClaw or Manus agents **through** Acteon. The hook-based
    integration shown in Section 11 works with any agent that makes HTTP
    calls or runs shell commands. Acteon does not replace the agent runtime --
    it governs the actions the runtime produces. You could run an OpenClaw
    swarm with every agent action dispatched through Acteon for policy
    enforcement, quotas, and audit.

### When to Use Each

- **OpenClaw** is the right choice when you need a **messaging-first agent**
  that connects to WhatsApp, Slack, or iMessage and runs tasks triggered by
  chat messages. Pair it with Acteon for safety.

- **Manus** is the right choice when you need **zero-setup autonomous task
  execution** and are comfortable with cloud-only deployment and limited
  configurability. Not suitable for regulated environments.

- **Claude Code + Acteon** is the right choice when you need a **governed
  coding agent swarm** with fine-grained permissions, prompt injection defense,
  approval workflows, full audit trails, and self-hosted deployment. This is
  the only option that gives you a configurable policy engine between the
  agent and the outside world.

---

## 13. Complete Configuration

Below is the full configuration combining all sections of this guide into a
single deployment.

=== "acteon.toml"

    ```toml title="acteon.toml"
    # ─── Server ─────────────────────────────────────────────
    [server]
    host = "0.0.0.0"
    port = 8080

    # ─── Authentication ─────────────────────────────────────
    [auth]
    enabled = true
    config_path = "auth.toml"
    watch = true

    # ─── State Backend ──────────────────────────────────────
    [state]
    backend = "redis"
    url = "redis://localhost:6379"

    # ─── Audit ──────────────────────────────────────────────
    [audit]
    enabled = true
    backend = "postgres"
    url = "postgres://acteon:acteon@localhost:5432/acteon"
    store_payload = true
    ttl_seconds = 7776000   # 90 days

    [audit.redact]
    enabled = true
    fields = ["api_key", "authorization"]
    placeholder = "[REDACTED]"

    # ─── LLM Guardrail ─────────────────────────────────────
    [llm_guardrail]
    endpoint = "https://api.openai.com/v1/chat/completions"
    model = "gpt-4"
    api_key_env = "OPENAI_API_KEY"
    policy = "block"
    temperature = 0.0
    max_tokens = 256

    # ─── Embedding (Semantic Routing) ──────────────────────
    [embedding]
    enabled = true
    endpoint = "https://api.openai.com/v1/embeddings"
    model = "text-embedding-3-small"
    api_key_env = "OPENAI_API_KEY"

    # ─── WASM Plugins (sandboxed: no FS, no network) ─────
    [wasm]
    enabled = true
    module_dir = "./plugins"
    memory_limit_bytes = 16777216       # 16 MB per invocation
    cpu_timeout_ms = 100                # Hard kill after 100ms

    # ─── Circuit Breaker ────────────────────────────────────
    [circuit_breaker]
    enabled = true
    failure_threshold = 5
    success_threshold = 2
    recovery_timeout_seconds = 60

    [circuit_breaker.providers.github]
    failure_threshold = 10
    recovery_timeout_seconds = 120
    fallback_provider = "github-webhook-queue"

    [circuit_breaker.providers.cloud-deploy]
    failure_threshold = 3
    recovery_timeout_seconds = 300

    # ─── Quotas ─────────────────────────────────────────────
    [[quotas]]
    id = "q-researcher-hourly"
    namespace = "agent-swarm"
    tenant = "researcher-agent"
    max_actions = 500
    window = "hourly"
    overage_behavior = "block"
    enabled = true

    [[quotas]]
    id = "q-code-hourly"
    namespace = "agent-swarm"
    tenant = "code-agent"
    max_actions = 200
    window = "hourly"
    overage_behavior = "block"
    enabled = true

    [[quotas]]
    id = "q-deploy-daily"
    namespace = "agent-swarm"
    tenant = "deploy-agent"
    max_actions = 50
    window = "daily"
    overage_behavior = "block"
    enabled = true

    # ─── State Machine ─────────────────────────────────────
    [[state_machines]]
    name = "agent-task"
    initial_state = "idle"
    states = ["idle", "researching", "coding", "reviewing", "deploying", "completed", "stale"]

    [[state_machines.transitions]]
    from = "idle"
    to = "researching"

    [[state_machines.transitions]]
    from = "researching"
    to = "coding"

    [[state_machines.transitions]]
    from = "coding"
    to = "reviewing"

    [[state_machines.transitions]]
    from = "reviewing"
    to = "deploying"

    [[state_machines.transitions]]
    from = "deploying"
    to = "completed"

    [[state_machines.transitions]]
    from = "researching"
    to = "idle"

    [[state_machines.transitions]]
    from = "coding"
    to = "idle"

    [[state_machines.transitions]]
    from = "reviewing"
    to = "idle"

    [[state_machines.timeouts]]
    state = "researching"
    after_seconds = 7200
    transition_to = "stale"

    [[state_machines.timeouts]]
    state = "coding"
    after_seconds = 7200
    transition_to = "stale"

    [[state_machines.timeouts]]
    state = "deploying"
    after_seconds = 3600
    transition_to = "stale"

    # ─── Chains ─────────────────────────────────────────────
    [[chains]]
    name = "research-pipeline"
    on_failure = "abort"
    timeout_seconds = 600

    [[chains.steps]]
    name = "search"
    provider = "github"
    action_type = "search_code"

    [[chains.steps]]
    name = "analyze"
    provider = "llm"
    action_type = "summarize"
    delay_seconds = 2

    [[chains.steps]]
    name = "review"
    sub_chain = "human-review-loop"

    [[chains.steps]]
    name = "notify"
    provider = "slack"
    action_type = "send_message"

    [[chains]]
    name = "human-review-loop"
    timeout_seconds = 300

    [[chains.steps]]
    name = "send-for-review"
    provider = "slack"
    action_type = "send_review_request"

    [[chains.steps]]
    name = "wait-approval"
    provider = "internal"
    action_type = "wait_approval"
    on_failure = "dlq"

    [[chains]]
    name = "resilient-notify"
    on_failure = "skip"

    [[chains.steps]]
    name = "try-slack"
    provider = "slack"
    action_type = "send_message"
    on_failure = "skip"

    [[chains.steps]]
    name = "try-email"
    provider = "email"
    action_type = "send_message"
    on_failure = "skip"

    [[chains.steps]]
    name = "try-webhook"
    provider = "webhook"
    action_type = "send_message"
    on_failure = "dlq"
    ```

=== "rules/agent-permissions.yaml"

    ```yaml title="rules/agent-permissions.yaml"
    rules:
      # ── Permanently blocked operations ───────────────
      - name: block-forbidden-ops
        priority: 1
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - field: action.action_type
              in: ["delete_database", "rotate_credentials", "modify_firewall"]
        action:
          type: suppress
          reason: "Operation is permanently forbidden for agents"

      # ── Injection defense: deterministic rules ───────
      - name: block-injection-phrases
        priority: 1
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - any:
                - field: action.payload.query
                  contains: "ignore previous instructions"
                - field: action.payload.query
                  contains: "you are now"
                - field: action.payload.message
                  contains: "ignore previous instructions"
                - field: action.payload.message
                  contains: "you are now"
        action:
          type: suppress
          reason: "Known injection phrase detected"

      - name: block-encoded-injections
        priority: 1
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - any:
                - field: action.payload.query
                  regex: "base64[:\\.\\s]|\\\\x[0-9a-fA-F]{2}"
                - field: action.payload.message
                  regex: "base64[:\\.\\s]|\\\\x[0-9a-fA-F]{2}"
        action:
          type: suppress
          reason: "Encoded payload injection attempt"

      - name: block-shell-injection
        priority: 1
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - field: action.action_type
              eq: "execute_command"
            - field: action.payload.command
              regex: "(;|&&|\\|\\||\\$\\(|`).*(curl|wget|nc|bash|sh|python)"
        action:
          type: suppress
          reason: "Shell injection pattern detected"

      - name: block-sensitive-paths
        priority: 1
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - any:
                - field: action.payload.file_path
                  regex: "(\\.env|\\.ssh/|credentials|\\bsecrets\\.)"
                - field: action.payload.command
                  regex: "cat.*(\\. env|\\.ssh/|/etc/shadow|credentials)"
        action:
          type: suppress
          reason: "Sensitive path access blocked"

      - name: block-data-exfiltration
        priority: 1
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - field: action.action_type
              eq: "execute_command"
            - field: action.payload.command
              regex: "(curl|wget|fetch)\\s.*https?://"
            - not:
                field: action.payload.command
                regex: "https?://(localhost|127\\.0\\.0\\.1|internal\\.|api\\.github\\.com)"
        action:
          type: suppress
          reason: "Outbound HTTP to unknown host blocked"

      # ── Injection defense: WASM ──────────────────────
      - name: wasm-injection-scan
        priority: 1
        condition:
          wasm_plugin: "injection-detector"
          wasm_function: "evaluate"
        action:
          type: suppress
          reason: "Prompt injection pattern detected"

      # ── Injection defense: LLM ───────────────────────
      - name: llm-injection-check
        priority: 2
        condition:
          field: action.action_type
          in: ["send_message", "execute_code", "call_api"]
        action:
          type: llm_guardrail
          evaluator_name: "injection-detector"
          block_on_flag: true
          send_to: "security-review-queue"

      # ── Injection defense: semantic anomaly ──────────
      - name: detect-anomalous-research
        priority: 3
        condition:
          all:
            - field: action.tenant
              eq: "researcher-agent"
            - semantic_match:
                text_field: "action.payload.query"
                topic: "infrastructure modification, credential access, or privilege escalation"
                threshold: 0.75
        action:
          type: reroute
          target_provider: "security-review-queue"

      # ── Approval gates ───────────────────────────────
      - name: approve-destructive-ops
        priority: 3
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - field: action.action_type
              in: ["deploy", "delete_resource", "modify_permissions", "execute_code"]
        action:
          type: require_approval
          message: "High-risk agent operation requires human approval"
          ttl_seconds: 1800

      # ── Throttle rules ──────────────────────────────
      - name: throttle-agent-api-calls
        priority: 8
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - field: action.action_type
              in: ["call_api", "search_code", "query_database"]
        action:
          type: throttle
          max_count: 10
          window_seconds: 60

      # ── Agent permissions ────────────────────────────
      - name: allow-researcher-search
        priority: 10
        condition:
          all:
            - field: action.tenant
              eq: "researcher-agent"
            - field: action.action_type
              in: ["search_code", "read_file", "list_issues"]
        action:
          type: allow

      - name: allow-code-agent-pr
        priority: 10
        condition:
          all:
            - field: action.tenant
              eq: "code-agent"
            - field: action.action_type
              in: ["create_branch", "create_pr", "push_commit"]
        action:
          type: allow

      - name: allow-deploy-staging
        priority: 10
        condition:
          all:
            - field: action.tenant
              eq: "deploy-agent"
            - field: action.action_type
              eq: "deploy"
            - field: action.payload.environment
              eq: "staging"
        action:
          type: allow

      - name: allow-support-messaging
        priority: 10
        condition:
          all:
            - field: action.tenant
              eq: "support-agent"
            - field: action.action_type
              in: ["send_email", "send_slack", "create_ticket"]
        action:
          type: allow

      # ── Deduplication ────────────────────────────────
      - name: dedup-agent-emails
        priority: 15
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - field: action.action_type
              eq: "send_email"
        action:
          type: deduplicate
          ttl_seconds: 600

      - name: dedup-agent-deploys
        priority: 15
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - field: action.action_type
              eq: "deploy"
        action:
          type: deduplicate
          ttl_seconds: 3600

      # ── Catch-all deny ──────────────────────────────
      - name: deny-all-agents
        priority: 100
        condition:
          field: action.namespace
          eq: "agent-swarm"
        action:
          type: suppress
          reason: "No matching permission rule"
    ```

=== "rules/agent-chains.yaml"

    ```yaml title="rules/agent-chains.yaml"
    rules:
      - name: trigger-research-pipeline
        priority: 5
        condition:
          all:
            - field: action.namespace
              eq: "agent-swarm"
            - field: action.action_type
              eq: "research_request"
        action:
          type: chain
          chain_name: "research-pipeline"
    ```

---

## 14. Best Practices

- **Deny by default.** Start with a catch-all suppress rule and explicitly
  allow only the operations each agent needs. This prevents privilege creep
  as you add new action types.

- **Use tenants for isolation.** Each agent (or agent class) should have its
  own tenant. This gives you per-agent audit trails, quotas, and rules without
  additional configuration.

- **Layer your defenses.** No single check catches everything. Start with
  deterministic rules (free, microseconds) for known-bad patterns, then WASM
  plugins (fast, sandboxed) for complex logic, then LLM guardrails (thorough,
  expensive) for semantic analysis, and finally semantic routing for anomaly
  detection.

- **Set meaningful dedup keys.** Use `{agent_id}-{task_id}-{action_type}` to
  prevent duplicate actions from retries while allowing different agents or
  tasks to perform the same operation. Ensure `task_id` is stable across
  agent restarts.

- **Gate destructive operations.** Any action that modifies production
  infrastructure, deletes data, or changes permissions should require human
  approval. Use tiered autonomy (Section 5) to avoid bottlenecking read-only
  operations.

- **Monitor suppression rates.** A spike in suppressed actions often indicates
  a misconfigured agent or a prompt injection attack. Set Grafana alerts on
  `acteon_actions_suppressed_total`.

- **Use state machines for workflow enforcement.** Agents that must follow a
  prescribed workflow (research -> code -> review -> deploy) should be tracked
  through a state machine. This prevents agents from skipping steps.

- **Set quotas conservatively.** Start with low limits and increase based on
  observed usage. It is easier to raise a quota than to recover from a runaway
  agent that burned through your cloud budget.

- **Test rules in the playground.** Before deploying rule changes, use the
  Rule Playground to verify that each agent can perform its intended operations
  and is blocked from everything else.

- **Audit everything.** Enable `store_payload = true` with field redaction.
  Full payload audit trails are invaluable for post-incident analysis of agent
  misbehavior.

---

## 15. See Also

- [**Runnable Example**](https://github.com/penserai/acteon/tree/main/examples/agent-swarm-coordination) -- Complete working setup with hooks, rules, and PostgreSQL
- [Rule System](../concepts/rules.md) -- How rules are evaluated and matched
- [The Dispatch Pipeline](../concepts/pipeline.md) -- Full pipeline stages
- [Authentication](../api/authentication.md) -- API key and JWT configuration
- [WASM Plugins](../features/wasm-plugins.md) -- Custom plugin development
- [LLM Guardrails](../features/llm-guardrails.md) -- LLM-based content gates
- [Semantic Routing](../features/semantic-routing.md) -- Embedding-based routing
- [Tenant Quotas](../features/tenant-quotas.md) -- Per-tenant rate limits
- [Throttling](../features/throttling.md) -- Per-action-type rate limits
- [Human Approvals](../features/approvals.md) -- Approval workflows
- [Task Chains](../features/chains.md) -- Multi-step orchestration
- [Sub-Chains](../features/sub-chains.md) -- Chain composition
- [State Machines](../features/state-machines.md) -- Event lifecycle tracking
- [Circuit Breaker](../features/circuit-breaker.md) -- Failure isolation
- [Deduplication](../features/deduplication.md) -- Idempotent action handling
- [Audit Trail](../features/audit-trail.md) -- Comprehensive event logging
- [Grafana Dashboards](../features/grafana-dashboards.md) -- Pre-built dashboards
- [Rule Playground](../features/rule-playground.md) -- Interactive rule testing
