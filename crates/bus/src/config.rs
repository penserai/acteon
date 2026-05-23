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
    /// **Phase 10 add-on**: enable transactional produces by setting
    /// a stable `transactional.id`. Required only by operators who
    /// want broker-side fencing across server restarts. When
    /// `Some`, every `produce` is wrapped in a Kafka transaction
    /// (begin → send → commit, or abort on error). When `None`
    /// (default), produces use the idempotent producer alone —
    /// dedup within a session, no cross-restart fencing.
    ///
    /// Stable across restarts: pick one per server instance (e.g.
    /// `acteon-server-1`, `acteon-server-2`). The broker uses this
    /// to track epochs and fence out zombie producers.
    ///
    /// Cost: each transaction adds two broker round-trips (begin +
    /// commit) on top of the produce itself. Worth it when the
    /// downstream topic must be exactly-once even across restarts;
    /// over-engineering when consumer-side dedup is already in
    /// place (e.g. for tool-call envelopes that have `call_id`
    /// dedup at lookup time).
    #[serde(default)]
    pub transactional_id: Option<String>,
    /// Per-transaction timeout in milliseconds. Used when
    /// `transactional_id` is `Some`; ignored otherwise. The broker
    /// fences a transaction that exceeds this timeout, so set it
    /// generously above the slowest `produce_timeout_ms` you
    /// expect.
    pub transaction_timeout_ms: u64,
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
            transactional_id: None,
            transaction_timeout_ms: 60_000,
            extra: Vec::new(),
        }
    }
}
