# Rule Coverage

Rule Coverage analyzes the audit trail to tell you **which combinations of
`(namespace, tenant, provider, action_type)` were matched by a rule** within a
time window and which slipped through without any policy applied. It is
designed as a safety-net audit: you can spot blind spots in your rule set
before they become incidents.

The feature is server-side: the gateway aggregates audit records natively
(with `GROUP BY` on Postgres/ClickHouse, or paged aggregation on non-SQL
backends) and returns only the aggregated report. Clients never page through
raw audit records — the client wire payload is bounded by the cardinality of
your dimensions, not the size of your audit trail.

## When to use it

- **Blind-spot detection.** A new provider or tenant was added and you want
  to confirm your safety rules cover it.
- **Dead-rule triage.** You suspect some rules are no longer firing and want
  to see what's actually matching in production.
- **Pre-incident review.** Before a release or compliance audit, verify that
  the rule set matches the deployed namespaces.
- **Operational debugging.** An operator thinks an action "should have been
  blocked" — the coverage report shows whether any rule even saw it.

## Terminology

| Term | Meaning |
|------|---------|
| **Combination** | A unique `(namespace, tenant, provider, action_type)` tuple observed in the audit trail |
| **COVERED** | Every action in the combination matched at least one rule |
| **PARTIAL** | Some actions matched a rule, some did not |
| **UNCOVERED** | No action in the combination matched any rule |
| **Unmatched rule** | An enabled rule that did not match *any* audit record inside the scanned window |
| **Scanned window** | The `scanned_from` → `scanned_to` time range the report summarizes |

## API endpoint

```
GET /v1/rules/coverage
```

### Query parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `namespace` | string | no | Filter by namespace |
| `tenant` | string | no | Filter by tenant (enforced against caller's allowed tenants) |
| `from` | string (RFC 3339) | no | Start of time range. Defaults to 7 days ago |
| `to` | string (RFC 3339) | no | End of time range. Defaults to now |

### Response shape

```json
{
  "scanned_from": "2026-04-08T12:00:00Z",
  "scanned_to": "2026-04-09T12:00:00Z",
  "total_actions": 1284,
  "unique_combinations": 12,
  "fully_covered": 8,
  "partially_covered": 2,
  "uncovered": 2,
  "rules_loaded": 17,
  "entries": [
    {
      "namespace": "prod",
      "tenant": "acme",
      "provider": "webhook",
      "action_type": "post",
      "total": 42,
      "covered": 0,
      "uncovered": 42,
      "matched_rules": []
    },
    {
      "namespace": "prod",
      "tenant": "acme",
      "provider": "email",
      "action_type": "send",
      "total": 120,
      "covered": 120,
      "uncovered": 0,
      "matched_rules": ["block-phishing", "rate-limit-email"]
    }
  ],
  "unmatched_rules": ["legacy-throttle"]
}
```

Entries are returned pre-sorted: **UNCOVERED → PARTIAL → COVERED**, with ties
broken by dimension key. Clients can re-sort locally if they need a different
order.

## CLI

```bash
# Last 24 hours (default), all namespaces
acteon rules coverage

# Last 7 days for a specific namespace
acteon rules coverage --since-hours 168 --namespace prod

# Explicit time range
acteon rules coverage \
  --from 2026-04-01T00:00:00Z \
  --to   2026-04-08T00:00:00Z

# Only show combinations where something slipped through
acteon rules coverage --only-uncovered

# Hide low-volume noise
acteon rules coverage --min-uncovered 10

# Sort by highest miss count first
acteon rules coverage --sort-by miss

# Machine-readable
acteon rules coverage --format json
```

### CLI flags

| Flag | Default | Description |
|------|---------|-------------|
| `--namespace <NS>` | — | Filter by namespace |
| `--tenant <T>` | — | Filter by tenant |
| `--since-hours <N>` | 24 | Scan the last N hours (mutually exclusive with `--from`/`--to`) |
| `--from <RFC3339>` | — | Explicit start of time range |
| `--to <RFC3339>` | — | Explicit end of time range |
| `--only-uncovered` | false | Hide entries with any covered actions |
| `--min-uncovered <N>` | 0 | Hide entries with fewer than N uncovered actions |
| `--sort-by <status\|total\|miss\|name>` | `status` | Sort order for the table |
| `--format <text\|json>` | text | Output format |

### Sample output

```text
Coverage analysis: scanned_from=2026-04-08T12:00:00Z scanned_to=2026-04-09T12:00:00Z total_actions=1284 rules_loaded=17

Coverage summary: combinations=12 fully_covered=8 partially_covered=2 uncovered=2

NAMESPACE  TENANT  PROVIDER  ACTION_TYPE  TOTAL  COVER   MISS  STATUS     RULES
---------------------------------------------------------------------------------------
prod       acme    webhook   post            42      0     42  UNCOVERED  -
prod       acme    sms       send            18      6     12  PARTIAL    allow-sms
prod       acme    email     send           120    120      0  COVERED    block-phishing, rate-limit-email

1 enabled rule(s) with no matches in the scanned window
  NOTE: This is window-scoped — a rule listed here may still be live if
  it triggers rarely and simply did not fire inside the queried time
  range. Verify against the full audit index before deleting any rule.
  Unmatched rule: legacy-throttle
```

## SDK usage

All five polyglot SDKs expose the endpoint as a thin wrapper. No client-side
aggregation — one HTTP request, one parsed report.

### Rust

```rust
use acteon_client::{ActeonClient, CoverageQuery};
use chrono::{Duration, Utc};

let client = ActeonClient::new("http://localhost:8080");
let query = CoverageQuery {
    namespace: Some("prod".into()),
    from: Some(Utc::now() - Duration::hours(24)),
    ..Default::default()
};
let report = client.rules_coverage(&query).await?;
println!("uncovered combinations: {}", report.uncovered);
```

### Python

```python
from acteon_client import ActeonClient, CoverageQuery
from datetime import datetime, timezone, timedelta

client = ActeonClient("http://localhost:8080")
query = CoverageQuery(
    namespace="prod",
    from_time=(datetime.now(timezone.utc) - timedelta(hours=24)).isoformat(),
)
report = client.rules_coverage(query)
print(f"uncovered: {report.uncovered}")
```

### Node.js

```typescript
import { ActeonClient } from "@acteon/client";

const client = new ActeonClient("http://localhost:8080");
const report = await client.rulesCoverage({
  namespace: "prod",
  from: new Date(Date.now() - 24 * 3600 * 1000).toISOString(),
});
console.log(`uncovered: ${report.uncovered}`);
```

### Go

```go
import (
    "context"
    "time"
    "github.com/acteon/acteon/clients/go/acteon"
)

client := acteon.NewClient("http://localhost:8080")
from := time.Now().Add(-24 * time.Hour)
report, err := client.RulesCoverage(context.Background(), &acteon.CoverageQuery{
    Namespace: "prod",
    From:      &from,
})
```

### Java

```java
ActeonClient client = new ActeonClient("http://localhost:8080");
CoverageQuery query = new CoverageQuery();
query.setNamespace("prod");
query.setFrom(Instant.now().minus(Duration.ofHours(24)).toString());
CoverageReport report = client.rulesCoverage(query);
```

## Backend behavior

Rule coverage aggregation runs server-side in the audit backend:

| Backend | Aggregation strategy | Notes |
|---------|----------------------|-------|
| **Postgres** | Native SQL `GROUP BY` | Uses the `idx_audit_coverage` covering index added in the rule-coverage migration. Single index-seek query, O(1) memory on the server. |
| **ClickHouse** | Native SQL `GROUP BY` | Same pattern as Postgres but using ClickHouse's `count()` and `toUnixTimestamp64Milli` for millisecond timestamps. |
| **Memory** | Paged in-memory fallback via `InMemoryAnalytics` | Streams audit records in 1000-record batches and accumulates into a hash map keyed by dimension tuple. Bounded server-side; non-SQL backends share this code path. |
| **Elasticsearch** | Paged in-memory fallback | Same as Memory. |
| **DynamoDB** | Paged in-memory fallback | Same as Memory. |

For high-volume deployments on non-SQL audit backends, the paged fallback
scales linearly with the number of audit records in the window rather than
with dimension cardinality. This is intentional: it keeps non-SQL backends
functional without forcing each to implement native aggregation, but it is a
scaling bottleneck for very large scan windows. Planned work on cursor-based
audit pagination will address this — see the
[roadmap](../reference/roadmap.md).

## The window-scoped caveat

The `unmatched_rules` list shows enabled rules that did not match any action
**inside the scanned window**. This is not the same as "the rule is dead":

- A rule that fires once a week will appear unmatched in a 24-hour scan.
- A rule gated to a specific tenant will appear unmatched when the scan is
  scoped to a different tenant.
- A rule behind a maintenance-window condition will appear unmatched outside
  the maintenance window.

**Always widen the scan window** before deciding a rule is dead. The CLI and
docs emphasize this for exactly this reason: don't delete a safety rule
based on a single-hour snapshot.

## Related features

- [Audit Trail](audit-trail.md) — the underlying data source
- [Rule Playground](rule-playground.md) — dry-run a single action against the rules
- [Analytics](analytics.md) — time-bucketed volume and outcome metrics
