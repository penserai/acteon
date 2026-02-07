# Type Reference

Complete reference for all public types in the Acteon workspace.

## Core Types (`acteon-core`)

### Action

The primary request type flowing through the gateway.

```rust
pub struct Action {
    pub id: ActionId,
    pub namespace: Namespace,
    pub tenant: TenantId,
    pub provider: ProviderId,
    pub action_type: String,
    pub payload: serde_json::Value,
    pub metadata: ActionMetadata,
    pub dedup_key: Option<String>,
    pub status: Option<String>,
    pub fingerprint: Option<String>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
```

**Builder methods:**

| Method | Description |
|--------|-------------|
| `Action::new(ns, tenant, provider, type, payload)` | Create with required fields |
| `.with_dedup_key(key)` | Set deduplication key |
| `.with_metadata(metadata)` | Set metadata labels |
| `.with_status(status)` | Set state machine state |
| `.with_fingerprint(fp)` | Set event correlation fingerprint |
| `.with_starts_at(ts)` | Set event lifecycle start |
| `.with_ends_at(ts)` | Set event lifecycle end |

### ActionOutcome

Result of action dispatch:

```rust
pub enum ActionOutcome {
    Executed(ProviderResponse),
    Deduplicated,
    Suppressed { rule: String },
    Rerouted { original_provider: String, new_provider: String, response: ProviderResponse },
    Throttled { retry_after: Duration },
    Failed(ActionError),
    Grouped { group_id: String, group_size: usize, notify_at: DateTime<Utc> },
    StateChanged { fingerprint: String, previous_state: String, new_state: String, notify: bool },
    PendingApproval { approval_id: String, expires_at: DateTime<Utc>, approve_url: String, reject_url: String, notification_sent: bool },
    ChainStarted { chain_id: String, chain_name: String, total_steps: usize, first_step: String },
    DryRun { verdict: String, matched_rule: Option<String>, would_be_provider: String },
    CircuitOpen { provider: String, fallback_chain: Vec<String> },
}
```

### ProviderResponse

```rust
pub struct ProviderResponse {
    pub status: ResponseStatus,           // Success | Failure | Partial
    pub body: serde_json::Value,
    pub headers: HashMap<String, String>,
}
```

### ActionMetadata

```rust
pub struct ActionMetadata {
    pub labels: HashMap<String, String>,
}
```

### ActionContext

```rust
pub struct ActionContext {
    pub action: Action,
    pub environment: HashMap<String, String>,
    pub timestamp: DateTime<Utc>,
}
```

### ActionKey

```rust
pub struct ActionKey {
    pub namespace: Namespace,
    pub tenant: TenantId,
    pub action_id: ActionId,
    pub discriminator: Option<String>,
}
```

Canonical form: `namespace:tenant:action_id[:discriminator]`

### Identity Newtypes

| Type | Wraps | Description |
|------|-------|-------------|
| `Namespace` | `String` | Logical namespace |
| `TenantId` | `String` | Tenant identifier |
| `ActionId` | `String` | Action UUID |
| `ProviderId` | `String` | Provider name |

All support: `new()`, `as_str()`, `Deref`, `Display`, `From<String>`, `From<&str>`

---

## State Machine Types

### StateMachineConfig

```rust
pub struct StateMachineConfig {
    pub name: String,
    pub initial_state: String,
    pub states: Vec<String>,
    pub transitions: Vec<TransitionConfig>,
    pub timeouts: Vec<TimeoutConfig>,
}
```

### TransitionConfig

```rust
pub struct TransitionConfig {
    pub from: String,
    pub to: String,
    pub on_transition: TransitionEffects,
}
```

### TimeoutConfig

```rust
pub struct TimeoutConfig {
    pub in_state: String,
    pub after_seconds: u64,
    pub transition_to: String,
}
```

---

## Event Grouping Types

### EventGroup

```rust
pub struct EventGroup {
    pub group_id: String,
    pub group_key: String,
    pub labels: HashMap<String, String>,
    pub events: Vec<GroupedEvent>,
    pub notify_at: DateTime<Utc>,
    pub state: GroupState,              // Pending | Notified | Resolved
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

### GroupedEvent

```rust
pub struct GroupedEvent {
    pub action_id: ActionId,
    pub fingerprint: Option<String>,
    pub status: Option<String>,
    pub payload: serde_json::Value,
    pub received_at: DateTime<Utc>,
}
```

---

## Chain Types

### ChainConfig

```rust
pub struct ChainConfig {
    pub name: String,
    pub steps: Vec<ChainStepConfig>,
    pub on_failure: ChainFailurePolicy,   // Abort | AbortNoDlq
    pub timeout_seconds: Option<u64>,
    pub on_cancel: Option<ChainNotificationTarget>,
}
```

### ChainStepConfig

```rust
pub struct ChainStepConfig {
    pub name: String,
    pub provider: String,
    pub action_type: String,
    pub payload_template: serde_json::Value,
    pub on_failure: Option<StepFailurePolicy>,  // Abort | Skip | Dlq
    pub delay_seconds: Option<u64>,
}
```

---

## Rule Types (`acteon-rules`)

### Rule

```rust
pub struct Rule {
    pub name: String,
    pub priority: i32,
    pub condition: Expr,
    pub action: RuleAction,
}
```

### RuleAction

```rust
pub enum RuleAction {
    Suppress,
    Deduplicate { ttl_seconds: u64 },
    Throttle { max_count: u64, window_seconds: u64, message: Option<String> },
    Reroute { target_provider: String },
    Modify { changes: serde_json::Value },
    Group { group_by: Vec<String>, group_wait_seconds: u64, group_interval_seconds: Option<u64>, max_group_size: Option<usize> },
    StateMachine { state_machine_name: String, fingerprint_fields: Vec<String> },
    RequireApproval { message: String, notification_targets: Vec<...>, ttl_seconds: Option<u64>, auto_approve_conditions: Vec<...> },
    Chain { chain_name: String },
    LlmGuardrail { evaluator_name: String, block_on_flag: Option<bool>, send_to: Option<String> },
}
```

### Expr (Expression IR)

```rust
pub enum Expr {
    Literal(bool),
    FieldEq { field: String, value: String },
    FieldContains { field: String, value: String },
    FieldStartsWith { field: String, prefix: String },
    FieldEndsWith { field: String, suffix: String },
    FieldRegex { field: String, pattern: String },
    FieldGt { field: String, value: f64 },
    FieldGte { field: String, value: f64 },
    FieldLt { field: String, value: f64 },
    FieldLte { field: String, value: f64 },
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Call { name: String, args: Vec<String> },
}
```

---

## Executor Types

### ExecutorConfig

```rust
pub struct ExecutorConfig {
    pub max_retries: u32,          // Default: 3
    pub retry_strategy: RetryStrategy,
    pub execution_timeout: Duration, // Default: 30s
    pub max_concurrent: usize,     // Default: 10
}
```

### RetryStrategy

```rust
pub enum RetryStrategy {
    ExponentialBackoff { initial_delay: Duration, max_delay: Duration },
    Constant { delay: Duration },
    Linear { initial_delay: Duration, increment: Duration },
}
```

---

## Audit Types

### AuditRecord

```rust
pub struct AuditRecord {
    pub id: String,
    pub action_id: String,
    pub chain_id: Option<String>,
    pub namespace: String,
    pub tenant: String,
    pub provider: String,
    pub action_type: String,
    pub verdict: String,
    pub matched_rule: Option<String>,
    pub outcome: String,
    pub action_payload: Option<Value>,
    pub verdict_details: Value,
    pub outcome_details: Value,
    pub metadata: Value,
    pub dispatched_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub expires_at: Option<DateTime<Utc>>,
    pub caller_id: Option<String>,
    pub auth_method: Option<String>,
}
```

### AuditQuery

```rust
pub struct AuditQuery {
    pub namespace: Option<String>,
    pub tenant: Option<String>,
    pub provider: Option<String>,
    pub action_type: Option<String>,
    pub outcome: Option<String>,
    pub verdict: Option<String>,
    pub matched_rule: Option<String>,
    pub caller_id: Option<String>,
    pub chain_id: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<u32>,        // Default: 50, Max: 1000
    pub offset: Option<u32>,
}
```

---

## LLM Types

### LlmGuardrailResponse

```rust
pub struct LlmGuardrailResponse {
    pub allowed: bool,
    pub reasoning: String,
    pub confidence: f32,           // 0.0 to 1.0
}
```

---

## Utility Functions

### `compute_fingerprint`

```rust
pub fn compute_fingerprint(action: &Action, fields: &[String]) -> String
```

Computes a SHA-256 fingerprint from specified action fields. Supports paths: `namespace`, `tenant`, `provider`, `action_type`, `id`, `status`, `metadata.key`, `payload.field.nested`.
