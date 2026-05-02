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
