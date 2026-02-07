# acteon-client (Go)

Go client for the Acteon action gateway.

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
