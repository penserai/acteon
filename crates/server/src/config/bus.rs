//! Configuration for the agentic message bus (Phase 1).
//!
//! Intentionally minimal — only what's needed to open a Kafka
//! connection. Later phases will add schema registry, consumer-group
//! policy, and HITL gate knobs.
use serde::Deserialize;

/// `[bus]` TOML block.
#[derive(Debug, Deserialize, Default, Clone)]
#[serde(default)]
pub struct BusServerConfig {
    /// Enable the bus feature. Must also be compiled with
    /// `--features bus` — the config toggle alone is not enough.
    pub enabled: bool,
    /// Kafka-specific settings. Required when `enabled` is `true`.
    pub kafka: KafkaClientConfig,
    /// Bounded, process-local HTTP consumer sessions.
    pub sessions: BusSessionConfig,
}

/// `[bus.sessions]` limits. Closed sessions count toward capacity until retention expires.
#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct BusSessionConfig {
    pub max_sessions: usize,
    pub max_sessions_per_tenant: usize,
    pub max_in_flight: usize,
    pub max_buffer_bytes: usize,
    pub max_ack_history: usize,
    pub idle_timeout_ms: u64,
    pub lifetime_ms: u64,
    pub closed_retention_ms: u64,
}

impl Default for BusSessionConfig {
    fn default() -> Self {
        Self {
            max_sessions: 512,
            max_sessions_per_tenant: 64,
            max_in_flight: 256,
            max_buffer_bytes: 8 * 1024 * 1024,
            max_ack_history: 1024,
            idle_timeout_ms: 60_000,
            lifetime_ms: 300_000,
            closed_retention_ms: 30_000,
        }
    }
}

impl BusSessionConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_sessions == 0
            || self.max_sessions_per_tenant == 0
            || self.max_sessions_per_tenant > self.max_sessions
            || !(1..=100_000).contains(&self.max_in_flight)
            || self.max_buffer_bytes == 0
            || self.max_ack_history == 0
            || self.idle_timeout_ms == 0
            || self.lifetime_ms == 0
            || self.closed_retention_ms == 0
        {
            return Err("invalid bus session capacity or timeout".into());
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct KafkaClientConfig {
    /// Comma-separated `host:port` bootstrap list.
    pub bootstrap_servers: String,
    /// Client ID advertised to the broker.
    pub client_id: String,
    /// Produce acknowledgement timeout (ms).
    pub produce_timeout_ms: u64,
    /// **Phase 10 add-on**: opt into Kafka transactional produces by
    /// setting a stable `transactional.id`. When set, every bus
    /// produce is wrapped in a Kafka transaction (begin → send →
    /// commit, or abort on error), giving broker-side fencing
    /// across server restarts. Pick one per server instance (e.g.
    /// `acteon-server-1`).
    ///
    /// Cost: each transaction adds two broker round-trips on top of
    /// the produce. Worth it when downstream topics need exactly-
    /// once semantics; over-engineering when consumer-side dedup
    /// (e.g. Phase 6a's `call_id` lookup) already de-duplicates
    /// duplicate produces.
    #[serde(default)]
    pub transactional_id: Option<String>,
    /// Per-transaction timeout (ms). Used only when
    /// `transactional_id` is set; ignored otherwise. Set generously
    /// above `produce_timeout_ms`.
    pub transaction_timeout_ms: u64,
    /// Pass-through properties for `librdkafka`.
    pub extra: Vec<(String, String)>,
}

impl Default for KafkaClientConfig {
    fn default() -> Self {
        Self {
            bootstrap_servers: "localhost:9092".into(),
            client_id: "acteon-bus".into(),
            produce_timeout_ms: 5_000,
            transactional_id: None,
            transaction_timeout_ms: 60_000,
            extra: Vec::new(),
        }
    }
}
