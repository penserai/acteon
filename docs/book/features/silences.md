# Silences

Silences are **time-bounded label-pattern mutes** that suppress dispatched
actions during a maintenance window or incident response. They are the
Acteon equivalent of Alertmanager silences and are the primary tool for
on-call operators who need to temporarily stop paging without modifying
the rule set.

Silences are evaluated **after** rule evaluation but **before** provider
dispatch, so the audit trail captures which rule verdict *would* have
applied. Operators get full forensic context when debugging silenced
alerts, and silences do not interfere with rule coverage analysis.

## When to use silences

- **Maintenance windows** — mute alerts for a service you're about to
  restart, deploy, or take offline.
- **Known upstream incidents** — suppress the downstream cascade while
  the root cause is being fixed.
- **Noisy alerts under active investigation** — stop paging while an
  engineer is already looking at the issue.
- **Incident response** — reduce notification storms during a live
  incident.

Silences are explicitly **not** a substitute for rules. A rule that
blocks actions is a policy; a silence is a temporary operator override.
Always expire silences as soon as the reason for them is resolved —
Acteon does this automatically at `ends_at`.

## The silence model

A silence has five parts:

| Field | Meaning |
|---|---|
| `matchers` | One or more label matchers, combined with **AND** — all must match |
| `starts_at` | When the silence becomes active (defaults to now) |
| `ends_at` | When the silence expires |
| `comment` | Human-readable reason (required — document why) |
| `created_by` | Caller identity, captured from the API key used |

### Matchers

Each matcher targets a single label on the dispatched action's
`metadata.labels` map:

| Operator | CLI form | Meaning |
|---|---|---|
| `equal` | `name=value` | Label value exactly equals the given value |
| `not_equal` | `name!=value` | Label is missing or has a different value |
| `regex` | `name=~pattern` | Label value matches the (anchored) regex |
| `not_regex` | `name!~pattern` | Label is missing or does not match the regex |

Regex patterns are **anchored at both ends** — a pattern `warn.*`
matches `warning` but not `prewarning`. To match a substring, use
`.*warn.*` explicitly.

### Regex complexity cap

Regex matchers are capped to prevent ReDoS:

- **Maximum pattern length**: 256 characters
- **Maximum compiled DFA size**: 64 KB
- Backtracking features (lookaround, backreferences) are not supported
  because Acteon uses the `regex` crate

Patterns exceeding these limits are rejected with `400 Bad Request` at
silence creation time. You can never create a silence that could stall
the dispatch path.

### Tenant scoping and hierarchy

Silences are created under a specific `(namespace, tenant)` and are
enforced against the caller's [API key grants](api-key-scoping.md).
A caller scoped to tenant `acme` can create silences in `acme` *and* in
any hierarchical child like `acme.us-east` or `acme.us-east.prod`. A
caller scoped to `acme.us-east` cannot create silences in `acme` or
`acme.eu-west`.

The permission required is `SilencesManage`, held by `admin` and
`operator` roles but not by `viewer`. This is deliberately separate
from `RulesManage` — on-call operators need to silence alerts without
being able to modify the rule set.

## API endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/v1/silences` | Create a silence |
| `GET` | `/v1/silences` | List silences (tenant filter auto-injected for scoped callers) |
| `GET` | `/v1/silences/{id}` | Get a silence by ID |
| `PUT` | `/v1/silences/{id}` | Extend `ends_at` or edit comment (matchers are immutable) |
| `DELETE` | `/v1/silences/{id}` | Expire a silence immediately |

### Create request shape

```json
{
  "namespace": "prod",
  "tenant": "acme",
  "matchers": [
    { "name": "severity", "value": "warning", "op": "equal" },
    { "name": "service", "value": "api|worker", "op": "regex" }
  ],
  "duration_seconds": 7200,
  "comment": "deploying api v2.3.0 to acme.us-east"
}
```

You can supply either `duration_seconds` (convenience) or an explicit
`ends_at` RFC 3339 timestamp. `starts_at` defaults to now. At least one
matcher is required — an empty matcher list is rejected.

### List query parameters

| Parameter | Default | Description |
|---|---|---|
| `namespace` | (none) | Filter by namespace |
| `tenant` | (auto-injected for single-tenant callers) | Filter by tenant |
| `include_expired` | `false` | Include silences whose `ends_at` is in the past |

## CLI

```bash
# Create a 2-hour silence
acteon silences create \
  --namespace prod --tenant acme \
  --matcher severity=warning \
  --matcher team=platform \
  --hours 2 \
  --comment "canary deploy"

# Create with an explicit end time
acteon silences create \
  --namespace prod --tenant acme \
  --matcher alertname=HighLatency \
  --ends-at 2026-04-11T00:00:00Z \
  --comment "investigating upstream"

# List active silences
acteon silences list --namespace prod

# Include expired silences
acteon silences list --include-expired

# Get a specific silence
acteon silences get <id>

# Extend a silence
acteon silences update <id> --ends-at 2026-04-11T06:00:00Z --comment "extended"

# Expire immediately
acteon silences expire <id>
```

### CLI matcher syntax

The CLI accepts matchers in a compact `key<op>value` form:

| CLI form | Operator |
|---|---|
| `severity=warning` | Equal |
| `severity!=info` | NotEqual |
| `severity=~warn.*` | Regex |
| `severity!~debug|trace` | NotRegex |

The longer operators (`!~`, `=~`, `!=`) are matched before `=` to avoid
ambiguity. Use single quotes around regex patterns to protect them from
shell expansion.

### Text output

```text
Silences count=3
  state=ACTIVE  id=01932c4a-...  namespace=prod  tenant=acme  ends_at=2026-04-11T00:00:00+00:00  matchers=severity="warning", team="platform"  comment="canary deploy"
  state=ACTIVE  id=01932c4b-...  namespace=prod  tenant=acme  ends_at=2026-04-10T12:00:00+00:00  matchers=alertname="HighLatency"  comment="investigating upstream"
  state=EXPIRED id=01932c49-...  namespace=prod  tenant=acme  ends_at=2026-04-10T06:00:00+00:00  matchers=severity="critical"  comment="db migration"
```

## What happens when a silence matches

When a dispatched action matches an active silence, Acteon:

1. **Skips the verdict's provider execution** — no email, no page, no
   webhook call
2. **Still evaluates all rules** — the rule verdict is computed and
   recorded in the audit trail
3. **Emits `ActionOutcome::Silenced { silence_id, matched_rule }`** —
   which shows up in the audit trail and `query_audit` results
4. **Publishes a stream event** — so subscribers see the silenced
   dispatch in real time with outcome category `silenced`

The original action stays in the audit trail with its full payload,
matched rule, and silence ID. You can query it later with:

```bash
acteon audit list --outcome silenced --from 2026-04-10T00:00:00Z
```

## Interaction with other features

| Feature | How silences interact |
|---|---|
| [Rules](../concepts/rules.md) | Rules still evaluate; silences just prevent delivery |
| [Rule Coverage](rule-coverage.md) | Silenced actions count toward coverage (the rule still matched) |
| [Audit Trail](audit-trail.md) | Silenced dispatches are fully recorded with the matched silence ID |
| [Event Streaming](event-streaming.md) | Silenced dispatches emit a stream event with outcome `silenced` |
| [API Key Scoping](api-key-scoping.md) | `SilencesManage` permission + tenant/namespace grants enforce who can mute what |
| [Analytics](analytics.md) | Silenced dispatches are visible in outcome breakdowns |

## Design notes

- **Expiry is eager at dispatch time** — silences whose `ends_at` has
  passed do not match new dispatches, even if the in-memory cache still
  holds them. This makes the system self-correcting.
- **DELETE is a soft-expire, not a hard delete** — `DELETE /v1/silences/{id}`
  sets `ends_at = now` and persists the updated record. The silence
  record itself is preserved so that audit trail references to its
  `silence_id` (recorded on previously silenced dispatches) remain
  resolvable via `GET /v1/silences/{id}` and
  `GET /v1/silences?include_expired=true`. A background reaper (Phase
  1.5) will eventually purge tombstoned records.
- **Empty matcher lists are rejected** — guards against accidentally
  muting everything with a bare silence.
- **Hierarchical matching is one-way** — a silence on `acme` covers
  `acme.us-east` but NOT vice versa. Sibling tenants (`acme.us-east`
  vs. `acme.eu-west`) do not cover each other. Dot-strict: `acme`
  does NOT match `acme-corp`.
- **Matchers are immutable on update** — to change matchers, expire the
  silence and create a new one. This prevents race conditions where an
  active silence changes shape mid-window.

## HA / distributed deployments

Silences are eventually consistent across gateway instances. When an
operator creates a silence via one instance, the change is visible on
that instance immediately and propagates to peer instances via a
background sync task that rebuilds the cache from the state store.

| Setting | Default | Description |
|---|---|---|
| `background.enable_silence_sync` | `true` | Enable periodic silence cache refresh from the state store |
| `background.silence_sync_interval_seconds` | `10` | How often to refresh |

For production HA deployments, leave `enable_silence_sync` enabled.
The 10-second sync interval is the upper bound on how long a silence
created on instance A will take to start muting dispatches on instance
B. Disabling sync is only appropriate for single-instance deployments.

See `docs/design-alertmanager-parity.md` for the Alertmanager feature
parity initiative this silence implementation is part of.
