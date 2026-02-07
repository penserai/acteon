# Time-Based Rules

Rules can include temporal conditions that match on the current time at dispatch. This enables patterns like business-hours suppression, weekend rerouting, and maintenance-window controls without external schedulers.

## How It Works

The rule engine exposes a `time` identifier containing the current timestamp broken into components. You can use `time.*` fields in conditions just like `action.*` fields — no special syntax or configuration required. By default, `time.*` fields use UTC, but you can configure a timezone at the gateway or per-rule level.

## Available Fields

| Field | Type | Range | Description |
|-------|------|-------|-------------|
| `time.hour` | int | 0–23 | Hour of the day |
| `time.minute` | int | 0–59 | Minute of the hour |
| `time.second` | int | 0–59 | Second of the minute |
| `time.day` | int | 1–31 | Day of the month |
| `time.month` | int | 1–12 | Month of the year |
| `time.year` | int | — | Four-digit year |
| `time.weekday` | string | — | English name (`"Monday"` … `"Sunday"`) |
| `time.weekday_num` | int | 1–7 | ISO weekday (1=Monday … 7=Sunday) |
| `time.timestamp` | int | — | Unix timestamp in seconds |

!!! note
    By default, all temporal values (except `timestamp`) use **UTC**. Use the `timezone` field on a rule or the `default_timezone` gateway setting to evaluate in a specific timezone. The `timestamp` field always returns the UTC unix timestamp regardless of timezone.

## Timezone Support

Rules can specify an IANA timezone (e.g. `"US/Eastern"`, `"Europe/Berlin"`) so that `time.*` fields are evaluated in local time. This avoids manual UTC offset calculations and correctly handles DST transitions.

**Resolution order** (most specific wins):

1. Per-rule `timezone` field
2. Gateway-level `default_timezone` (set in `acteon.toml` under `[rules]`)
3. UTC (implicit default)

### Per-Rule Timezone

```yaml title="rules/eastern_business_hours.yaml"
rules:
  - name: business-hours-eastern
    timezone: "US/Eastern"
    condition:
      all:
        - field: time.hour
          gte: 9
        - field: time.hour
          lt: 17
        - field: time.weekday_num
          lte: 5
    action:
      type: allow
```

### Gateway Default Timezone

Set in `acteon.toml`:

```toml
[rules]
directory = "rules/"
default_timezone = "US/Eastern"
```

All rules without an explicit `timezone` field will use `US/Eastern` for `time.*` fields.

### CEL with Timezone

```yaml title="rules/eastern_hours.cel"
rules:
  - name: business-hours-eastern
    timezone: "US/Eastern"
    condition: 'time.hour >= 9 && time.hour < 17 && time.weekday_num <= 5'
    action:
      type: allow
```

## YAML Examples

### Suppress Outside Business Hours

```yaml title="rules/business_hours.yaml"
rules:
  - name: suppress-outside-hours
    priority: 1
    description: "Suppress non-critical notifications outside 9-17 UTC"
    condition:
      any:
        - field: time.hour
          lt: 9
        - field: time.hour
          gte: 17
    action:
      type: suppress
```

### Suppress on Weekends

```yaml title="rules/weekends.yaml"
rules:
  - name: suppress-weekends
    priority: 2
    description: "Suppress notifications on Saturday and Sunday"
    condition:
      field: time.weekday_num
      gt: 5
    action:
      type: suppress
```

### Reroute to On-Call During Off-Hours

```yaml title="rules/oncall_reroute.yaml"
rules:
  - name: reroute-off-hours-to-oncall
    priority: 3
    description: "Reroute alerts to PagerDuty outside business hours"
    condition:
      all:
        - field: action.action_type
          eq: "alert"
        - any:
            - field: time.hour
              lt: 9
            - field: time.hour
              gte: 17
    action:
      type: reroute
      target_provider: pagerduty
```

### Combined: Business Hours + Weekdays Only

```yaml title="rules/weekday_business_hours.yaml"
rules:
  - name: business-hours-only
    priority: 1
    description: "Suppress outside Mon-Fri 9-17 UTC"
    condition:
      any:
        - field: time.weekday_num
          gt: 5
        - field: time.hour
          lt: 9
        - field: time.hour
          gte: 17
    action:
      type: suppress
```

### Weekday Name Matching

```yaml title="rules/weekday_name.yaml"
rules:
  - name: suppress-saturday
    priority: 1
    condition:
      field: time.weekday
      eq: "Saturday"
    action:
      type: suppress
```

## CEL Examples

The same conditions work in CEL rule files with natural expression syntax:

```yaml title="rules/business_hours.cel"
rules:
  - name: suppress-outside-hours
    priority: 1
    condition: 'time.hour < 9 || time.hour >= 17'
    action:
      type: suppress

  - name: business-hours-weekdays
    priority: 2
    condition: 'time.hour >= 9 && time.hour < 17 && time.weekday_num <= 5'
    action:
      type: allow

  - name: reroute-night-alerts
    priority: 3
    condition: 'action.action_type == "alert" && (time.hour < 6 || time.hour >= 22)'
    action:
      type: reroute
      target_provider: pagerduty

  - name: suppress-weekends
    priority: 4
    condition: 'time.weekday == "Saturday" || time.weekday == "Sunday"'
    action:
      type: suppress
```

## Combining with Action Fields

Time conditions compose freely with action-based conditions:

```yaml
rules:
  - name: throttle-night-email
    priority: 5
    description: "Throttle email during off-hours to avoid inbox flooding"
    condition:
      all:
        - field: action.action_type
          eq: "send_email"
        - any:
            - field: time.hour
              lt: 8
            - field: time.hour
              gte: 20
    action:
      type: throttle
      max_count: 10
      window_seconds: 3600
```

## Testing with Dry-Run

Use [dry-run mode](dry-run.md) to verify time-based rules behave as expected:

```bash
curl -s -X POST "http://localhost:8080/v1/dispatch?dry_run=true" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "notifications",
    "tenant": "tenant-1",
    "provider": "email",
    "action_type": "send_email",
    "payload": {"to": "user@example.com"}
  }' | jq .
```

The response shows which rule matched and what the verdict would be at the current time.

## Simulation

```bash
cargo run -p acteon-simulation --example time_based_simulation
```

## See Also

- [Rule System](../concepts/rules.md) — full rule system overview
- [YAML Rule Reference](../api/rule-reference.md) — complete syntax reference
- [Dry-Run Mode](dry-run.md) — testing rules without executing
