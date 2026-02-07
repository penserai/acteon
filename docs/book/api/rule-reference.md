# YAML Rule Reference

Complete syntax reference for Acteon YAML rule files.

## File Structure

Rule files are YAML documents with a top-level `rules` array:

```yaml title="rules/example.yaml"
rules:
  - name: rule-name
    priority: 10
    description: "Optional description"
    condition: ...
    action: ...
```

## Rule Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Unique rule identifier |
| `priority` | i32 | Yes | Evaluation order (lower = first) |
| `description` | string | No | Human-readable description |
| `condition` | object | Yes | When the rule matches |
| `action` | object | Yes | What happens when it matches |

---

## Conditions

### Single Field Match

```yaml
condition:
  field: action.action_type
  eq: "send_email"
```

### All (AND)

```yaml
condition:
  all:
    - field: action.action_type
      eq: "send_email"
    - field: action.payload.to
      contains: "@"
```

### Any (OR)

```yaml
condition:
  any:
    - field: action.provider
      eq: "email"
    - field: action.provider
      eq: "sms"
```

### Not (Negation)

```yaml
condition:
  not:
    field: action.action_type
    eq: "internal"
```

### Nested Logic

```yaml
condition:
  all:
    - field: action.action_type
      eq: "send_email"
    - any:
        - field: action.payload.priority
          eq: "high"
        - field: action.payload.priority
          eq: "urgent"
    - not:
        field: action.metadata.skip
        eq: "true"
```

### Semantic Match

Match actions based on meaning using vector embeddings:

```yaml
condition:
  semantic_match: "Infrastructure issues, server problems"
  threshold: 0.75
  text_field: action.payload.message
```

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `semantic_match` | string | Yes | — | Topic description to match against |
| `threshold` | float | No | `0.8` | Minimum cosine similarity (0.0 to 1.0) |
| `text_field` | string | No | — | Field path for the text. When omitted, the entire payload is used |

`semantic_match` can be used inside `all` / `any` blocks:

```yaml
condition:
  all:
    - field: action.payload.severity
      eq: "critical"
    - semantic_match: "Infrastructure and server issues"
      threshold: 0.7
      text_field: action.payload.description
```

!!! note
    Requires the `[embedding]` section in `acteon.toml`. See [Semantic Routing](../features/semantic-routing.md) for configuration details.

### Expression Functions

```yaml
condition:
  call: has_active_event
  args: [cluster_down, action.metadata.cluster]
```

---

## Operators

| Operator | Type | Description | Example |
|----------|------|-------------|---------|
| `eq` | string | Exact match | `eq: "send_email"` |
| `contains` | string | Substring | `contains: "@"` |
| `starts_with` | string | Prefix | `starts_with: "cluster_"` |
| `ends_with` | string | Suffix | `ends_with: "@test.com"` |
| `regex` | string | Regular expression | `regex: "^alert_.*$"` |
| `gt` | number | Greater than | `gt: 100` |
| `gte` | number | Greater than or equal | `gte: 50` |
| `lt` | number | Less than | `lt: 10` |
| `lte` | number | Less than or equal | `lte: 0` |

---

## Field Paths

| Path | Description |
|------|-------------|
| `action.namespace` | Namespace string |
| `action.tenant` | Tenant ID string |
| `action.provider` | Provider name |
| `action.action_type` | Action type discriminator |
| `action.id` | Action UUID |
| `action.status` | Current state machine state |
| `action.payload.<path>` | JSON payload field (dot-separated) |
| `action.metadata.<key>` | Metadata label value |

### Payload Path Examples

```yaml
# Top-level field
field: action.payload.to

# Nested field
field: action.payload.user.email

# Array element (by index)
field: action.payload.recipients.0.email
```

### Temporal Fields

The `time` identifier provides the current UTC time at dispatch:

| Path | Type | Description |
|------|------|-------------|
| `time.hour` | int (0–23) | Hour of the day |
| `time.minute` | int (0–59) | Minute |
| `time.second` | int (0–59) | Second |
| `time.day` | int (1–31) | Day of month |
| `time.month` | int (1–12) | Month |
| `time.year` | int | Year |
| `time.weekday` | string | English name (`"Monday"` … `"Sunday"`) |
| `time.weekday_num` | int (1–7) | ISO weekday (1=Mon … 7=Sun) |
| `time.timestamp` | int | Unix seconds |

```yaml
# Suppress outside business hours
condition:
  any:
    - field: time.hour
      lt: 9
    - field: time.hour
      gte: 17

# Suppress on weekends
condition:
  field: time.weekday_num
  gt: 5

# Match specific weekday by name
condition:
  field: time.weekday
  eq: "Saturday"
```

See [Time-Based Rules](../features/time-based-rules.md) for more patterns.

---

## Actions

### Suppress

```yaml
action:
  type: suppress
  reason: "Optional reason"            # Optional
```

### Deduplicate

```yaml
action:
  type: deduplicate
  ttl_seconds: 300                     # Required
```

### Throttle

```yaml
action:
  type: throttle
  max_count: 100                       # Required
  window_seconds: 60                   # Required
  message: "Rate limited"             # Optional
```

### Reroute

```yaml
action:
  type: reroute
  target_provider: "sms"              # Required
```

### Modify

```yaml
action:
  type: modify
  changes:                             # Required (key-value pairs)
    tracking_enabled: true
    source: "gateway"
```

### Group

```yaml
action:
  type: group
  group_by:                            # Required (field list)
    - metadata.cluster
    - metadata.severity
  group_wait_seconds: 60               # Required
  group_interval_seconds: 300          # Optional
  max_group_size: 100                  # Optional
```

### State Machine

```yaml
action:
  type: state_machine
  state_machine: alert                 # Required (references config)
  fingerprint_fields:                  # Required (field list)
    - action_type
    - metadata.cluster
```

### Require Approval

```yaml
action:
  type: require_approval
  message: "Requires approval"         # Required
  ttl_seconds: 3600                    # Optional (default: 3600)
```

### Chain

```yaml
action:
  type: chain
  chain_name: "pipeline-name"         # Required (references config)
```

### LLM Guardrail

```yaml
action:
  type: llm_guardrail
  evaluator_name: "content-safety"    # Required
  block_on_flag: true                  # Optional
  send_to: "review-queue"            # Optional
```

---

## Expression Functions

| Function | Arguments | Description |
|----------|-----------|-------------|
| `has_active_event` | `(event_type, label_value)` | Check if active event exists |
| `get_event_state` | `(fingerprint)` | Get current event state |
| `event_in_state` | `(fingerprint, state)` | Check event is in specific state |

```yaml
# Inhibit pod alerts when cluster is down
condition:
  all:
    - field: action.action_type
      starts_with: "pod_"
    - call: has_active_event
      args: [cluster_down, action.metadata.cluster]
```

---

## Complete Example

```yaml title="rules/complete-example.yaml"
rules:
  # Block spam (highest priority)
  - name: block-spam
    priority: 1
    description: "Block spam actions"
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress

  # Inhibit dependent alerts
  - name: inhibit-pod-alerts
    priority: 2
    condition:
      all:
        - field: action.action_type
          starts_with: "pod_"
        - call: has_active_event
          args: [cluster_down, action.metadata.cluster]
    action:
      type: suppress
      reason: "Cluster is down"

  # Route urgent to SMS
  - name: reroute-urgent
    priority: 5
    condition:
      field: action.payload.priority
      eq: "urgent"
    action:
      type: reroute
      target_provider: "sms"

  # Add tracking to emails
  - name: add-tracking
    priority: 8
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: modify
      changes:
        tracking_enabled: true

  # Deduplicate emails
  - name: dedup-emails
    priority: 10
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: deduplicate
      ttl_seconds: 300

  # Throttle notifications
  - name: throttle-notifications
    priority: 15
    condition:
      field: action.action_type
      eq: "send_notification"
    action:
      type: throttle
      max_count: 100
      window_seconds: 60

  # Group cluster alerts
  - name: group-alerts
    priority: 20
    condition:
      field: action.action_type
      starts_with: "cluster_"
    action:
      type: group
      group_by:
        - metadata.cluster
        - metadata.severity
      group_wait_seconds: 60

  # Track alert lifecycle
  - name: alert-lifecycle
    priority: 25
    condition:
      field: action.action_type
      eq: "alert"
    action:
      type: state_machine
      state_machine: alert
      fingerprint_fields:
        - action_type
        - metadata.cluster
        - metadata.service
```
