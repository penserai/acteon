# acteon-client (Python)

Python client for the Acteon action gateway.

## Complete platform API

The generated operation catalog exposes all 198 finite HTTP operations, including receipt sessions, managed stages, workflows, execution controls, inference profiles, and stream windows. Use an authenticated client and a configured, existing stage for this example:

```python
from acteon_client import ActeonClient, PlatformOperation

with ActeonClient("http://localhost:8080", api_key="your-api-key") as client:
    status = client.platform_request(
        PlatformOperation.BUS_STAGES_STATUS,
        path={"namespace": "observability", "tenant": "demo", "id": "log-detector"},
    )
```

Path parameters are escaped individually. Query and body fields use the server's wire names; JSON response envelopes are preserved. Calls do not retry automatically. Keep request IDs stable across retries and return opaque receipt IDs unchanged. HTTP access does not imply a code-defined workflow runner in every language.

See [SDK coverage and wire contracts](https://penserai.github.io/acteon/api/sdk-coverage/) for return types, native streaming APIs, runtime availability, and server feature requirements. The catalog is generated from the registered server routes and checked in CI.

## Typed governance management

Use `governance`, `publish_governance_permit`, and `intervene_governance` (sync and async) to inspect permitted routes, issue bounded permits, and close/reopen resources or revoke permits, subjects, and credentials. Requests and responses have native typed models. These methods require an authenticated operator and an independently configured execution manager for the requested namespace and tenant; an executor credential alone does not grant management authority.

Calls preserve HTTP refusals and do not retry automatically. Keep `change_id` stable when reconciling a lost response. A receipt records persisted authority; closure affects subsequent starts and does not promise cancellation of work already in flight.

See [governance management](https://penserai.github.io/acteon/features/governance/) for deployment policy and wire examples, and [the governed city simulation](https://penserai.github.io/acteon/guides/governed-city/) for real authenticated requests.

## Installation

```bash
pip install acteon-client
```

Or install from source:

```bash
cd clients/python
pip install -e .
```

## Quick Start

```python
from acteon_client import ActeonClient, Action

# Create a client
client = ActeonClient("http://localhost:8080")

# Check health
if client.health():
    print("Server is healthy")

# Dispatch an action
action = Action(
    namespace="notifications",
    tenant="tenant-1",
    provider="email",
    action_type="send_notification",
    payload={"to": "user@example.com", "subject": "Hello"},
)

outcome = client.dispatch(action)
print(f"Outcome: {outcome.outcome_type}")

# Close the client
client.close()
```

## Context Manager

```python
with ActeonClient("http://localhost:8080") as client:
    outcome = client.dispatch(action)
```

## Async Client

```python
import asyncio
from acteon_client import AsyncActeonClient, Action


async def main():
    async with AsyncActeonClient("http://localhost:8080") as client:
        action = Action(
            namespace="notifications",
            tenant="tenant-1",
            provider="email",
            action_type="send_notification",
            payload={"to": "user@example.com"},
        )
        outcome = await client.dispatch(action)
        print(f"Outcome: {outcome.outcome_type}")


asyncio.run(main())
```

## Batch Dispatch

```python
actions = [
    Action(namespace="ns", tenant="t1", provider="email", action_type="send", payload={"i": i})
    for i in range(10)
]

results = client.dispatch_batch(actions)
for result in results:
    if result.success:
        print(f"Success: {result.outcome.outcome_type}")
    else:
        print(f"Error: {result.error.message}")
```

## Rule Management

```python
# List all rules
rules = client.list_rules()
for rule in rules:
    print(f"{rule.name}: priority={rule.priority}, enabled={rule.enabled}")

# Reload rules from disk
result = client.reload_rules()
print(f"Loaded {result.loaded} rules")

# Disable a rule
client.set_rule_enabled("block-spam", False)
```

## Time-Based Rules

Rules can use `time.*` fields to match on the current UTC time at dispatch. Configure these in your YAML or CEL rule files — no client-side changes needed.

```yaml
# rules/business_hours.yaml
rules:
  - name: suppress-outside-hours
    priority: 1
    condition:
      any:
        - field: time.hour
          lt: 9
        - field: time.hour
          gte: 17
    action:
      type: suppress

  - name: suppress-weekends
    priority: 2
    condition:
      field: time.weekday_num
      gt: 5
    action:
      type: suppress
```

Use dry-run to test what a time-based rule would do right now:

```python
outcome = client.dispatch(action, dry_run=True)
print(f"Verdict: {outcome.verdict}")  # e.g. "suppress"
print(f"Matched rule: {outcome.matched_rule}")  # e.g. "suppress-outside-hours"
```

Available `time` fields: `hour` (0–23), `minute`, `second`, `day`, `month`, `year`, `weekday` (`"Monday"`…`"Sunday"`), `weekday_num` (1=Mon…7=Sun), `timestamp`.

## Audit Trail

```python
from acteon_client import AuditQuery

# Query audit records
query = AuditQuery(tenant="tenant-1", limit=10)
page = client.query_audit(query)
print(f"Found {page.total} records")
for record in page.records:
    print(f"  {record.action_id}: {record.outcome}")

# Get specific record
record = client.get_audit_record("action-id-123")
if record:
    print(f"Found: {record.outcome}")
```

## Configuration

```python
client = ActeonClient(
    "http://localhost:8080",
    timeout=60.0,  # Request timeout in seconds
    api_key="your-key",  # Optional API key
)
```

API keys are sent via the `Authorization: Bearer <key>` header. The server
accepts both JWTs and raw API keys on that header. API keys are scoped by
tenant, namespace, provider, and action type on the server side — see the
[API Key Scoping](https://penserai.github.io/acteon/features/api-key-scoping/)
documentation for the grant model and hierarchical tenant matching.

## Task-Queue Worker

`Worker` polls a durable task queue and dispatches each task to a handler
registered for its `action_type`. Returning completes the task; raising
fails it as **retryable by default** (bounded by the task's `max_attempts`).
Raise `NonRetryableError` to fail permanently. Long-running handlers are
heartbeat-extended automatically at half the lease interval, and `async def`
handlers are supported.

```python
from acteon_client import ActeonClient, NonRetryableError, Worker

client = ActeonClient("http://localhost:8080", api_key="your-key")
worker = Worker(client, "jobs", "tenant-1", queue="emails", max_concurrent=4)


def send_email(payload):
    if "@" not in payload["to"]:
        raise NonRetryableError("malformed address")  # never retried
    return {"message_id": deliver(payload)}  # completes the task


worker.register("send_email", send_email)
worker.run()  # blocks; call worker.stop() from a signal handler to drain

# Producers enqueue work with:
client.enqueue_task(
    "emails", "jobs", "tenant-1", "send_email", {"to": "user@example.com"}, max_attempts=5
)
```

`worker.run_once()` polls and processes a single batch — useful in tests
and cron-style invocations.

## Workflows

Workflows are checkpoint-based: the registered function re-runs from the
top on every continuation, and `ctx` replays recorded checkpoints by name —
completed `ctx.step(...)` calls return their stored result instantly, and
suspension points (`ctx.sleep`, `ctx.wait_for_signal`) only suspend the
first time through. Code paths up to a suspension point must therefore be
deterministic; side effects belong inside `ctx.step`.

```python
from acteon_client import ActeonClient, Worker

client = ActeonClient("http://localhost:8080", api_key="your-key")
worker = Worker(client, "jobs", "tenant-1", queue="wf-queue")


def onboarding(ctx, input):
    account = ctx.step("provision", lambda: provision(input["user"]))
    ctx.sleep(24 * 3600)  # durable timer
    approval = ctx.wait_for_signal("approved", timeout_seconds=86_400)
    if approval is None:  # timed out
        return {"status": "expired"}
    child_id = ctx.start_child("welcome_email", {"account": account})
    outcome = ctx.wait_for_child(child_id)
    return {"status": "done", "email": outcome}


worker.register_workflow("onboarding", onboarding)
worker.run()
```

Drive executions from any client:

```python
execution = client.start_workflow("jobs", "tenant-1", "onboarding", "wf-queue", {"user": "u-1"})
client.signal_workflow(
    execution.execution_id, "approved", "jobs", "tenant-1", payload={"by": "ops"}
)
execution = client.get_workflow_execution(execution.execution_id, "jobs", "tenant-1")
print(execution.status, execution.result)
```

## Error Handling

```python
from acteon_client import ActeonError, ConnectionError, ApiError, HttpError

try:
    outcome = client.dispatch(action)
except ConnectionError as e:
    print(f"Connection failed: {e}")
    if e.is_retryable():
        # Retry logic
        pass
except ApiError as e:
    print(f"API error [{e.code}]: {e.message}")
    if e.is_retryable():
        # Retry logic
        pass
except HttpError as e:
    print(f"HTTP {e.status}: {e.message}")
```

## API Reference

### ActeonClient Methods

| Method | Description |
|--------|-------------|
| `health()` | Check server health |
| `dispatch(action)` | Dispatch a single action |
| `dispatch_batch(actions)` | Dispatch multiple actions |
| `list_rules()` | List all loaded rules |
| `reload_rules()` | Reload rules from disk |
| `set_rule_enabled(name, enabled)` | Enable/disable a rule |
| `query_audit(query)` | Query audit records |
| `get_audit_record(action_id)` | Get specific audit record |
| `fetch_signing_keys()` | Fetch the server's active signing keyring (JWKS-style discovery) |

### Action Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `namespace` | str | Yes | Logical grouping |
| `tenant` | str | Yes | Tenant identifier |
| `provider` | str | Yes | Target provider |
| `action_type` | str | Yes | Type of action |
| `payload` | dict | Yes | Action-specific data |
| `id` | str | No | Auto-generated UUID |
| `dedup_key` | str | No | Deduplication key |
| `metadata` | dict | No | Key-value metadata |

## License

Apache-2.0

## Dispatch outcomes

All clients recognize the server's 17 dispatch variants, including `Grouped`, `StateChanged`, `PendingApproval`, `ChainStarted`, `CircuitOpen`, `RecurringCreated`, `Silenced`, and `Muted`. Single and batch dispatch preserve their fields, including approval capabilities and chain IDs. A pending approval or a started chain is not a completed provider execution; inspect the outcome before advancing an agent workflow.

## Execution-only credentials

Configure runtime API keys or JWT users with `role = "executor"` in the server's
`auth.toml`, and limit their namespace, tenant, provider and action grants.
Use the client's existing API-key or bearer-token configuration; the role is
established by the server, not by client-supplied action fields. Administrative
methods return HTTP 403 for executor credentials. Keep operator credentials and
signed human approval URLs in a trusted host.
See [authentication and role boundaries](https://penserai.github.io/acteon/api/authentication/).


### Stable actor identity

`client.identity()` or `await client.identity()` inspects the authenticated credential and its optional stable
principal binding. Configure `principal = { id = "diagnostic-agent", kind = "agent" }`
on the credential in server `auth.toml`; keep it unchanged across credential
rotation. Roles and grants still apply independently to each credential. Legacy
credentials return a null principal. Principal metadata does not grant execution
permits or bind a bus agent automatically. See the
[authentication guide](https://penserai.github.io/acteon/api/authentication/).

The identity response also exposes the optional logical enrollment ID configured
as `authority_id` in `auth.toml`. Keep this ID when rotating a key; separate
credentials for the same actor retain separate IDs and grants. JWT sessions pin
the ID at login and require a new login if it changes. This field is inspection
metadata, not a bearer credential or execution permit.

## Execution permits

When the server enables execution authority, effectful dispatch requires explicit
references to permits already issued to the authenticated principal. These
options select permits; the server checks current credentials, scope, revisions,
qualified effects and budget before execution. They do not issue permits or
replace authentication. Existing dispatch methods omit the header by default.

```python
from acteon_client import PermitReference

permits = [PermitReference("maya-incident", 1)]
outcome = client.dispatch(action, permits=permits)
batch = client.dispatch_batch([action], permits=permits)
# AsyncActeonClient accepts the same options; await both calls.
```

The same references apply to every action in a batch. Dry runs can omit them.
Missing/invalid headers, authorization denials and conflicting replay requests
preserve their HTTP status in the client's HTTP error type. Permit admission
refusals can also be returned as a `Failed` action outcome; inspect the outcome.
Do not generate a new action ID merely to bypass a replay conflict.

## Agent workforce

Typed workforce methods inspect teams, memberships, ownership, assignments and
mandates, and submit all ten workforce changes. They require independently
declared workforce management bounds and current operator authentication.
Public values do not establish representation or caller authority.

```python
from acteon_client import (
    ActeonClient,
    TeamRef,
    WorkforceTeam,
    PutWorkforceTeam,
    WorkforceChangeRequest,
)

with ActeonClient("http://localhost:8080", api_key="operator-key") as client:
    roster = client.workforce("prod", "acme")
    receipt = client.change_workforce(
        WorkforceChangeRequest(
            "prod",
            "acme",
            "enroll-reliability-1",
            PutWorkforceTeam(
                WorkforceTeam(TeamRef("prod", "acme", "reliability"), 1, "Reliability")
            ),
            "Establish incident response",
        )
    )
```

`AsyncActeonClient` provides the same methods with `await`.

Preserve the exact request and change ID after a lost acknowledgment. Mutations
retain HTTP refusal status and are not automatically retried. See the
[workforce model and API](https://penserai.github.io/acteon/features/workforce/)
for represented permits, dependencies, revisions and intervention semantics.


### Pending provider work

`ProviderPending` means a governed provider attempt is still in flight, needs
reconciliation, or awaits a retry under its original identity. Preserve its
execution ID, attempt count and state. It is not a completed failure: do not
resend the action as fresh work. Chain details expose the durable `wait_state`
(`waitState` in Node and Java), and `waiting_provider` remains an active status.
Completed receipts can repair workflow results after revocation or expiry;
starting another effect still requires current authority.

### Retained provider history

Inspect a governed provider execution using independently authorized history management credentials:

```python
history = client.provider_execution_history("prod", "acme", execution_id)
# The async client exposes the same method with await.
```

The response includes the signed participant, receipt state, observed authority generation, optional verified operation metadata and binding, cancellation fence, and per-attempt evidence. Nested outcomes use the SDK’s existing outcome model. Original results and accepted reconciliation remain separate. A completed receipt may contain a failed outcome. Null evidence means no verified result is available; it does not prove that no effect occurred. Reading history never starts, retries, reconciles, or resumes execution.

The server requires `can_read_history` and current scope management authority; the execution ID is a reference, not authorization. A scope may retain this access after every live provider is removed.

Provider execution history preserves an optional reconciliation `acceptance`
record with the original operator principal, evaluated authority incarnation and
generation, and `accepted_at_ms`. This is distinct from the proof's
`resolved_at_ms` recording time. Legacy adapter settlements may omit acceptance;
a client must not infer the operator from the execution owner. Reading this
metadata grants no execution or reconciliation authority.


Governance management bounds also preserve the independent `can_reconcile`
capability; absence means denied. It authorizes qualified finality management,
subject to the server's exact resource ceiling and trusted verifier installation.
History access and permit-issuing permissions do not imply this capability.
Typed correlation and acceptance methods use current reconciliation authority and
never automatically retry acceptance. Only trusted server installations with a
qualified verifier can accept evidence. The server installs operator-qualified
sources through `[[reconciliation_sources]]`; source declarations do not grant
management permission. See the governance guide for dedicated keys, exact binding
digests and coordinated key retirement.

```python
from acteon_client import ProviderReconciliationRequest

correlation = client.provider_reconciliation_correlation("prod", "acme", execution_id, 0)
# evidence_base64 comes from the independently qualified finality source.
receipt = client.accept_provider_reconciliation(
    "prod",
    "acme",
    execution_id,
    0,
    ProviderReconciliationRequest(proof_base64=evidence_base64),
)
```

The async client exposes the same methods with `await`.

These methods preserve the completed or definitive no-effect receipt. Correlation
is not a permit; proof bytes must come from the qualified source. After a lost
acknowledgment, replay the exact proof under current authority rather than changing
its content or verifier revision.

## Authenticated individual-agent services

These native helpers target a configured governed agent service. The server must
have the service installed, and the client must retain the original requester
credential and invocation permits. This is a separate surface from tenant-level
legacy A2A tasks.

```python
from acteon_client import make_message, make_part_text

receipt = client.agent_service_send_message(
    "prod",
    "acme",
    "notifier",
    make_message("incident-42", "user", [make_part_text("Notify the incident owner")]),
)
task = client.agent_service_get_task(receipt)
```

The async client exposes the same methods with `await`. Persist the frozen
`AgentServiceReceipt` in host state when work must survive a client restart.

Acceptance does not prove that provider execution has completed. Read the task's
status and artifacts through the receipt. Its source context comes only from the
admission response header; never obtain it from model output or task metadata.
Keep it with the original route and task ID in host state, outside model messages.
It is not a bearer credential: the original requester must still authenticate.

The helpers do not add retry loops. After a lost response, explicitly reuse the
same message ID and unchanged input; a new ID represents new work. SDK default
transports reject service redirects. A custom transport must keep retries and
redirects disabled for this surface. HTTP failures preserve their status and
server error body. Observation never resumes or starts provider work.
