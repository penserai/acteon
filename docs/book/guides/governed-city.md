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

This establishes an observable control loop for immediate qualified operations.
It does not establish team representation, autonomous A2A selection, delegated
budget conservation or remote cancellation. Those require their own runtime
contracts and scenarios.
