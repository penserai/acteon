# State Backends

State backends provide distributed state management for Acteon. They handle distributed locking, deduplication checks, counters, event state tracking, group management, and chain coordination.

## StateStore Trait

All state backends implement the `StateStore` trait:

```rust
#[async_trait]
pub trait StateStore: Send + Sync + 'static {
    async fn check_and_set(&self, key: &str, value: &str, ttl: Duration) -> Result<bool>;
    async fn get(&self, key: &str) -> Result<Option<String>>;
    async fn set(&self, key: &str, value: &str, ttl: Duration) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<bool>;
    async fn increment(&self, key: &str, delta: i64, ttl: Duration) -> Result<i64>;
    async fn compare_and_swap(&self, key: &str, expected: u64, value: &str, ttl: Duration)
        -> Result<CasResult>;
    async fn scan_keys(&self, namespace: &str, tenant: &str, kind: &str, prefix: &str)
        -> Result<Vec<(String, String)>>;
    async fn scan_keys_by_kind(&self, kind: &str) -> Result<Vec<(String, String)>>;
    async fn index_timeout(&self, key: &str, expires_at_ms: i64) -> Result<()>;
    async fn remove_timeout_index(&self, key: &str) -> Result<()>;
    async fn get_expired_timeouts(&self, now_ms: i64) -> Result<Vec<String>>;
    async fn index_chain_ready(&self, key: &str, ready_at_ms: i64) -> Result<()>;
    async fn remove_chain_ready_index(&self, key: &str) -> Result<()>;
    async fn get_ready_chains(&self, now_ms: i64) -> Result<Vec<String>>;
}
```

## DistributedLock Trait

```rust
#[async_trait]
pub trait DistributedLock: Send + Sync + 'static {
    async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<Option<Box<dyn LockGuard>>>;
    async fn acquire(&self, name: &str, ttl: Duration, timeout: Duration)
        -> Result<Box<dyn LockGuard>>;
}

#[async_trait]
pub trait LockGuard: Send + Sync {
    async fn extend(&self, duration: Duration) -> Result<()>;
    async fn release(self: Box<Self>) -> Result<()>;
    async fn is_held(&self) -> Result<bool>;
}
```

## Key Operations

| Operation | Purpose | Used By |
|-----------|---------|---------|
| `check_and_set` | Atomic set-if-not-exists | Deduplication |
| `get` / `set` | Key-value read/write | Event state, groups |
| `increment` | Atomic counter | Throttling |
| `compare_and_swap` | Optimistic concurrency | State transitions |
| `scan_keys` | Key iteration | Event queries |
| `index_timeout` | Schedule timeout | State machine timeouts |
| `index_chain_ready` | Schedule chain step | Chain advancement |

## Lock Consistency Guarantees

| Backend | Mutual Exclusion | Failover Safety | Recommendation |
|---------|-----------------|-----------------|----------------|
| Memory | Perfect | N/A (single process) | Development only |
| Redis (single) | Strong | N/A | Good for most cases |
| Redis (Sentinel) | Strong | Lock may be lost | Accept rare duplicates |
| PostgreSQL | ACID | Locks survive failover | Strong consistency |
| DynamoDB | Strong | Conditional writes | Strong consistency |
| ClickHouse | None | N/A | Not for locking |

!!! warning "ClickHouse"
    ClickHouse does not provide distributed locking. It uses eventual consistency with `ReplacingMergeTree`. Under concurrent load, 10-20% of duplicate actions may be processed. Do not use ClickHouse if you require strict deduplication.
