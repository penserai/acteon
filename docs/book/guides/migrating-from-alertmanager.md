# Migrating from Prometheus Alertmanager

Acteon now covers every major Alertmanager primitive, so you can lift
an existing `alertmanager.yml` into Acteon with a single CLI command,
review the diff, and apply. This guide walks through that process,
explains how each Alertmanager concept maps to Acteon, and calls out
the gotchas worth knowing before you cut over.

## Why migrate

If your Alertmanager deployment is doing exactly one job — fan-out
notifications with grouping, silences, and inhibition — Acteon will
feel familiar but heavier. Acteon is most useful when you want
**more than notification routing**:

- **A real audit trail** for every dispatched action with structured
  query support and optional hash-chained tamper-evidence.
- **LLM guardrails** that can deny, modify, or require human approval
  for actions whose payloads match a freeform policy.
- **Task chains** — multi-step workflows that branch on previous-step
  output, run sub-steps in parallel, and cancel cleanly.
- **Multi-provider routing** with circuit breakers, fallback chains,
  and per-tenant quotas.
- **Action signing** so dispatchers can prove an action came from a
  trusted source and replay protection rejects duplicates.

The tradeoff is operational complexity: Acteon is a stateful gateway
with its own state store and audit backend. If you only need silent
fan-out, Alertmanager is simpler. If you need any of the items above,
read on.

## Concept mapping

| Alertmanager                               | Acteon                                                          | Notes                                                                                                  |
|--------------------------------------------|------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------|
| `route` (root)                             | Default rule fall-through                                       | Root receiver becomes the default `reroute` for actions that don't match any explicit rule.            |
| `route.routes` (children)                  | Individual `rules:` entries                                     | Each child becomes one rule with a `condition` and an `action`. Priority follows depth-first order.   |
| `match: { k: v }`                          | `field: action.metadata.k\neq: v`                               | Equality match on action metadata labels.                                                              |
| `match_re: { k: pattern }`                 | `field: action.metadata.k\nmatches: pattern`                    | Regex match. Anchored on Acteon side.                                                                  |
| `group_by`, `group_wait`, `group_interval`, `repeat_interval` | `action: { type: group, ... }`                          | Translated 1:1 by the importer. See [Event Grouping](../features/event-grouping.md).                   |
| `receivers`                                | Entries in `providers.toml`                                     | Each receiver becomes one provider. Slack receivers without an API URL fall back to a `log` provider with a TODO comment. |
| `inhibit_rules`                            | `action: { type: suppress }`                                    | Each inhibit rule becomes a suppress rule against the **target** matchers. Source-side cross-checking is approximated via priority ordering. |
| `silences` (runtime)                       | [Silences](../features/silences.md) — `POST /v1/silences`       | Identical model: label matchers, time window, hierarchical tenant inheritance.                         |
| `time_intervals` / `mute_time_intervals` (top-level + per-route) | [Time intervals](../features/time-intervals.md) — `POST /v1/time-intervals` + per-rule `mute_time_intervals` / `active_time_intervals` | Importer translates Alertmanager's textual weekday and month forms (`monday:friday`, `jan:dec`) into Acteon's numeric ranges. |
| `templates`                                | [Payload templates](../features/payload-templates.md)           | Acteon uses MiniJinja, not Go's `text/template`. **Templates are not auto-translated** — you'll need to port any non-trivial templates by hand. |
| Cluster gossip (HA)                        | State-store sync                                                | Acteon HA uses a shared state backend (Redis, Postgres, DynamoDB) with periodic sync intervals. No gossip protocol.|

## Step-by-step migration

### 1. Run the importer

```bash
acteon import alertmanager \
  --config /etc/alertmanager/alertmanager.yml \
  --output-dir ./acteon-config \
  --default-namespace prod \
  --default-tenant acme
```

The importer prints a one-line summary to stderr and writes:

- `acteon-config/providers.toml` — one entry per Alertmanager receiver.
- `acteon-config/rules.yaml` — one rule per route child + one rule per inhibit rule.
- `acteon-config/time-intervals.yaml` — one entry per top-level `time_intervals:` definition (only when present).

Pass `--dry-run` to print everything to stdout instead of writing
files. That's the recommended first invocation so you can eyeball
the diff before anything touches disk.

### 2. Review the generated providers

Open `providers.toml` and replace every `TODO` placeholder. The
importer copies what it can from your Alertmanager `global:` block
(SMTP host/from, Slack API URL, etc.), but anything secret-shaped
(routing keys, API tokens, webhook URLs) is left as-is from your
config — review and rotate them through your secret manager rather
than committing them.

A few receiver families have caveats:

- **VictorOps + Pushover** are feature-gated in `acteon-server`. The
  generated `providers.toml` includes `# Requires --features ...`
  comments. Build with the matching cargo features or the registry
  will refuse to start.
- **Slack** receivers without a `slack_api_url` are translated to a
  `log` provider so you don't end up with a broken provider after
  import. Either set `slack_api_url` in the Alertmanager `global:`
  block before re-running the import, or change the type to `slack`
  and supply a webhook URL by hand.

### 3. Review the generated rules

Open `rules.yaml`. Each child route becomes one rule named
`imported-<label-hint>-<priority>`. The importer preserves:

- Priority ordering (depth-first, root first).
- `match` → equality conditions on `action.metadata.<label>`.
- `match_re` → anchored regex conditions.
- `group_by` + `group_wait` + `group_interval` + `repeat_interval`
  → `action: { type: group, ... }`.
- Inherited `mute_time_intervals` / `active_time_intervals` from the
  enclosing route.

`inhibit_rules` get one suppress rule each at the bottom of the file.
This is an approximation — Acteon doesn't have Alertmanager's
"inhibit when source matches" cross-correlation. The generated rule
will suppress all actions matching the **target** labels regardless
of whether a source-matching alert is currently firing. Add an
explicit condition (e.g., `action.metadata.severity == "warning"`
plus a check for an existing critical via the audit trail) if you
need true inhibition.

### 4. Review the generated time intervals

If your Alertmanager config has a top-level `time_intervals:` (or
the legacy `mute_time_intervals:`) list, the importer also writes
`time-intervals.yaml`. Each entry is namespaced with
`--default-namespace` and `--default-tenant`. Apply them via:

```bash
for entry in $(yq '.time_intervals[] | @json' time-intervals.yaml); do
  echo "$entry" | curl -X POST $ACTEON_URL/v1/time-intervals \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data @-
done
```

(A bulk-load command is on the roadmap. For now, one POST per entry.)

The importer translates Alertmanager's textual day and month forms
to Acteon's numeric ranges:

| Alertmanager       | Acteon                          |
|--------------------|---------------------------------|
| `monday:friday`    | `{ start: 1, end: 5 }`          |
| `saturday`         | `{ start: 6, end: 6 }`          |
| `jan:dec`          | `{ start: 1, end: 12 }`         |
| `1:31`             | `{ start: 1, end: 31 }`         |
| `-1`               | `{ start: -1, end: -1 }` (last day of month) |

### 5. Apply the providers and rules

Move the files into the locations Acteon reads at startup:

```bash
cp acteon-config/providers.toml /etc/acteon/providers.toml
cp acteon-config/rules.yaml /etc/acteon/rules/imported.yaml
```

Restart the gateway. The startup logs will report the loaded provider
count and rule count. Use `acteon rules list` (or
`GET /v1/rules`) to confirm the rules came through.

### 6. Cut over traffic

Acteon doesn't have an Alertmanager-protocol receiver out of the box.
Two cutover patterns work in practice:

- **Run side-by-side with a webhook receiver.** Add a `webhook`
  receiver to your existing Alertmanager that POSTs to
  `$ACTEON_URL/v1/dispatch`. Acteon will dispatch through your
  new rules; Alertmanager continues to dispatch through its old
  routes in parallel. Compare audit trails for a few days, then
  delete the old receivers from Alertmanager.
- **Drop Alertmanager entirely.** Update your Prometheus
  `alerting:` section to point at Acteon's
  `/v1/dispatch/alertmanager` endpoint. (This endpoint is on the
  roadmap; until then, the side-by-side pattern is the safer cutover.)

## What's still different

A handful of Alertmanager features don't map directly. None of these
are showstoppers, but they're worth knowing before you cut over:

- **Templates.** Alertmanager's Go-template `templates:` directory
  is not auto-translated. Acteon uses MiniJinja for [payload
  templates](../features/payload-templates.md). Functionally
  equivalent for most use cases — variable substitution, conditionals,
  loops — but the syntax is different and you'll need to port each
  template by hand.
- **Cluster gossip.** Alertmanager uses a custom HashiCorp memberlist
  gossip protocol to coordinate silences and notification dedup
  across replicas. Acteon uses a shared state backend (Redis,
  Postgres, DynamoDB) with periodic sync. The default sync interval
  is 10 seconds for silences and 30 seconds for time intervals;
  silences and intervals created on one replica become visible on
  peers within that window.
- **Inhibition correlation.** Alertmanager's inhibit_rules suppress a
  target alert *only when a source alert is currently firing*. The
  importer approximates this with a static suppress rule on the
  target labels — you'll lose the source-cross-check unless you wire
  up an explicit lookup against the [audit trail](../features/audit-trail.md)
  in a custom rule condition.
- **`continue: true` on routes.** Alertmanager allows a child route
  to fall through to its siblings when `continue: true` is set.
  Acteon rules are first-match-wins. The importer ignores
  `continue` today; if your routing tree depends on it, you'll need
  to expand the affected branches into multiple rules by hand.
- **Per-receiver group_by overrides.** Alertmanager lets you set
  `group_by: ['...']` on the receiver itself. Acteon's `group`
  rule action carries `group_by`, so per-route overrides translate
  cleanly, but per-receiver-only overrides do not — fold them into
  the matching route before importing.

## Worked example

The repo ships a complete sample at
`examples/alertmanager-with-time-intervals.yml` that exercises every
primitive the importer supports today. Run:

```bash
cargo run -p acteon-cli -- import alertmanager \
  --config examples/alertmanager-with-time-intervals.yml \
  --default-namespace prod \
  --default-tenant acme \
  --dry-run
```

### Input

```yaml title="alertmanager-with-time-intervals.yml (excerpt)"
route:
  receiver: default-slack
  group_by: [alertname, cluster]
  group_wait: 30s
  group_interval: 5m
  repeat_interval: 4h
  routes:
    - match: { severity: critical }
      receiver: pagerduty-oncall
      mute_time_intervals:
        - planned-maintenance
    - match: { team: database }
      receiver: opsgenie-database
      active_time_intervals:
        - business-hours

receivers:
  - name: pagerduty-oncall
    pagerduty_configs:
      - routing_key: "REPLACE_WITH_PD_ROUTING_KEY"
  - name: opsgenie-database
    opsgenie_configs:
      - api_key: "REPLACE_WITH_OPSGENIE_API_KEY"

time_intervals:
  - name: business-hours
    time_intervals:
      - times: [{ start_time: "09:00", end_time: "17:00" }]
        weekdays: ["monday:friday"]
        location: "America/New_York"
```

### Output

```toml title="providers.toml (excerpt)"
[[providers]]
name = "pagerduty-oncall"
type = "pagerduty"
routing_key = "REPLACE_WITH_PD_ROUTING_KEY"

[[providers]]
name = "opsgenie-database"
type = "opsgenie"
opsgenie.api_key = "REPLACE_WITH_OPSGENIE_API_KEY"
```

```yaml title="rules.yaml (excerpt)"
rules:
  - name: imported-critical-1
    priority: 1
    condition:
      field: action.metadata.severity
      eq: "critical"
    action:
      type: reroute
      target_provider: pagerduty-oncall
    mute_time_intervals:
      - planned-maintenance

  - name: imported-database-2
    priority: 2
    condition:
      field: action.metadata.team
      eq: "database"
    action:
      type: reroute
      target_provider: opsgenie-database
    active_time_intervals:
      - business-hours
```

```yaml title="time-intervals.yaml (excerpt)"
time_intervals:
  - name: business-hours
    namespace: prod
    tenant: acme
    location: "America/New_York"
    time_ranges:
      - times:
          - start: "09:00"
            end: "17:00"
        weekdays:
          - start: 1
            end: 5
```

The full output (5 providers, 6 rules, 2 time intervals) is what
`cargo run -- import alertmanager --dry-run` prints when you point
it at the sample.

## Related reading

- [Time intervals](../features/time-intervals.md) — the recurring
  schedule primitive that mirrors Alertmanager's `time_intervals`.
- [Silences](../features/silences.md) — runtime, time-bounded label
  mutes (the same mental model as Alertmanager silences).
- [Event grouping](../features/event-grouping.md) — `group_wait`,
  `group_interval`, `repeat_interval` semantics.
- [Suppression](../features/suppression.md) — the rule action
  `inhibit_rules` translates into.
- [Audit trail](../features/audit-trail.md) — the structured log
  Acteon adds on top of dispatch.
