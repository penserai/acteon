use serde::{Deserialize, Serialize};

/// Top-level bus configuration surfaced via the server's TOML.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BusConfig {
    /// Whether the bus feature is active. When `false` the server
    /// accepts requests but the endpoints return 503.
    pub enabled: bool,
    /// Kafka-specific settings. Unused when `enabled` is `false`.
    pub kafka: KafkaBusConfig,
}

/// Kafka connection settings. Mirrors the `bootstrap.servers` +
/// security config surface of `librdkafka`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KafkaBusConfig {
    /// Comma-separated `host:port` list. Defaults to `localhost:9092`
    /// for the docker-compose Kafka profile.
    pub bootstrap_servers: String,
    /// Client-ID prefix. Useful for distinguishing Acteon servers in
    /// broker-side metrics.
    pub client_id: String,
    /// Milliseconds to wait for a produce to be acknowledged before
    /// returning `BusError::Timeout`.
    pub produce_timeout_ms: u64,
    /// Additional rdkafka properties pass-through (e.g.
    /// `security.protocol`, `sasl.mechanism`, `ssl.ca.location`).
    #[serde(default)]
    pub extra: Vec<(String, String)>,
}

impl Default for KafkaBusConfig {
    fn default() -> Self {
        Self {
            bootstrap_servers: "localhost:9092".into(),
            client_id: "acteon-bus".into(),
            produce_timeout_ms: 5_000,
            extra: Vec::new(),
        }
    }
}
