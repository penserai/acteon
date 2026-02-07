# Advanced Patterns

## Multi-Provider with Rerouting and Throttling

This example demonstrates routing actions across multiple providers with rerouting for urgent messages and throttling for rate limiting.

### Rules

```yaml title="rules/multi_provider.yaml"
rules:
  - name: reroute-high-priority
    priority: 1
    description: "Reroute high-priority notifications to SMS"
    condition:
      field: action.payload.priority
      eq: "urgent"
    action:
      type: reroute
      target_provider: "sms"

  - name: throttle-notifications
    priority: 5
    description: "Throttle notifications to 100 per minute"
    condition:
      field: action.action_type
      eq: "send_notification"
    action:
      type: throttle
      max_count: 100
      window_seconds: 60

  - name: modify-add-tracking
    priority: 10
    description: "Add tracking metadata to all email actions"
    condition:
      field: action.action_type
      eq: "send_email"
    action:
      type: modify
      changes:
        tracking_enabled: true
        source: "acteon-gateway"
```

### Code

```rust
let gateway = GatewayBuilder::new()
    .state(Arc::new(MemoryStateStore::new()))
    .lock(Arc::new(MemoryDistributedLock::new()))
    .rules(rules)
    .provider(Arc::new(MockProvider::new("email")))
    .provider(Arc::new(MockProvider::new("sms")))
    .provider(Arc::new(MockProvider::new("webhook")))
    .build()?;

// Urgent → rerouted to SMS
let urgent = Action::new(
    "notifications", "tenant-1", "email", "send_notification",
    json!({"priority": "urgent", "body": "Server down!"}),
);
let outcome = gateway.dispatch(urgent, None).await?;
// → Rerouted from 'email' to 'sms'

// Normal → executed (throttle counts)
let normal = Action::new(
    "notifications", "tenant-1", "email", "send_notification",
    json!({"body": "Weekly digest"}),
);
let outcome = gateway.dispatch(normal, None).await?;
// → Executed

// Email → modified with tracking fields
let email = Action::new(
    "notifications", "tenant-1", "email", "send_email",
    json!({"to": "user@example.com", "subject": "Newsletter"}),
);
let outcome = gateway.dispatch(email, None).await?;
// → Executed (payload now includes tracking_enabled: true)
```

### Running

```bash
cargo run -p acteon-gateway --example multi_provider
```

---

## Alert Lifecycle with State Machines

Model an alert lifecycle from firing through acknowledgment to resolution.

### Configuration

```toml title="acteon.toml"
[[state_machines]]
name = "alert"
initial_state = "firing"
states = ["firing", "acknowledged", "resolved", "stale"]

[[state_machines.transitions]]
from = "firing"
to = "acknowledged"

[[state_machines.transitions]]
from = "acknowledged"
to = "resolved"

[[state_machines.transitions]]
from = "firing"
to = "resolved"

[[state_machines.timeouts]]
state = "firing"
after_seconds = 3600
transition_to = "stale"
```

### Rules

```yaml title="rules/alert-lifecycle.yaml"
rules:
  - name: alert-state-machine
    priority: 5
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

  - name: inhibit-pod-alerts
    priority: 1
    condition:
      all:
        - field: action.action_type
          starts_with: "pod_"
        - call: has_active_event
          args: [cluster_down, action.metadata.cluster]
    action:
      type: suppress
      reason: "Parent cluster is down"
```

### Workflow

```bash
# 1. Alert fires
curl -X POST http://localhost:8080/v1/dispatch \
  -d '{"namespace":"monitoring","tenant":"t1","provider":"pagerduty",
       "action_type":"alert","payload":{},"status":"firing",
       "metadata":{"labels":{"cluster":"prod","service":"api"}}}'
# → StateChanged: null → firing

# 2. Operator acknowledges
curl -X PUT http://localhost:8080/v1/events/FINGERPRINT/transition \
  -d '{"to_state":"acknowledged","namespace":"monitoring","tenant":"t1"}'
# → firing → acknowledged

# 3. Issue resolved
curl -X PUT http://localhost:8080/v1/events/FINGERPRINT/transition \
  -d '{"to_state":"resolved","namespace":"monitoring","tenant":"t1"}'
# → acknowledged → resolved
```

---

## Event Grouping for Alert Batching

Consolidate cluster alerts into periodic summaries.

### Rules

```yaml title="rules/grouping.yaml"
rules:
  - name: group-cluster-alerts
    priority: 5
    condition:
      field: action.action_type
      starts_with: "cluster_"
    action:
      type: group
      group_by:
        - metadata.cluster
        - metadata.severity
      group_wait_seconds: 60
      group_interval_seconds: 300
      max_group_size: 100
```

### Workflow

```bash
# Multiple alerts arrive — all grouped
for i in $(seq 1 5); do
  curl -s -X POST http://localhost:8080/v1/dispatch \
    -d "{\"namespace\":\"monitoring\",\"tenant\":\"t1\",\"provider\":\"slack\",
         \"action_type\":\"cluster_alert\",\"payload\":{\"seq\":$i},
         \"metadata\":{\"labels\":{\"cluster\":\"prod\",\"severity\":\"warning\"}}}"
done

# Check groups
curl -s http://localhost:8080/v1/groups | jq .

# Force flush a group
curl -s -X DELETE http://localhost:8080/v1/groups/GROUP_KEY
```

---

## Production Configuration: Redis State + PostgreSQL Audit

```toml title="acteon.toml"
[server]
host = "0.0.0.0"
port = 8080

[state]
backend = "redis"
url = "redis://redis:6379"
prefix = "acteon"

[audit]
enabled = true
backend = "postgres"
url = "postgres://acteon:acteon@postgres:5432/acteon"
prefix = "acteon_"
ttl_seconds = 2592000
store_payload = true

[audit.redact]
enabled = true
fields = ["password", "token", "api_key", "secret", "credit_card"]
placeholder = "[REDACTED]"

[rules]
directory = "./rules"

[executor]
max_retries = 3
timeout_seconds = 30
max_concurrent = 100

[auth]
enabled = true
config_path = "auth.toml"
watch = true
```

```bash
docker compose --profile postgres up -d
cargo run -p acteon-server --features "postgres" -- -c acteon.toml
```

---

## Webhook Rerouting with Deduplication

This example shows routing critical alerts from Slack to an external webhook endpoint, with deduplication to prevent alert storms.

### Rules

```yaml title="rules/webhook_routing.yaml"
rules:
  - name: reroute-critical-to-webhook
    priority: 1
    description: "Reroute critical alerts to external webhook"
    condition:
      field: action.payload.severity
      eq: "critical"
    action:
      type: reroute
      target_provider: "webhook"

  - name: dedup-webhook-alerts
    priority: 5
    description: "Deduplicate webhook alerts within 5 minutes"
    condition:
      field: action.provider
      eq: "webhook"
    action:
      type: deduplicate
      ttl_seconds: 300
```

### Dispatch

```bash
# Warning alert — stays on Slack
curl -s -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "monitoring",
    "tenant": "acme-corp",
    "provider": "slack",
    "action_type": "alert",
    "payload": {
      "severity": "warning",
      "message": "Response times elevated"
    }
  }' | jq .
# → Executed (via Slack)

# Critical alert — rerouted to webhook
curl -s -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "monitoring",
    "tenant": "acme-corp",
    "provider": "slack",
    "action_type": "alert",
    "payload": {
      "severity": "critical",
      "message": "Database unreachable"
    },
    "dedup_key": "db-unreachable"
  }' | jq .
# → Rerouted (Slack → Webhook)

# Same critical alert — deduplicated
curl -s -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "monitoring",
    "tenant": "acme-corp",
    "provider": "slack",
    "action_type": "alert",
    "payload": {
      "severity": "critical",
      "message": "Database unreachable"
    },
    "dedup_key": "db-unreachable"
  }' | jq .
# → Deduplicated
```

### Simulation

```bash
cargo run -p acteon-simulation --example webhook_simulation
```
