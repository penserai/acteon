# Event Grouping and State Machines

**Status:** Draft
**Author:** Acteon Team
**Created:** 2026-02-03

## Overview

This document proposes adding two complementary features to Acteon:

1. **Event State Machines** — Track events through configurable lifecycle states (e.g., `active → acknowledged → resolved`)
2. **Event Grouping** — Batch related events into a single notification with configurable windows

These features enable Acteon to handle use cases requiring event correlation, deduplication across time windows, and lifecycle tracking — while remaining generic enough for diverse applications beyond alerting.

## Motivation

Currently, Acteon treats each action as an independent, stateless dispatch. This works well for transactional notifications but falls short for scenarios where:

- Multiple related events should be batched into a single notification
- Events have a lifecycle (opened, acknowledged, closed)
- Downstream notifications should be suppressed when a root-cause event is active
- Resolution notifications should be sent when an event clears

### Use Cases

| Use Case | Requirement |
|----------|-------------|
| Infrastructure alerting | Group alerts by service, send resolution when recovered |
| E-commerce order updates | Batch rapid status changes, notify once per order |
| IoT sensor events | Aggregate readings over time windows |
| Support ticket routing | Track ticket state, escalate if unacknowledged |
| Payment processing | Correlate related transaction events |

## Design

### 1. Event Identity and Fingerprinting

Events with the same **fingerprint** are considered instances of the same logical event. The fingerprint is computed from configurable fields.

```rust
pub struct Action {
    // ... existing fields ...

    /// Event status for stateful events. None for stateless actions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<EventStatus>,

    /// Fingerprint for correlating related events.
    /// If not provided, computed from fingerprint_fields rule config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,

    /// When this event instance started (for stateful events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<DateTime<Utc>>,

    /// When this event instance ended (for resolved events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    /// Event is active/firing
    Active,
    /// Event has been acknowledged but not resolved
    Acknowledged,
    /// Event is resolved/cleared
    Resolved,
}
```

### 2. State Machine Definition

State machines are defined in configuration and referenced by rules. Each state machine defines:

- **States**: The possible states an event can be in
- **Transitions**: Valid state changes and optional side effects
- **Timeouts**: Automatic transitions after duration (e.g., auto-resolve)

```yaml
# acteon.toml or state_machines.yaml
[state_machines.default]
initial_state = "active"
states = ["active", "acknowledged", "resolved"]

[[state_machines.default.transitions]]
from = "active"
to = "acknowledged"
on_transition = { notify = false }  # Don't notify on ack

[[state_machines.default.transitions]]
from = "active"
to = "resolved"
on_transition = { notify = true, template = "resolved" }

[[state_machines.default.transitions]]
from = "acknowledged"
to = "resolved"
on_transition = { notify = true, template = "resolved" }

[[state_machines.default.transitions]]
from = "active"
to = "active"
on_transition = { notify = false }  # Suppress repeat active events

[state_machines.default.timeouts]
# Auto-resolve if no update received for 5 minutes
active = { after_seconds = 300, transition_to = "resolved" }
acknowledged = { after_seconds = 3600, transition_to = "resolved" }
```

### 3. Event Grouping

Grouping batches multiple events into a single notification. Configuration:

```yaml
rules:
  - name: group-by-service
    priority: 10
    condition:
      field: action.action_type
      in: ["service_error", "service_warning", "service_critical"]
    action:
      type: group
      config:
        # Fields to group by — events with same values are grouped
        group_by:
          - tenant
          - metadata.service
          - metadata.environment

        # How long to wait for initial batch
        group_wait_seconds: 30

        # How long to wait before sending another notification for same group
        group_interval_seconds: 300

        # Maximum events per group before forcing a send
        max_group_size: 100

        # Template for grouped notification
        template: service_alert_group
```

### 4. Group State Storage

Groups are stored in the state backend with the following structure:

```rust
pub struct EventGroup {
    /// Unique identifier for this group
    pub group_id: String,

    /// Hash of group_by field values
    pub group_key: String,

    /// The group_by field values
    pub group_labels: HashMap<String, String>,

    /// Events in this group
    pub events: Vec<GroupedEvent>,

    /// When the group was created
    pub created_at: DateTime<Utc>,

    /// When the group was last updated
    pub updated_at: DateTime<Utc>,

    /// When the next notification should be sent
    pub notify_at: DateTime<Utc>,

    /// Current group state
    pub state: GroupState,
}

pub struct GroupedEvent {
    pub action_id: Uuid,
    pub fingerprint: String,
    pub status: EventStatus,
    pub payload: serde_json::Value,
    pub received_at: DateTime<Utc>,
}

pub enum GroupState {
    /// Accumulating events, waiting for group_wait
    Pending,
    /// Notification sent, in group_interval cooldown
    Notified { last_sent: DateTime<Utc> },
    /// All events resolved, group can be cleaned up
    Resolved,
}
```

### 5. Grouping Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                        Event Arrives                             │
└─────────────────────────────────────────────────────────────────┘
                               │
                               ▼
                    ┌─────────────────────┐
                    │ Compute fingerprint │
                    │ and group_key       │
                    └─────────────────────┘
                               │
                               ▼
                    ┌─────────────────────┐
              ┌─────│ Group exists?       │─────┐
              │     └─────────────────────┘     │
              │ No                              │ Yes
              ▼                                 ▼
    ┌──────────────────┐              ┌──────────────────┐
    │ Create new group │              │ Add to existing  │
    │ Set notify_at =  │              │ group            │
    │ now + group_wait │              └──────────────────┘
    └──────────────────┘                       │
              │                                │
              ▼                                ▼
    ┌──────────────────┐              ┌──────────────────┐
    │ Store group      │              │ Check notify_at  │
    │ Return: Grouped  │              └──────────────────┘
    └──────────────────┘                       │
                                    ┌──────────┴──────────┐
                                    │                     │
                              Past due             Not yet due
                                    │                     │
                                    ▼                     ▼
                          ┌──────────────────┐  ┌──────────────────┐
                          │ Send grouped     │  │ Update group     │
                          │ notification     │  │ Return: Grouped  │
                          │ Set notify_at =  │  └──────────────────┘
                          │ now + interval   │
                          └──────────────────┘
```

### 6. Inhibition Rules

Inhibition suppresses events when a related "source" event is active.

```yaml
rules:
  - name: inhibit-on-cluster-down
    priority: 5  # Higher priority than grouping
    condition:
      field: action.metadata.severity
      in: ["warning", "info"]
    action:
      type: inhibit
      config:
        # Source event that triggers inhibition
        source_match:
          field: action.action_type
          eq: cluster_down

        # Fields that must match between source and target
        equal_fields:
          - tenant
          - metadata.cluster

        # Only inhibit if source is in these states
        source_states: ["active", "acknowledged"]
```

### 7. API Changes

#### New Endpoints

```
# Event state management
PUT  /v1/events/{fingerprint}/acknowledge
PUT  /v1/events/{fingerprint}/resolve
GET  /v1/events/{fingerprint}
GET  /v1/events?status=active&tenant=X

# Group management
GET  /v1/groups
GET  /v1/groups/{group_id}
DELETE /v1/groups/{group_id}  # Force resolve

# Silences (future)
POST   /v1/silences
GET    /v1/silences
DELETE /v1/silences/{id}
```

#### Dispatch Response Changes

```rust
pub enum ActionOutcome {
    // ... existing variants ...

    /// Event was added to a group; notification pending
    Grouped {
        group_id: String,
        group_size: usize,
        notify_at: DateTime<Utc>,
    },

    /// Event was inhibited by an active source event
    Inhibited {
        source_fingerprint: String,
        source_action_type: String,
    },

    /// Event state was updated (e.g., active → resolved)
    StateChanged {
        fingerprint: String,
        previous_state: EventStatus,
        new_state: EventStatus,
    },
}
```

### 8. Background Processing

A background task handles:

1. **Group flushing**: Send notifications for groups past their `notify_at`
2. **Timeout processing**: Auto-transition events past their timeout
3. **Cleanup**: Remove resolved groups after retention period

```rust
pub struct GroupProcessor {
    state: Arc<dyn StateStore>,
    providers: Arc<ProviderRegistry>,
    flush_interval: Duration,
}

impl GroupProcessor {
    pub async fn run(&self, mut shutdown: broadcast::Receiver<()>) {
        let mut interval = tokio::time::interval(self.flush_interval);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.flush_pending_groups().await;
                    self.process_timeouts().await;
                    self.cleanup_resolved_groups().await;
                }
                _ = shutdown.recv() => break,
            }
        }
    }
}
```

---

## Implementation Plan

### Phase 1: Foundation
- [ ] Add `status`, `fingerprint`, `starts_at`, `ends_at` to Action
- [ ] Add `Grouped`, `Inhibited`, `StateChanged` outcome variants
- [ ] Implement fingerprint computation

### Phase 2: State Machines
- [ ] State machine configuration parser
- [ ] State transition logic in gateway
- [ ] Timeout background processor
- [ ] State query API (`GET /v1/events`)

### Phase 3: Grouping
- [ ] Group storage schema
- [ ] `type: group` rule action
- [ ] Group flush background processor
- [ ] Group query API (`GET /v1/groups`)

### Phase 4: Inhibition
- [ ] `type: inhibit` rule action
- [ ] Active event index for fast lookup
- [ ] Inhibition matching logic

### Phase 5: Templates & Notifications
- [ ] Template configuration format
- [ ] Template rendering engine
- [ ] Grouped notification formatting

### Phase 6: Silences (Future)
- [ ] Silence storage and API
- [ ] Silence matching in dispatch pipeline
- [ ] Silence expiration processor

---

## Open Questions

1. **Storage overhead**: How to efficiently store/query groups with many events?
2. **Cluster coordination**: How do multiple Acteon instances coordinate group flushing?
3. **Template engine**: Use existing engine (Tera, Handlebars) or custom DSL?
4. **Backwards compatibility**: How to handle dispatch to endpoints expecting instant delivery?

---

## References

- [PagerDuty Event Orchestration](https://support.pagerduty.com/docs/event-orchestration)
- [OpsGenie Alert Policies](https://support.atlassian.com/opsgenie/docs/what-are-alert-policies/)
