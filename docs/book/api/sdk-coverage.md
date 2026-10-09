# SDK coverage and wire contracts

Use Acteon's SDKs to connect agents, services, and workers to the same execution and governance platform. Start with the typed helpers for dispatch, rules, audit, approvals, and bus operations. Use the complete **platform operation API** for controls that do not yet have a dedicated helper in your language.

The current source tree provides a generated catalog for **211 finite HTTP operations** in Rust, Python, TypeScript, Go, and Java. It includes receipt sessions, managed-stage recovery, workflow and execution controls, inference profiles, stream windows, and operator APIs. Six streaming or polymorphic RPC routes use the existing streaming and A2A clients instead. Server configuration, authorization, and optional build features still determine which operations are available on your deployment.

## Choose the right interface

| Capability | Rust | Python | TypeScript | Go | Java |
|---|---|---|---|---|---|
| Typed dispatch, batch, rules, audit and bus helpers | Yes | Yes | Yes | Yes | Yes |
| Complete finite HTTP operation catalog | Yes | Yes, sync and async | Yes | Yes | Yes |
| Native streaming and A2A helpers | Yes | Yes | Yes | Yes | Yes |
| Authenticated agent-service receipts, observation and future-start stop | Yes | Yes, sync and async | Yes | Yes | Yes |
| Governed registry inspection, mutation and explicit recovery | Yes | Yes, sync and async | Yes | Yes | Yes |
| Code-defined workflow runner | — | Yes | Yes | — | — |
| Managed stream-processing adapter | `stream-processing` feature | — | — | — | — |

Every SDK's A2A factory surface includes an input-response helper that fixes
role `user`, `taskId`, optional `contextId`, and
`metadata["acteon.challengeId"]` together: Rust `a2a_input_response`, Python
`make_input_response`, TypeScript `makeInputResponse`, Go `MakeInputResponse`,
and Java `A2A.makeInputResponse`. Pass the current Task's
`pendingApprovalId`; the server rejects stale challenge bindings.

A complete HTTP client is not a workflow runtime. For example, Go and Java can start, inspect, signal, and cancel workflows through the operation API; Python and TypeScript also provide the replay-aware runner that executes a code-defined workflow. See [durable workflows](../features/workflows.md) for the runtime contract.

## Inspect a managed stage

These examples call the same authenticated endpoint, `GET /v1/bus/stages/{namespace}/{tenant}/{id}`. They assume a configured bus and an existing stage named `log-detector`. Supply an API key authorized for that scope. Use the SDK source from the matching server revision when adopting newly added operations.

=== "Python"

    ```python
    import os
    from acteon_client import ActeonClient, PlatformOperation

    with ActeonClient("http://localhost:8080", api_key=os.environ["ACTEON_API_KEY"]) as client:
        status = client.platform_request(
            PlatformOperation.BUS_STAGES_STATUS,
            path={"namespace": "observability", "tenant": "demo", "id": "log-detector"},
        )
        print(status)
    ```

    `AsyncActeonClient` exposes the same method with `await`.

=== "TypeScript"

    ```typescript
    import { ActeonClient } from "@acteon/client";

    const client = new ActeonClient("http://localhost:8080", {
      apiKey: process.env.ACTEON_API_KEY,
    });
    const status = await client.platformRequest("bus_stages_status", {
      path: { namespace: "observability", tenant: "demo", id: "log-detector" },
    });
    console.log(status);
    ```

=== "Go"

    ```go
    client := acteon.NewClient("http://localhost:8080", acteon.WithAPIKey(os.Getenv("ACTEON_API_KEY")))
    status, err := client.PlatformRequest(ctx, acteon.OpBusStagesStatus,
        map[string]string{"namespace": "observability", "tenant": "demo", "id": "log-detector"},
        nil, nil)
    if err != nil { return err }
    fmt.Println(string(status))
    ```

=== "Java"

    ```java
    try (var client = new ActeonClient("http://localhost:8080", System.getenv("ACTEON_API_KEY"))) {
        var status = client.platformRequest(PlatformOperation.BUS_STAGES_STATUS,
            Map.of("namespace", "observability", "tenant", "demo", "id", "log-detector"),
            null, null);
        System.out.println(status);
    }
    ```

=== "Rust"

    ```rust
    use acteon_client::{ActeonClient, PlatformOperation};

    let client = ActeonClient::builder("http://localhost:8080")
        .api_key(std::env::var("ACTEON_API_KEY")?)
        .build()?;
    let status = client.platform_request(
        PlatformOperation::BusStagesStatus,
        &[("namespace", "observability"), ("tenant", "demo"), ("id", "log-detector")],
        &[], None,
    ).await?;
    println!("{status}");
    ```

## Request and response rules

The operation catalog fixes the method and route; callers supply exact path parameter names, query values, and a JSON body using **server wire names**, such as `request_id` and `receipt_ids`. Path values are escaped as individual segments. Empty values, `.` and `..`, missing parameters, extra parameters, and bodies on GET operations are rejected before sending.

Responses retain the server's JSON envelope. Python returns decoded values, TypeScript returns `unknown`, Go returns `json.RawMessage`, Java returns `JsonNode`, and Rust returns `serde_json::Value`. Validate or deserialize a response into application types before acting on it. This API does not invent client-side DTOs for every operation.

Text responses, such as Prometheus metrics, become strings (JSON strings in Go and Rust). HTTP 204 becomes null. Non-success HTTP responses retain their status and response text in the SDK's HTTP error type. None of these calls retry automatically.

For receipt sessions and recovery operations, retain the caller-generated request ID across retries. Pass opaque receipt IDs back unchanged; do not replace them with Kafka offsets. Creating a receipt-required subscription uses `receipt_required` in Python/Go and wire JSON, or `receiptRequired` in the TypeScript and Java typed models. Subscription responses retain both receipt mode and the server's scoped `consumer_group`.

The generic A2A REST cancellation route accepts the router's literal `id` parameter, including its `:cancel` suffix. Prefer the dedicated A2A cancel helper, which constructs the protocol request for you. Use native streaming APIs for SSE, bus consumption streams, and A2A RPC streaming; the finite operation API does not buffer these streams.

## Scope and compatibility

Typed topic lookup and subscription lookup filter the server's scoped list endpoints; there are no individual GET routes for those resources. Topic deletion resolves the registered `kafka_name` before calling the delete endpoint. These helpers retain their existing public signatures.

Some older SDKs expose plugin registration, individual plugin lookup, and invocation methods. The current server does **not** register those HTTP routes. They are not supported capabilities of this server revision. The server exposes plugin listing and a deletion route that may report `501 Not Implemented`; configure WASM plugins through server configuration. These legacy methods are excluded from the generated catalog.

## Keep clients and server together

[`scripts/sdk/platform_catalog.py`](https://github.com/penserai/acteon/blob/main/scripts/sdk/platform_catalog.py) reads registered routes and generates the five language catalogs plus a shared contract fixture. CI runs:

```bash
python3 scripts/sdk/platform_catalog.py --check
```

After adding or changing a route, regenerate the catalogs, run each language's formatter and tests, and update the relevant typed helper and documentation. The transport tests check methods, authentication, escaping, query values, request bodies, response envelopes, and error handling. The [polyglot simulation](polyglot-clients.md#testing-with-polyglot-simulation) exercises actual clients against a running in-memory server.

## Dispatch outcomes

All clients recognize the server's 18 dispatch variants, including `Grouped`, `StateChanged`, `PendingApproval`, `ChainStarted`, `CircuitOpen`, `RecurringCreated`, `Silenced`, and `Muted`. Single and batch dispatch preserve their fields, including approval capabilities and chain IDs. A pending approval or a started chain is not a completed provider execution; inspect the outcome before advancing an agent workflow.

`ProviderPending` preserves the original provider execution ID, registered
attempt count, and a typed state (`in_flight`, `reconciliation_required`, or
`awaiting_retry`). It is not a completed failure and must not trigger a fresh
send. Python exposes `outcome.pending`, Node `outcome.pending`, Go
`outcome.Pending`, Java `outcome.getPending()`, and Rust
`ActionOutcome::ProviderPending(work)`. Chain detail responses preserve
`wait_state` (`waitState` in Node and Java), including the retained receipt IDs.

## Governance management

All five SDKs expose native typed scope inspection, permit publication, and intervention methods. See [governance management](../features/governance.md) for method names and authority requirements. The shared governance fixture checks request bodies, authentication, typed responses, and HTTP refusal preservation; actual authority enforcement is tested separately against the server and configured state backend.

## Governed registry metadata

Every SDK has typed helpers for an exact agent/card projection and its versioned
mutation. Rust uses `registry_projection`/`mutate_registry`; Python uses the same
names in synchronous and asynchronous clients; TypeScript and Java use
`registryProjection`/`mutateRegistry`; Go uses
`RegistryProjection`/`MutateRegistry`. The clients send once, refuse redirects,
retain HTTP refusal status, and require a correlated completed/applied receipt.

Keep the complete mutation and caller-owned change ID in durable host state. A
lost acknowledgement is recovered only by explicitly replaying that exact
request. A conflict requires a fresh inspection and reviewed intent. The shared
wire fixture checks all five clients, while the same create/update/delete/recreate
lifecycle runs against memory, Redis, PostgreSQL, and DynamoDB. See
[managing registry projections](../features/governance.md#managing-registry-projections).

## Workforce management

Every SDK supplies typed workforce inspection and all ten mutation variants.
Rust and Python use `workforce`/`change_workforce`; TypeScript and Java use
`workforce`/`changeWorkforce`; Go uses `Workforce`/`ChangeWorkforce`. Python
supports both synchronous and asynchronous clients. The shared wire contract
verifies all variants, scoped authentication, nested record decoding and HTTP
refusals without mutation retries. See [Agent workforce](../features/workforce.md).

## Historical provider execution evidence

`GET /v1/governance/executions/{execution_id}?namespace=prod&tenant=acme`
returns verified provider receipts, original evidence and accepted reconciliation
records from the deployment's configured state backend. The operator must have
current scope management authority and an explicit `can_read_history = true`
deployment grant. The manager's `subjects` list limits whose work can be read.
Permit issuance and intervention grants do not imply history access.

The operation catalog exposes `governance_provider_history` in all five SDKs.
Every SDK provides a typed `ProviderExecutionHistory` response and a dedicated helper:

| SDK | Method |
|---|---|
| Rust | `provider_execution_history(namespace, tenant, execution_id)` |
| Python (sync and async) | `provider_execution_history(namespace, tenant, execution_id)` |
| TypeScript | `providerExecutionHistory(namespace, tenant, executionId)` |
| Go | `ProviderExecutionHistory(ctx, namespace, tenant, executionID)` |
| Java | `providerExecutionHistory(namespace, tenant, executionId)` |

Nested outcomes use each SDK's existing outcome model. Original evidence and
accepted reconciliation remain distinct; a completed receipt may contain a failed
outcome. The shared fixture is checked against Rust's public DTO and exercises all
five receipt states across clients. Client decoding tests do not establish server
authorization or backend durability.
See [historical receipts](../features/durable-executions.md#historical-receipts)
for evidence integrity and recovery semantics.


## Retain an authenticated agent-service receipt

Individual governed services expose `POST
/a2a/{namespace}/{tenant}/agents/{agent}/v1/message:send` and `GET
/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}`. These require a configured
service and the requester's original private authentication; a registry card
alone does not install a runtime or authorize an invocation.

All five SDKs expose native acceptance and observation helpers:

| SDK | Accept message | Accept governed child | Observe retained receipt |
| --- | --- | --- | --- |
| Rust | `agent_service_send_message` | `agent_service_send_message_with_parent` | `agent_service_get_task` |
| Python, sync/async | `agent_service_send_message` | `parent=AgentServiceParent(...)` | `agent_service_get_task` |
| TypeScript | `agentServiceSendMessage` | `parent: AgentServiceParentOptions` | `agentServiceGetTask` |
| Go | `AgentServiceSendMessage` | `AgentServiceSendMessageWithParent` | `AgentServiceGetTask` |
| Java | `agentServiceSendMessage` | overload with `AgentServiceParent` | `agentServiceGetTask` |

The returned receipt retains the `x-acteon-agent-source-context` response header
separately from task data and binds it to the original namespace, tenant, agent,
and task ID. Keep the receipt in host-owned state; do not place it in model
messages. Shared agent callers must supply the exact retained context on reads.
The receipt does not replace authentication. The SDKs never substitute metadata
for a missing header, and observation does not start or resume work.

Use a stable message ID. If an admission response is lost, explicitly replay the
same unchanged message under current invocation authority. A fresh ID creates
new work. SDK default transports reject redirects and the helpers introduce no
retry loops; custom transports must retain those constraints. Configured CORS
origins can read the provenance and A2A version headers.

For peer invocation, all SDKs can carry the existing host-owned execution
context and explicit permit references. Acteon treats the encoded context as an
opaque lookup reference: the server recovers sealed state and revalidates the
authenticated principal, permit revisions, registry, delegation grant, and
budgets. Context and permit headers are required together. Model messages and
task metadata are never authority sources.

An accepted agent can invoke one configured peer without receiving those
authority references. The helper uses the source service receipt only to address
the accepted task; it sends the target, skill and message while current private
authentication proves the source recipient:

Before selection, the SDKs can request current safe options for one exact skill.
The server intersects the source agent's reviewed onward list, live registry and
card state, installed binding, and caller grant. Returned descriptions are
explicitly untrusted. Options contain no endpoint or authority material and do
not authorize a later send.

| SDK | Safe peer discovery |
| --- | --- |
| Rust | `agent_service_discover_peers(&source, skill)` |
| Python, sync/async | `agent_service_discover_peers(source, skill)` |
| TypeScript | `agentServiceDiscoverPeers(source, skill)` |
| Go | `AgentServiceDiscoverPeers(ctx, source, skill)` |
| Java | `agentServiceDiscoverPeers(source, skill)` |

| SDK | Governed peer submission |
| --- | --- |
| Rust | `agent_service_send_peer(&source, target, skill, &message)` |
| Python, sync/async | `agent_service_send_peer(source, target, skill, message)` |
| TypeScript | `agentServiceSendPeer(source, target, skill, message)` |
| Go | `AgentServiceSendPeer(ctx, source, target, skill, message)` |
| Java | `agentServiceSendPeer(source, target, skill, message)` |

The typed receipt preserves `accepted`, `rejected`, and `uncertain` as distinct
states and validates the stable submission UUID and accepted task scope. Helpers
send one request, reject redirects, and never turn uncertainty into retry.

Once accepted, the same five clients expose `agent_service_refresh_peer`,
`agentServiceRefreshPeer`, or `AgentServiceRefreshPeer` according to language
conventions. Refresh sends the stable submission ID with the retained source
receipt. It revalidates current authority, reads the remote task once, journals
only a valid forward snapshot, and never resubmits the original message.

Accepted peer receipts also support durable cancellation through
`agent_service_cancel_peer`, `agentServiceCancelPeer`, or
`AgentServiceCancelPeer`. The helper sends one request and validates the stable
submission and cancellation UUIDs, exact remote task identity, tenant scope,
and lifecycle state. It exposes `unsupported`, `rejected`, `uncertain`,
`restricted`, and `reconciled` without collapsing them. `restricted` proves the
remote service durably blocked future starts and carries the current nonterminal
task; it does not claim an in-flight provider effect stopped. Never retry an
uncertain cancellation automatically. Call cancellation explicitly again to let
Acteon reconcile by a safe task read, or call peer refresh when only the latest
lifecycle snapshot is needed. A reconciled `completed` or `failed` task means
the task became final before cancellation took effect. When the original target
acknowledgment was lost, a nonterminal observation keeps the cancellation
`uncertain`; the repeat does not send another stop request.

The admin UI's **Governed tasks** tab on an agent detail page accepts work and
refreshes accepted tasks using the current browser identity. The server enforces
its configured service, source permits, and recipient authority. The tab retains
receipts in memory while it is open, reuses the original request on explicit
retry, and displays task state and result artifacts without displaying source
context. Leaving the tab clears this local view. SDK host persistence supports
longer-lived clients.


## Stop future agent-service starts

The service control extension `POST
/a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/stop` uses the original
requester's private credential and admission source-context header. Agent
requesters must send the exact `x-acteon-agent-source-context` returned for that
job. The finite operation catalog includes `agent_services_task_stop` in every
SDK; dedicated receipt-aware stop helpers retain the original job ID and source
context when invoking this control.

| SDK | Stop helper | Acknowledgement |
| --- | --- | --- |
| Rust | `agent_service_stop_task(&receipt)` | `AgentServiceStopReceipt` |
| Python sync/async | `agent_service_stop_task(receipt)` | `AgentServiceStopReceipt` |
| TypeScript | `agentServiceStopTask(receipt)` | `AgentServiceStopReceipt` |
| Go | `AgentServiceStopTask(ctx, receipt)` | `AgentServiceStopReceipt` |
| Java | `agentServiceStopTask(receipt)` | `AgentServiceStopReceipt` |

All helpers send one request, validate the original task identity and exact
restriction acknowledgement, and reject redirects. The agent detail view provides
**Stop future starts** for each locally retained job. A failed acknowledgement
keeps that job available for an explicit retry; a successful acknowledgement
keeps the restriction visible alongside later provider results.

A successful response is `{"task": ..., "future_starts_blocked": true,
"provider_abort": ...}`. `provider_abort` is optional and has one typed state:
`restricted_only`, `uncertain` with the stable provider attempt ID, or
`reconciled` with the accepted finality-proof digest. Every SDK validates this
shape, including the canonical lowercase UUIDv5 attempt ID and lowercase
SHA-256 proof digest. The browser explains the same distinction beside the task.

The restriction blocks future starts in the recipient execution subtree and survives
restart. It leaves the task's status and artifacts tied to actual provider
results. `restricted_only` and `uncertain` do not undo an effect already
delivered, release unresolved capacity, or prove that an external provider
aborted. For a lost acknowledgement, repeat the stop against the same original
task. A subsequent known completion can still appear in observation.
