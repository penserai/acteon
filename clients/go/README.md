# acteon-client (Go)

Go client for the Acteon action gateway.

## Complete platform API

The generated operation catalog exposes all 191 finite HTTP operations, including receipt sessions, managed stages, workflows, execution controls, inference profiles, and stream windows. Use an authenticated client and a configured, existing stage for this example:

```go
status, err := client.PlatformRequest(ctx, acteon.OpBusStagesStatus,
    map[string]string{"namespace": "observability", "tenant": "demo", "id": "log-detector"},
    nil, nil)
if err != nil { return err }
fmt.Println(string(status))
```

Path parameters are escaped individually. Query and body fields use the server's wire names; JSON response envelopes are preserved. Calls do not retry automatically. Keep request IDs stable across retries and return opaque receipt IDs unchanged. HTTP access does not imply a code-defined workflow runner in every language.

See [SDK coverage and wire contracts](https://penserai.github.io/acteon/api/sdk-coverage/) for return types, native streaming APIs, runtime availability, and server feature requirements. The catalog is generated from the registered server routes and checked in CI.

## Typed governance management

Use `Governance`, `PublishGovernancePermit`, and `InterveneGovernance` to inspect permitted routes, issue bounded permits, and close/reopen resources or revoke permits, subjects, and credentials. Requests and responses have native typed models. These methods require an authenticated operator and an independently configured execution manager for the requested namespace and tenant; an executor credential alone does not grant management authority.

Calls preserve HTTP refusals and do not retry automatically. Keep `change_id` stable when reconciling a lost response. A receipt records persisted authority; closure affects subsequent starts and does not promise cancellation of work already in flight.

See [governance management](https://penserai.github.io/acteon/features/governance/) for deployment policy and wire examples, and [the governed city simulation](https://penserai.github.io/acteon/guides/governed-city/) for real authenticated requests.

## Installation

```bash
go get github.com/penserai/acteon/clients/go/acteon
```

## Quick Start

```go
package main

import (
    "context"
    "fmt"
    "log"

    "github.com/penserai/acteon/clients/go/acteon"
)

func main() {
    client := acteon.NewClient("http://localhost:8080")
    ctx := context.Background()

    // Check health
    healthy, _ := client.Health(ctx)
    if healthy {
        fmt.Println("Server is healthy")
    }

    // Dispatch an action
    action := acteon.NewAction(
        "notifications",
        "tenant-1",
        "email",
        "send_notification",
        map[string]any{"to": "user@example.com", "subject": "Hello"},
    )

    outcome, err := client.Dispatch(ctx, action)
    if err != nil {
        log.Fatal(err)
    }
    fmt.Printf("Outcome: %s\n", outcome.Type)
}
```

## Batch Dispatch

```go
actions := make([]*acteon.Action, 10)
for i := range actions {
    actions[i] = acteon.NewAction("ns", "t1", "email", "send", map[string]any{"i": i})
}

results, err := client.DispatchBatch(ctx, actions)
if err != nil {
    log.Fatal(err)
}

for _, result := range results {
    if result.Success {
        fmt.Printf("Success: %s\n", result.Outcome.Type)
    } else {
        fmt.Printf("Error: %s\n", result.Error.Message)
    }
}
```

## Handling Outcomes

```go
outcome, err := client.Dispatch(ctx, action)
if err != nil {
    log.Fatal(err)
}

switch outcome.Type {
case acteon.OutcomeExecuted:
    fmt.Println("Executed:", outcome.Response.Body)
case acteon.OutcomeDeduplicated:
    fmt.Println("Already processed")
case acteon.OutcomeSuppressed:
    fmt.Printf("Suppressed by rule: %s\n", outcome.Rule)
case acteon.OutcomeRerouted:
    fmt.Printf("Rerouted: %s -> %s\n", outcome.OriginalProvider, outcome.NewProvider)
case acteon.OutcomeThrottled:
    fmt.Printf("Retry after %v\n", outcome.RetryAfter)
case acteon.OutcomeFailed:
    fmt.Printf("Failed: %s\n", outcome.Error.Message)
}
```

## Convenience Methods

```go
if outcome.IsExecuted() {
    fmt.Println("Executed successfully")
}

if outcome.IsDeduplicated() {
    fmt.Println("Duplicate detected")
}
```

## Rule Management

```go
// List all rules
rules, err := client.ListRules(ctx)
for _, rule := range rules {
    fmt.Printf("%s: priority=%d, enabled=%t\n", rule.Name, rule.Priority, rule.Enabled)
}

// Reload rules from disk
result, err := client.ReloadRules(ctx)
fmt.Printf("Loaded %d rules\n", result.Loaded)

// Disable a rule
err = client.SetRuleEnabled(ctx, "block-spam", false)
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

```go
outcome, err := client.Dispatch(ctx, action, acteon.WithDryRun())
if outcome.IsDryRun() {
    fmt.Printf("Verdict: %s\n", outcome.Verdict)          // e.g. "suppress"
    fmt.Printf("Matched rule: %s\n", outcome.MatchedRule)  // e.g. "suppress-outside-hours"
}
```

Available `time` fields: `hour` (0–23), `minute`, `second`, `day`, `month`, `year`, `weekday` (`"Monday"`…`"Sunday"`), `weekday_num` (1=Mon…7=Sun), `timestamp`.

## Audit Trail

```go
// Query audit records
query := &acteon.AuditQuery{Tenant: "tenant-1", Limit: 10}
page, err := client.QueryAudit(ctx, query)
fmt.Printf("Found %d records\n", page.Total)
for _, record := range page.Records {
    fmt.Printf("  %s: %s\n", record.ActionID, record.Outcome)
}

// Get specific record
record, err := client.GetAuditRecord(ctx, "action-id-123")
if record != nil {
    fmt.Printf("Found: %s\n", record.Outcome)
}
```

## Configuration

API keys are sent via the `Authorization: Bearer <key>` header. The server
accepts both JWTs and raw API keys on that header. API keys are scoped by
tenant, namespace, provider, and action type on the server side — see the
[API Key Scoping](https://penserai.github.io/acteon/features/api-key-scoping/)
documentation for the grant model and hierarchical tenant matching.

```go
client := acteon.NewClient(
    "http://localhost:8080",
    acteon.WithTimeout(60*time.Second),
    acteon.WithAPIKey("your-key"),
)

// Or with a custom HTTP client
httpClient := &http.Client{
    Timeout: 60 * time.Second,
    Transport: &http.Transport{
        MaxIdleConns: 100,
    },
}
client := acteon.NewClient(
    "http://localhost:8080",
    acteon.WithHTTPClient(httpClient),
)
```

## Error Handling

```go
import "errors"

outcome, err := client.Dispatch(ctx, action)
if err != nil {
    var connErr *acteon.ConnectionError
    var apiErr *acteon.APIError
    var httpErr *acteon.HTTPError

    switch {
    case errors.As(err, &connErr):
        fmt.Printf("Connection failed: %s\n", connErr.Message)
        if connErr.IsRetryable() {
            // Retry logic
        }
    case errors.As(err, &apiErr):
        fmt.Printf("API error [%s]: %s\n", apiErr.Code, apiErr.Message)
        if apiErr.IsRetryable() {
            // Retry logic
        }
    case errors.As(err, &httpErr):
        fmt.Printf("HTTP %d: %s\n", httpErr.Status, httpErr.Message)
    }
}
```

## API Reference

### Client Methods

| Method | Description |
|--------|-------------|
| `Health(ctx)` | Check server health |
| `Dispatch(ctx, action)` | Dispatch a single action |
| `DispatchBatch(ctx, actions)` | Dispatch multiple actions |
| `ListRules(ctx)` | List all loaded rules |
| `ReloadRules(ctx)` | Reload rules from disk |
| `SetRuleEnabled(ctx, name, enabled)` | Enable/disable a rule |
| `QueryAudit(ctx, query)` | Query audit records |
| `GetAuditRecord(ctx, actionID)` | Get specific audit record |
| `FetchSigningKeys(ctx)` | Fetch the server's active signing keyring (JWKS-style discovery) |

### Action Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `Namespace` | string | Yes | Logical grouping |
| `Tenant` | string | Yes | Tenant identifier |
| `Provider` | string | Yes | Target provider |
| `ActionType` | string | Yes | Type of action |
| `Payload` | map[string]any | Yes | Action-specific data |
| `ID` | string | No | Auto-generated UUID |
| `DedupKey` | string | No | Deduplication key |
| `Metadata` | *ActionMetadata | No | Key-value metadata |

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

`client.Identity(ctx)` inspects the authenticated credential and its optional stable
principal binding. Configure `principal = { id = "diagnostic-agent", kind = "agent" }`
on the credential in server `auth.toml`; keep it unchanged across credential
rotation. Roles and grants still apply independently to each credential. Legacy
credentials return a null principal. Principal metadata does not grant execution
permits or bind a bus agent automatically. See the
[authentication guide](https://penserai.github.io/acteon/api/authentication/).

The identity response also exposes the optional `AuthorityID` logical enrollment ID configured
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

```go
permits := []acteon.PermitReference{{ID: "maya-incident", AcceptedRevision: 1}}
outcome, err := client.DispatchWithPermits(ctx, action, permits)
batch, err := client.DispatchBatchWithPermits(ctx, []*acteon.Action{action}, permits)
```

The same references apply to every action in a batch. Dry runs can omit them.
Missing/invalid headers, authorization denials and conflicting replay requests
preserve their HTTP status in the client's HTTP error type. Permit admission
refusals can also be returned as a `Failed` action outcome; inspect the outcome.
Do not generate a new action ID merely to bypass a replay conflict.
