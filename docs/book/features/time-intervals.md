# Time Intervals

Time intervals are named, tenant-scoped recurring schedules. Rules
reference them by name through their `mute_time_intervals` and
`active_time_intervals` fields to gate dispatch by wall-clock time.
The model mirrors Prometheus Alertmanager's `time_intervals`, so
configurations imported with `acteon import alertmanager` map 1:1.

## When to use

- **Mute non-business-hours pages.** Rules continue to match, but
  silently no-op outside of working hours.
- **Restrict cron-style operations to specific weekdays or months.**
- **Mute alerts during planned maintenance windows that recur.** For
  one-off windows, prefer [silences](silences.md).
- **Honor an Alertmanager `mute_time_intervals` config** without
  manually translating routes.

## Model

A `TimeInterval` has:

- `name`, `namespace`, `tenant` — identity within the system.
- `time_ranges` — a list of [`TimeRange`](#timerange) entries. The
  interval matches if **any** range matches the current instant.
- `location` — IANA timezone (`America/New_York`, `Europe/Berlin`, …).
  Defaults to UTC.
- `description`, `created_by`, `created_at`, `updated_at`.

### TimeRange

A `TimeRange` is a composite predicate. Each populated list is ANDed
together; an empty list means "any value matches" for that field.

| Field           | Shape                                            | Example          |
|-----------------|--------------------------------------------------|------------------|
| `times`         | `[{ start: "HH:MM", end: "HH:MM" }]`             | `09:00-17:00`    |
| `weekdays`      | `[{ start: 1..7, end: 1..7 }]` (1=Mon..7=Sun)    | `1-5` (Mon-Fri)  |
| `days_of_month` | `[{ start: -31..31, end: -31..31 }]`             | `-1` (last day)  |
| `months`        | `[{ start: 1..12, end: 1..12 }]`                 | `4-4` (April)    |
| `years`         | `[{ start: int, end: int }]`                     | `2026-2026`      |

`days_of_month` accepts negative indices counting from the end of
the month — `{ start: -1, end: -1 }` always selects the last
calendar day, regardless of month length or leap year.

## Creating a time interval

```bash
curl -X POST $ACTEON_URL/v1/time-intervals \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "business-hours",
    "namespace": "prod",
    "tenant": "acme",
    "location": "America/New_York",
    "time_ranges": [
      {
        "times": [{"start": "09:00", "end": "17:00"}],
        "weekdays": [{"start": 1, "end": 5}]
      }
    ]
  }'
```

## Referencing intervals from rules

Add `mute_time_intervals` or `active_time_intervals` to any YAML rule.
Multiple names are allowed; **mute** matches if **any** named interval
is currently active, while **active** mutes the rule unless **at least
one** named interval is currently active.

```yaml title="rules/escalation.yaml"
rules:
  - name: page-oncall
    priority: 1
    description: "Send pages to PagerDuty for critical alerts"
    condition:
      field: action.metadata.severity
      eq: critical
    action:
      type: reroute
      target_provider: pagerduty
    mute_time_intervals:
      - planned-maintenance

  - name: business-hours-emails
    priority: 2
    description: "Email summaries only during business hours"
    condition:
      field: action.action_type
      eq: digest
    action:
      type: allow
    active_time_intervals:
      - business-hours
```

When a referenced interval matches the current time and the rule has
it under `mute_time_intervals`, the dispatch short-circuits to
`ActionOutcome::Muted`. The audit record captures the rule that would
otherwise have applied so operators can trace what happened.

## Missing interval names

If a rule references an interval name that does not exist in the
gateway's cache (for example, a typo), the gate behaves asymmetrically:

- **`mute_time_intervals`** — **fail-open**. A missing name is treated
  as "not currently muting" and the dispatch proceeds. The gateway
  logs `warn!("rule references unknown mute_time_interval")` so the
  typo surfaces in logs.
- **`active_time_intervals`** — **fail-closed**. A missing name cannot
  contribute to "any active interval matches right now," so a rule
  whose only reference is a typo will be silently muted. The gateway
  also emits a `warn!("rule references unknown active_time_interval")`
  for each missing name, so operators can grep for the typo when
  dispatches suddenly stop firing.

Why the asymmetry? A missing mute interval is a safe-to-ignore
configuration drift — a dispatch that should have been muted will
simply proceed. A missing active interval is the opposite: treating
it as "always active" would quietly disable the operator's intent to
restrict the rule to specific windows. Fail-closed preserves the
safety guarantee at the cost of one-typo-mutes-everything, and the
warn log is the escape hatch.

## Hierarchical tenant matching

Like silences, time intervals support hierarchical tenant inheritance.
An interval defined for tenant `acme` covers actions dispatched to
`acme.us-east`, `acme.us-east.prod`, and so on. The `acme` tenant does
**not** cover `acme-corp` (the dot delimiter is required).

## Pipeline position

Time interval gating runs after silences and before provider dispatch:

```
rule evaluation → silence check → time-interval check → execute verdict
```

This means:

- A silence still trumps an active interval (silenced actions don't
  reach the time-interval gate).
- The audit record for a muted action carries `outcome=muted`,
  `interval=<name>`, and `reason=mute_time_interval` (or
  `active_time_interval`).
- The provider is never invoked for muted actions.

## Importing from Alertmanager

`acteon import alertmanager --config alertmanager.yml --output-dir ./out`
parses both legacy `mute_time_intervals:` and the modern
`time_intervals:` top-level lists, plus per-route
`mute_time_intervals` / `active_time_intervals` references, and emits:

- `out/time-intervals.yaml` — one entry per interval, ready to apply
  via `POST /v1/time-intervals`.
- Generated rule entries with `mute_time_intervals` /
  `active_time_intervals` populated from the matching route.

Weekday and month names are translated from Alertmanager's textual
form (`monday:friday`, `jan:dec`) to Acteon's numeric ranges. Numeric
forms (`1:5`, `1:12`) and single values (`saturday`, `april`) are
accepted as well.

## Related features

- [Silences](silences.md) — one-off, time-bounded label-pattern mutes.
- [Suppression rules](suppression.md) — block actions unconditionally
  via rule action `type: suppress`.
- [Recurring actions](recurring-actions.md) — schedule actions to
  dispatch on a cron schedule (the inverse use case).
