# Permits and closures in a governed city

A city gives participants room to choose their work while controlling where
they can operate. This scenario gives a human operator management authority and
an agent principal permission to submit incident operations. Both use the same
Acteon server; each has a separate credential and independently bounded rights.

The simulation starts an actual server and a local HTTP receiver. It invokes
the Python SDK's typed management and dispatch methods, counts real provider
POSTs, and writes a JSON report. Business payloads are synthetic. No language
model or autonomous peer selection is invoked in this scenario.

## Run it

From the repository root:

```bash
cargo build -p acteon-server --no-default-features
python3 -m pip install -e clients/python
python3 examples/governed-city/run.py --output /tmp/governed-city-results.json
```

The runner selects unused loopback ports, creates temporary authentication
files and secrets, and cleans up its own server and receiver. The checked-in
configuration uses the memory state backend; it needs no Redis. PostgreSQL
restart persistence is exercised separately by the real-server integration
suite, including a closure remaining active across restart.

## What happens

| Step | Expected outcome | Actual webhook calls |
|---|---|---:|
| Agent tries to manage governance | HTTP 403 | 0 |
| Human operator issues a bounded permit | Committed authority generation | 0 |
| Agent executes a permitted action | Executed | 1 |
| Agent retries the same action ID | Saved outcome | 1 |
| Operator closes an exact effect resource | Committed authority generation | 1 |
| Agent attempts another action | Refused | 1 |
| Operator reopens the resource; agent acts | Executed | 2 |
| Operator revokes the permit; agent acts | Refused | 2 |
| Operator revokes the logical credential | Subsequent dispatch HTTP 403 | 2 |

Every row asserts the receiver's current call count. Successful completion
requires exactly two authorized sends and zero unauthorized sends. The report
includes individual outcomes, authority generations, the final bounded scope
view and elapsed time. A single run is a correctness demonstration, not a
performance benchmark.

The operator has no provider/action dispatch grant. Its ability to issue and
intervene comes from an explicit management declaration and current authenticated
scope permission. The agent has an executor role and a qualified dispatch grant;
those do not authorize governance management.

## Deployment configuration

The runner uses this checked-in configuration, substituting its selected ports:

<!-- governance-example-file: acteon.toml -->
```toml
# The runner substitutes ephemeral ports; no external state service is required.
[server]
host = "127.0.0.1"
port = 18080
[state]
backend = "memory"
[ui]
enabled = false
[auth]
enabled = true
config_path = "auth.toml"
watch = false
[auth.authority]
namespace = "auth-control"
tenant = "deployment"
source_id = "city-auth"
bootstrap = true
[[providers]]
name = "incident"
type = "webhook"
url = "http://127.0.0.1:18081/incident"
internal_hosts = ["127.0.0.1"]
[[execution_authority.scopes]]
namespace = "prod"
tenant = "acme"
bootstrap = true
publisher = {id="operations-owner",kind="human"}
subjects = [{id="operator",kind="human"},{id="agent/maya",kind="agent"}]
routes = [{provider="incident",action_type="execute"}]
valid_from_ms = 0
credential_limits = {max_units=5,max_concurrent=2,deadline_ms=4102444800000}
root_max_units = 5
root_max_concurrent = 1
root_lifetime_ms = 60000
[[execution_authority.scopes.managers]]
principal = {id="operator",kind="human"}
subjects = [{id="agent/maya",kind="agent"}]
routes = [{provider="incident",action_type="execute"}]
valid_from_ms = 0
limits = {max_units=5,max_concurrent=2,deadline_ms=4102444800000}
can_issue_permits = true
can_intervene = true
```

See [governance management](../features/governance.md) for credential enrollment,
the API models, typed SDK methods, conflict/replay behavior and the Admin UI.
See [execution permits](../features/execution-permits.md) for selected-provider
qualification and execution coverage.

## Keep evidence after shutting down an integration

In an operating deployment, permit revocation and provider retirement can leave
work that still needs investigation. Give the operator an independent
`can_read_history = true` management grant with a bounded subject allowlist.
Issuance and intervention permissions alone do not grant access to these records.

When a pending action or chain reports a provider execution UUID, retain that
reference. It is distinct from the original action ID. An authorized operator can
inspect its verified receipt through the typed client:

```python
history = client.provider_execution_history("prod", "acme", provider_execution_id)
print(history.subject.id, history.receipt.status.state)
for attempt in history.attempts:
    print(attempt.ordinal, attempt.ledger_status, attempt.original_outcome)
    if attempt.reconciliation is not None:
        print(attempt.reconciliation.verifier_revision, attempt.reconciliation.outcome)
```

The signed participant identifies whose work produced the evidence. Original
provider observations and accepted reconciliation remain separate, so an
uncertain or failed observation is never silently rewritten as a successful
send. A completed receipt can contain a failed outcome; missing evidence does
not establish that no effect happened.

To remove every live provider while retaining this access, use a reviewed
[history-only deployment](../features/execution-permits.md#retire-execution-while-retaining-history).
It connects to the existing configured state backend and retains the original
context keys and reviewed historical effects. Inspection does not start or retry
work, restore a permit, or release a reserved budget. This operating-deployment
extension is covered by the retirement contract; the simulation above exercises
the immediate execution and intervention loop.

This establishes an observable control loop for immediate qualified operations.
The [agent workforce](agent-workforce.md) scenario covers team representation,
and the governed peer runtime now provides safe registry discovery plus durable
send, refresh, input continuation, remote verifier-backed authorization, and
at-most-once cancellation. This particular runner does not invoke those paths.
Agents can choose among the returned safe options, while every lifecycle step
independently rechecks authority and binding state. Shared delegated budget
conservation remains separate runtime and scenario work.
