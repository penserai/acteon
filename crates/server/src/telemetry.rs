//! OpenTelemetry distributed tracing initialization and shutdown.
//!
//! When enabled, this module sets up a [`tracing_subscriber`] registry that
//! combines the standard `fmt` layer with an OpenTelemetry layer backed by
//! an OTLP exporter. This bridges the existing `tracing` instrumentation
//! (spans and events) directly into an OpenTelemetry-compatible collector.

use std::time::Duration;

use opentelemetry::trace::TracerProvider;
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{BatchSpanProcessor, Sampler, SdkTracerProvider};
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::TelemetryConfig;

/// Opaque handle returned by [`init`]. Dropping it is a no-op; call
/// [`TelemetryGuard::shutdown`] for a clean flush of pending spans.
pub struct TelemetryGuard {
    provider: Option<SdkTracerProvider>,
}

impl TelemetryGuard {
    /// Flush pending spans and shut down the exporter.
    ///
    /// This should be called during server shutdown to avoid losing
    /// in-flight trace data.
    pub fn shutdown(mut self) {
        if let Some(provider) = self.provider.take()
            && let Err(e) = provider.shutdown()
        {
            tracing::warn!(error = %e, "OpenTelemetry tracer provider shutdown failed");
        }
    }
}

/// Initialize the tracing subscriber.
///
/// When `config.enabled` is `true`, a combined `fmt` + OpenTelemetry subscriber
/// is installed. When disabled, only the standard `fmt` subscriber is used
/// (zero OpenTelemetry overhead).
///
/// If the OTLP exporter fails to build (e.g., invalid endpoint), the server
/// falls back to fmt-only tracing and logs an error rather than panicking.
pub fn init(config: &TelemetryConfig) -> TelemetryGuard {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer();

    if !config.enabled {
        // OTel disabled: plain fmt subscriber only.
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();

        return TelemetryGuard { provider: None };
    }

    // Register the W3C Trace Context propagator globally so that the
    // trace_context middleware can extract `traceparent`/`tracestate` headers.
    global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());

    // Build the OTLP exporter (graceful fallback on failure).
    let exporter = match build_exporter(config) {
        Ok(exporter) => exporter,
        Err(e) => {
            // Install fmt-only subscriber and log the error. Telemetry
            // misconfiguration should not prevent the server from starting.
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .init();
            tracing::error!(
                error = %e,
                endpoint = %config.endpoint,
                protocol = %config.protocol,
                "failed to build OTLP exporter, falling back to fmt-only tracing"
            );
            return TelemetryGuard { provider: None };
        }
    };

    // Build resource attributes.
    let mut resource_kvs = vec![
        KeyValue::new("service.name", config.service_name.clone()),
        KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        KeyValue::new("process.pid", std::process::id().to_string()),
    ];
    if let Ok(hostname) = std::env::var("HOSTNAME").or_else(|_| std::env::var("HOST")) {
        resource_kvs.push(KeyValue::new("host.name", hostname));
    }
    for (k, v) in &config.resource_attributes {
        resource_kvs.push(KeyValue::new(k.clone(), v.clone()));
    }
    let resource = Resource::builder().with_attributes(resource_kvs).build();

    // Build the sampler.
    let sampler = if (config.sample_ratio - 1.0).abs() < f64::EPSILON {
        Sampler::AlwaysOn
    } else if config.sample_ratio <= 0.0 {
        Sampler::AlwaysOff
    } else {
        Sampler::TraceIdRatioBased(config.sample_ratio)
    };

    // Build the tracer provider with a batch span processor.
    let provider = SdkTracerProvider::builder()
        .with_span_processor(BatchSpanProcessor::builder(exporter).build())
        .with_sampler(sampler)
        .with_resource(resource)
        .build();

    // Register as the global tracer provider.
    global::set_tracer_provider(provider.clone());

    let tracer = provider.tracer("acteon");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_layer)
        .init();

    info!(
        endpoint = %config.endpoint,
        protocol = %config.protocol,
        sample_ratio = config.sample_ratio,
        "OpenTelemetry tracing enabled"
    );

    TelemetryGuard {
        provider: Some(provider),
    }
}

/// Build the OTLP span exporter based on the configured protocol.
///
/// Returns `Err` if the exporter fails to build (invalid endpoint, TLS
/// misconfiguration, etc.).
fn build_exporter(
    config: &TelemetryConfig,
) -> Result<opentelemetry_otlp::SpanExporter, opentelemetry::trace::TraceError> {
    let timeout = Duration::from_secs(config.timeout_seconds);

    match config.protocol.as_str() {
        "http" => opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(&config.endpoint)
            .with_timeout(timeout)
            .build(),
        "grpc" => opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&config.endpoint)
            .with_timeout(timeout)
            .build(),
        other => {
            tracing::warn!(
                protocol = %other,
                "unknown telemetry protocol, defaulting to gRPC"
            );
            opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(&config.endpoint)
                .with_timeout(timeout)
                .build()
        }
    }
}
