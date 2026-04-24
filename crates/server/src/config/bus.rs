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
    /// Pass-through properties for `librdkafka`.
    pub extra: Vec<(String, String)>,
}

impl Default for KafkaClientConfig {
    fn default() -> Self {
        Self {
            bootstrap_servers: "localhost:9092".into(),
            client_id: "acteon-bus".into(),
            produce_timeout_ms: 5_000,
            extra: Vec::new(),
        }
    }
}
