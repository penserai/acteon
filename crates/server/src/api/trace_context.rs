//! W3C Trace Context propagation middleware for Axum.
//!
//! Extracts `traceparent` and `tracestate` headers from incoming HTTP requests
//! and injects them into the current `tracing` span context so that
//! OpenTelemetry can link incoming traces to the server-side spans.

use std::collections::HashMap;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::{global, trace::TraceContextExt};
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Carrier that reads from HTTP header maps.
struct HeaderExtractor<'a>(&'a axum::http::HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(axum::http::HeaderName::as_str).collect()
    }
}

/// Carrier that writes to a `HashMap`.
struct MapInjector<'a>(&'a mut HashMap<String, String>);

impl Injector for MapInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
}

/// Carrier that reads from a `HashMap`.
struct MapExtractor<'a>(&'a HashMap<String, String>);

impl Extractor for MapExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}

/// Capture the current span's trace context into a [`HashMap`].
pub fn capture_trace_context() -> HashMap<String, String> {
    let mut context = HashMap::new();
    let cx = tracing::Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut MapInjector(&mut context));
    });
    context
}

/// Restore trace context from a [`HashMap`] and set it as the parent of the current span.
#[allow(clippy::implicit_hasher)]
pub fn restore_trace_context(context: &HashMap<String, String>) {
    if context.is_empty() {
        return;
    }

    let parent_cx = global::get_text_map_propagator(|p| {
        let extractor = MapExtractor(context);
        p.extract(&extractor)
    });

    if parent_cx.span().span_context().is_valid() {
        tracing::Span::current().set_parent(parent_cx);
    }
}

/// Axum middleware that extracts W3C Trace Context from incoming requests.
///
/// When a valid `traceparent` header is present, the extracted context becomes
/// the parent of the current span, linking this request to the caller's trace.
/// When absent, a new root context is used (no-op).
pub async fn propagate_trace_context(request: Request, next: Next) -> Response {
    let parent_cx = global::get_text_map_propagator(|p| {
        let extractor = HeaderExtractor(request.headers());
        p.extract(&extractor)
    });

    // If the extracted context has a valid remote span, set it as the parent
    // of the current tracing span so OTel links them.
    if parent_cx.span().span_context().is_remote() {
        tracing::Span::current().set_parent(parent_cx);
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    // -- HeaderExtractor unit tests -------------------------------------------

    #[test]
    fn extractor_get_returns_value_for_present_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
                .parse()
                .unwrap(),
        );

        let extractor = HeaderExtractor(&headers);
        let value = extractor.get("traceparent");
        assert_eq!(
            value,
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        );
    }

    #[test]
    fn extractor_get_returns_none_for_missing_header() {
        let headers = HeaderMap::new();
        let extractor = HeaderExtractor(&headers);
        assert!(extractor.get("traceparent").is_none());
    }

    #[test]
    fn extractor_get_returns_none_for_non_ascii_value() {
        let mut headers = HeaderMap::new();
        // HeaderValue can hold non-UTF-8 bytes; to_str() returns Err for those.
        headers.insert(
            "traceparent",
            axum::http::HeaderValue::from_bytes(&[0x80, 0x81]).unwrap(),
        );

        let extractor = HeaderExtractor(&headers);
        assert!(extractor.get("traceparent").is_none());
    }

    #[test]
    fn extractor_keys_returns_all_header_names() {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", "value1".parse().unwrap());
        headers.insert("tracestate", "value2".parse().unwrap());
        headers.insert("x-custom", "value3".parse().unwrap());

        let extractor = HeaderExtractor(&headers);
        let keys = extractor.keys();
        assert!(keys.contains(&"traceparent"));
        assert!(keys.contains(&"tracestate"));
        assert!(keys.contains(&"x-custom"));
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn extractor_keys_empty_headers() {
        let headers = HeaderMap::new();
        let extractor = HeaderExtractor(&headers);
        assert!(extractor.keys().is_empty());
    }

    #[test]
    fn extractor_tracestate_preserved() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
                .parse()
                .unwrap(),
        );
        headers.insert(
            "tracestate",
            "congo=t61rcWkgMzE,rojo=00f067aa0ba902b7".parse().unwrap(),
        );

        let extractor = HeaderExtractor(&headers);
        assert_eq!(
            extractor.get("tracestate"),
            Some("congo=t61rcWkgMzE,rojo=00f067aa0ba902b7")
        );
    }

    // -- Propagation integration tests ----------------------------------------

    #[test]
    fn valid_traceparent_extracts_remote_context() {
        // Register the W3C propagator for this test.
        global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
                .parse()
                .unwrap(),
        );

        let cx = global::get_text_map_propagator(|p| {
            let extractor = HeaderExtractor(&headers);
            p.extract(&extractor)
        });

        let span_ctx = cx.span().span_context().clone();
        assert!(span_ctx.is_remote());
        assert!(span_ctx.is_valid());
        assert_eq!(
            span_ctx.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(span_ctx.span_id().to_string(), "00f067aa0ba902b7");
    }

    #[test]
    fn missing_traceparent_yields_non_remote_context() {
        global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );

        let headers = HeaderMap::new();

        let cx = global::get_text_map_propagator(|p| {
            let extractor = HeaderExtractor(&headers);
            p.extract(&extractor)
        });

        // No remote span should be present.
        assert!(!cx.span().span_context().is_remote());
    }

    #[test]
    fn invalid_traceparent_yields_non_remote_context() {
        global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );

        let mut headers = HeaderMap::new();
        headers.insert("traceparent", "not-a-valid-traceparent".parse().unwrap());

        let cx = global::get_text_map_propagator(|p| {
            let extractor = HeaderExtractor(&headers);
            p.extract(&extractor)
        });

        // Invalid traceparent should be gracefully ignored.
        assert!(!cx.span().span_context().is_remote());
    }
}
