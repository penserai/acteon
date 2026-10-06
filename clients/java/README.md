# acteon-client (Java)

Java client for the Acteon action gateway.

## Complete platform API

The generated operation catalog exposes all 197 finite HTTP operations, including receipt sessions, managed stages, workflows, execution controls, inference profiles, and stream windows. Use an authenticated client and a configured, existing stage for this example:

```java
var status = client.platformRequest(PlatformOperation.BUS_STAGES_STATUS,
    Map.of("namespace", "observability", "tenant", "demo", "id", "log-detector"),
    null, null);
```

Path parameters are escaped individually. Query and body fields use the server's wire names; JSON response envelopes are preserved. Calls do not retry automatically. Keep request IDs stable across retries and return opaque receipt IDs unchanged. HTTP access does not imply a code-defined workflow runner in every language.

See [SDK coverage and wire contracts](https://penserai.github.io/acteon/api/sdk-coverage/) for return types, native streaming APIs, runtime availability, and server feature requirements. The catalog is generated from the registered server routes and checked in CI.

## Requirements

- Java 21+
- Maven or Gradle

## Typed governance management

Use `governance`, `publishGovernancePermit`, and `interveneGovernance` to inspect permitted routes, issue bounded permits, and close/reopen resources or revoke permits, subjects, and credentials. Requests and responses have native typed models. These methods require an authenticated operator and an independently configured execution manager for the requested namespace and tenant; an executor credential alone does not grant management authority.

Calls preserve HTTP refusals and do not retry automatically. Keep `change_id` stable when reconciling a lost response. A receipt records persisted authority; closure affects subsequent starts and does not promise cancellation of work already in flight.

See [governance management](https://penserai.github.io/acteon/features/governance/) for deployment policy and wire examples, and [the governed city simulation](https://penserai.github.io/acteon/guides/governed-city/) for real authenticated requests.

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

API keys are sent via the `Authorization: Bearer <key>` header. The server
accepts both JWTs and raw API keys on that header. API keys are scoped by
tenant, namespace, provider, and action type on the server side — see the
[API Key Scoping](https://penserai.github.io/acteon/features/api-key-scoping/)
documentation for the grant model and hierarchical tenant matching.

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
| `fetchSigningKeys()` | Fetch the server's active signing keyring (JWKS-style discovery) |

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

`client.identity()` inspects the authenticated credential and its optional stable
principal binding. Configure `principal = { id = "diagnostic-agent", kind = "agent" }`
on the credential in server `auth.toml`; keep it unchanged across credential
rotation. Roles and grants still apply independently to each credential. Legacy
credentials return a null principal. Principal metadata does not grant execution
permits or bind a bus agent automatically. See the
[authentication guide](https://penserai.github.io/acteon/api/authentication/).

The identity response also exposes the optional `authorityId()` logical enrollment ID configured
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

```java
var permits = List.of(new PermitReference("maya-incident", 1));
var outcome = client.dispatch(action, permits);
var batch = client.dispatchBatch(List.of(action), permits);
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

```java
var roster = client.workforce("prod", "acme");
var receipt = client.changeWorkforce(new Workforce.ChangeRequest(
    "prod", "acme", "enroll-reliability-1",
    new Workforce.PutTeam(new Workforce.Team(
        new Workforce.TeamRef("prod", "acme", "reliability"), 1, "Reliability")),
    "Establish incident response"));
```

Import `com.acteon.client.models.Workforce` for the typed models.

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

```java
var history = client.providerExecutionHistory("prod", "acme", executionId);
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

```java
var correlation = client.providerReconciliationCorrelation("prod", "acme", executionId, 0);
// evidenceBase64 comes from the independently qualified finality source.
var receipt = client.acceptProviderReconciliation("prod", "acme", executionId, 0,
    new ProviderReconciliation.Request(evidenceBase64));
```

These methods preserve the completed or definitive no-effect receipt. Correlation
is not a permit; proof bytes must come from the qualified source. After a lost
acknowledgment, replay the exact proof under current authority rather than changing
its content or verifier revision.
