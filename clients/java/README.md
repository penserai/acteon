# acteon-client (Java)

Java client for the Acteon action gateway.

## Requirements

- Java 21+
- Maven or Gradle

## Installation

### Maven

```xml
<dependency>
    <groupId>com.acteon</groupId>
    <artifactId>acteon-client</artifactId>
    <version>0.1.0</version>
</dependency>
```

### Build from source

```bash
cd clients/java
mvn clean install
```

## Quick Start

```java
import com.acteon.client.ActeonClient;
import com.acteon.client.models.*;

import java.util.Map;

public class Example {
    public static void main(String[] args) throws Exception {
        ActeonClient client = new ActeonClient("http://localhost:8080");

        // Check health
        if (client.health()) {
            System.out.println("Server is healthy");
        }

        // Dispatch an action
        Action action = new Action(
            "notifications",
            "tenant-1",
            "email",
            "send_notification",
            Map.of("to", "user@example.com", "subject", "Hello")
        );

        ActionOutcome outcome = client.dispatch(action);
        System.out.println("Outcome: " + outcome.getType());
    }
}
```

## Builder Pattern

```java
Action action = Action.builder()
    .namespace("notifications")
    .tenant("tenant-1")
    .provider("email")
    .actionType("send_notification")
    .payload(Map.of("to", "user@example.com"))
    .dedupKey("unique-key")
    .labels(Map.of("env", "production"))
    .build();
```

## Batch Dispatch

```java
List<Action> actions = IntStream.range(0, 10)
    .mapToObj(i -> new Action("ns", "t1", "email", "send", Map.of("i", i)))
    .toList();

List<BatchResult> results = client.dispatchBatch(actions);
for (BatchResult result : results) {
    if (result.isSuccess()) {
        System.out.println("Success: " + result.getOutcome().getType());
    } else {
        System.out.println("Error: " + result.getError().getMessage());
    }
}
```

## Handling Outcomes

```java
ActionOutcome outcome = client.dispatch(action);

switch (outcome.getType()) {
    case EXECUTED -> System.out.println("Executed: " + outcome.getResponse().getBody());
    case DEDUPLICATED -> System.out.println("Already processed");
    case SUPPRESSED -> System.out.println("Suppressed by rule: " + outcome.getRule());
    case REROUTED -> System.out.println("Rerouted: " + outcome.getOriginalProvider() + " -> " + outcome.getNewProvider());
    case THROTTLED -> System.out.println("Retry after " + outcome.getRetryAfter());
    case FAILED -> System.out.println("Failed: " + outcome.getError().getMessage());
}
```

## Convenience Methods

```java
if (outcome.isExecuted()) {
    System.out.println("Executed successfully");
}

if (outcome.isDeduplicated()) {
    System.out.println("Duplicate detected");
}
```

## Rule Management

```java
// List all rules
List<RuleInfo> rules = client.listRules();
for (RuleInfo rule : rules) {
    System.out.printf("%s: priority=%d, enabled=%b%n", rule.getName(), rule.getPriority(), rule.isEnabled());
}

// Reload rules from disk
ReloadResult result = client.reloadRules();
System.out.println("Loaded " + result.getLoaded() + " rules");

// Disable a rule
client.setRuleEnabled("block-spam", false);
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

```java
ActionOutcome outcome = client.dispatch(action, true); // dry_run = true
if (outcome.isDryRun()) {
    System.out.println("Verdict: " + outcome.getVerdict());        // e.g. "suppress"
    System.out.println("Matched rule: " + outcome.getMatchedRule()); // e.g. "suppress-outside-hours"
}
```

Available `time` fields: `hour` (0–23), `minute`, `second`, `day`, `month`, `year`, `weekday` (`"Monday"`…`"Sunday"`), `weekday_num` (1=Mon…7=Sun), `timestamp`.

## Audit Trail

```java
// Query audit records
AuditQuery query = AuditQuery.builder()
    .tenant("tenant-1")
    .limit(10)
    .build();

AuditPage page = client.queryAudit(query);
System.out.println("Found " + page.getTotal() + " records");
for (AuditRecord record : page.getRecords()) {
    System.out.println("  " + record.getActionId() + ": " + record.getOutcome());
}

// Get specific record
Optional<AuditRecord> record = client.getAuditRecord("action-id-123");
record.ifPresent(r -> System.out.println("Found: " + r.getOutcome()));
```

## Configuration

```java
// With API key
ActeonClient client = new ActeonClient("http://localhost:8080", "your-api-key");

// With custom timeout
ActeonClient client = new ActeonClient(
    "http://localhost:8080",
    "your-api-key",
    Duration.ofSeconds(60)
);
```

## Error Handling

```java
import com.acteon.client.exceptions.*;

try {
    ActionOutcome outcome = client.dispatch(action);
} catch (ConnectionException e) {
    System.out.println("Connection failed: " + e.getMessage());
    if (e.isRetryable()) {
        // Retry logic
    }
} catch (ApiException e) {
    System.out.println("API error [" + e.getCode() + "]: " + e.getMessage());
    if (e.isRetryable()) {
        // Retry logic
    }
} catch (HttpException e) {
    System.out.println("HTTP " + e.getStatus() + ": " + e.getMessage());
}
```

## Try-with-Resources

```java
try (ActeonClient client = new ActeonClient("http://localhost:8080")) {
    ActionOutcome outcome = client.dispatch(action);
}
```

## API Reference

### ActeonClient Methods

| Method | Description |
|--------|-------------|
| `health()` | Check server health |
| `dispatch(action)` | Dispatch a single action |
| `dispatchBatch(actions)` | Dispatch multiple actions |
| `listRules()` | List all loaded rules |
| `reloadRules()` | Reload rules from disk |
| `setRuleEnabled(name, enabled)` | Enable/disable a rule |
| `queryAudit(query)` | Query audit records |
| `getAuditRecord(actionId)` | Get specific audit record |

### Action Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `namespace` | String | Yes | Logical grouping |
| `tenant` | String | Yes | Tenant identifier |
| `provider` | String | Yes | Target provider |
| `actionType` | String | Yes | Type of action |
| `payload` | Map | Yes | Action-specific data |
| `id` | String | No | Auto-generated UUID |
| `dedupKey` | String | No | Deduplication key |
| `metadata` | ActionMetadata | No | Key-value metadata |

## License

Apache-2.0
