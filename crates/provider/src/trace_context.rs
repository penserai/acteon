//! Outbound W3C Trace Context propagation for HTTP providers.
//!
//! Injects `traceparent` and `tracestate` headers into outgoing HTTP requests
//! so that downstream services can link their traces back to Acteon's spans.
//!
//! When OpenTelemetry is disabled (no global propagator registered), the
//! injector is a no-op and adds zero headers.

use opentelemetry::propagation::Injector;
use opentelemetry::{Context, global};

/// A [`reqwest::header::HeaderMap`]-backed injector for OpenTelemetry propagators.
struct HeaderInjector<'a>(&'a mut reqwest::header::HeaderMap);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let Ok(name) = reqwest::header::HeaderName::from_bytes(key.as_bytes())
            && let Ok(val) = reqwest::header::HeaderValue::from_str(&value)
        {
            self.0.insert(name, val);
        }
    }
}

/// Inject the current span's trace context into a [`reqwest::RequestBuilder`].
///
/// Adds `traceparent` and `tracestate` headers (W3C Trace Context) so that
/// the receiving service can continue the distributed trace.
///
/// This is a no-op when no global text-map propagator has been registered
/// (i.e. when OpenTelemetry tracing is disabled).
pub fn inject_trace_context(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let mut temp_headers = reqwest::header::HeaderMap::new();
    let cx = Context::current();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut HeaderInjector(&mut temp_headers));
    });

    let mut builder = builder;
    for (name, value) in temp_headers {
        if let Some(name) = name {
            builder = builder.header(name, value);
        }
    }
    builder
}
