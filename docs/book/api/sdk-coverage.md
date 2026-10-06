# SDK coverage and wire contracts

Use Acteon's SDKs to connect agents, services, and workers to the same execution and governance platform. Start with the typed helpers for dispatch, rules, audit, approvals, and bus operations. Use the complete **platform operation API** for controls that do not yet have a dedicated helper in your language.

The current source tree provides a generated catalog for **197 finite HTTP operations** in Rust, Python, TypeScript, Go, and Java. It includes receipt sessions, managed-stage recovery, workflow and execution controls, inference profiles, stream windows, and operator APIs. Six streaming or polymorphic RPC routes use the existing streaming and A2A clients instead. Server configuration, authorization, and optional build features still determine which operations are available on your deployment.

## Choose the right interface

| Capability | Rust | Python | TypeScript | Go | Java |
|---|---|---|---|---|---|
| Typed dispatch, batch, rules, audit and bus helpers | Yes | Yes | Yes | Yes | Yes |
| Complete finite HTTP operation catalog | Yes | Yes, sync and async | Yes | Yes | Yes |
| Native streaming and A2A helpers | Yes | Yes | Yes | Yes | Yes |
| Code-defined workflow runner | — | Yes | Yes | — | — |
| Managed stream-processing adapter | `stream-processing` feature | — | — | — | — |

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
