use std::collections::HashMap;

use serde::Deserialize;

/// Configuration for `OpenTelemetry` distributed tracing.
///
/// When enabled, Acteon exports trace spans via OTLP to a collector (Jaeger,
/// Grafana Tempo, etc.), providing end-to-end visibility through the dispatch
/// pipeline: HTTP ingress, rule evaluation, state operations, provider
/// execution, and audit recording.
///
/// # Example
///
/// ```toml
/// [telemetry]
/// enabled = true
/// endpoint = "http://localhost:4317"
/// service_name = "acteon"
/// sample_ratio = 1.0
/// protocol = "grpc"
/// ```
#[derive(Debug, Deserialize)]
pub struct TelemetryConfig {
    /// Whether `OpenTelemetry` tracing is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// OTLP exporter endpoint.
    #[serde(default = "default_otel_endpoint")]
    pub endpoint: String,
    /// Service name reported in traces.
    #[serde(default = "default_otel_service_name")]
    pub service_name: String,
    /// Sampling ratio (0.0 to 1.0). `1.0` traces every request.
    #[serde(default = "default_otel_sample_ratio")]
    pub sample_ratio: f64,
    /// OTLP transport protocol: `"grpc"` or `"http"`.
    #[serde(default = "default_otel_protocol")]
    pub protocol: String,
    /// Exporter timeout in seconds.
    #[serde(default = "default_otel_timeout")]
    pub timeout_seconds: u64,
    /// Additional resource attributes as `key=value` pairs.
    #[serde(default)]
    pub resource_attributes: HashMap<String, String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_otel_endpoint(),
            service_name: default_otel_service_name(),
            sample_ratio: default_otel_sample_ratio(),
            protocol: default_otel_protocol(),
            timeout_seconds: default_otel_timeout(),
            resource_attributes: HashMap::new(),
        }
    }
}

fn default_otel_endpoint() -> String {
    "http://localhost:4317".to_owned()
}

fn default_otel_service_name() -> String {
    "acteon".to_owned()
}

fn default_otel_sample_ratio() -> f64 {
    1.0
}

fn default_otel_protocol() -> String {
    "grpc".to_owned()
}

fn default_otel_timeout() -> u64 {
    10
}
