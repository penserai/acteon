//! SSE event stream support for the Acteon client.
//!
//! Provides [`StreamFilter`] for configuring event subscriptions and
//! an async [`Stream`] implementation that parses SSE frames from the
//! `/v1/stream` endpoint.

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncBufReadExt;
use tokio_util::io::StreamReader;

use acteon_core::StreamEvent;

use crate::Error;

/// Filter parameters for the SSE event stream.
///
/// Use the builder methods to configure which events to receive.
///
/// # Example
///
/// ```
/// use acteon_client::StreamFilter;
///
/// let filter = StreamFilter::new()
///     .namespace("notifications")
///     .action_type("send_email")
///     .outcome("executed");
/// ```
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StreamFilter {
    /// Filter by namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Filter by action type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_type: Option<String>,
    /// Filter by outcome category.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Filter by stream event type tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    /// Filter by chain ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_id: Option<String>,
    /// Filter by group ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// Filter by action ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
}

impl StreamFilter {
    /// Create an empty filter (receives all events).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter events by namespace.
    #[must_use]
    pub fn namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    /// Filter events by action type.
    #[must_use]
    pub fn action_type(mut self, action_type: impl Into<String>) -> Self {
        self.action_type = Some(action_type.into());
        self
    }

    /// Filter events by outcome category (e.g., `executed`, `suppressed`, `failed`).
    #[must_use]
    pub fn outcome(mut self, outcome: impl Into<String>) -> Self {
        self.outcome = Some(outcome.into());
        self
    }

    /// Filter events by stream event type (e.g., `action_dispatched`, `group_flushed`).
    #[must_use]
    pub fn event_type(mut self, event_type: impl Into<String>) -> Self {
        self.event_type = Some(event_type.into());
        self
    }

    /// Filter events by chain ID.
    #[must_use]
    pub fn chain_id(mut self, chain_id: impl Into<String>) -> Self {
        self.chain_id = Some(chain_id.into());
        self
    }

    /// Filter events by group ID.
    #[must_use]
    pub fn group_id(mut self, group_id: impl Into<String>) -> Self {
        self.group_id = Some(group_id.into());
        self
    }

    /// Filter events by action ID.
    #[must_use]
    pub fn action_id(mut self, action_id: impl Into<String>) -> Self {
        self.action_id = Some(action_id.into());
        self
    }
}

/// A single SSE frame received from the server.
#[derive(Debug, Clone)]
pub struct SseFrame {
    /// The SSE event type (from `event:` line).
    pub event: Option<String>,
    /// The event ID (from `id:` line).
    pub id: Option<String>,
    /// The event data (from `data:` line(s)).
    pub data: String,
}

/// An item yielded by the [`EventStream`].
#[derive(Debug)]
pub enum StreamItem {
    /// A parsed `StreamEvent` from the gateway.
    Event(Box<StreamEvent>),
    /// The server indicated the client missed events due to backpressure.
    Lagged {
        /// Number of events that were skipped.
        skipped: u64,
    },
    /// A keep-alive comment was received (stream is still alive).
    KeepAlive,
}

/// An async stream of SSE events from the Acteon gateway.
///
/// Created via [`ActeonClient::stream`](crate::ActeonClient::stream).
/// Implements `futures::Stream<Item = Result<StreamItem, Error>>`.
pub struct EventStream {
    inner: Pin<Box<dyn Stream<Item = Result<StreamItem, Error>> + Send>>,
}

impl Stream for EventStream {
    type Item = Result<StreamItem, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// Create an `EventStream` from a reqwest response that returns SSE data.
pub(crate) fn event_stream_from_response(response: reqwest::Response) -> EventStream {
    let byte_stream = response.bytes_stream();
    let reader = StreamReader::new(byte_stream.map(|result| result.map_err(std::io::Error::other)));
    let lines = tokio::io::BufReader::new(reader).lines();

    let stream = futures::stream::unfold(
        (lines, SseFrameState::default()),
        |(mut lines, mut frame_state)| async move {
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if line.is_empty() {
                            // Blank line = end of SSE frame.
                            if let Some(frame) = frame_state.take_frame() {
                                let item = parse_sse_frame(&frame);
                                return Some((item, (lines, frame_state)));
                            }
                            // Empty frame (e.g., double newline), skip.
                            continue;
                        }

                        if let Some(rest) = line.strip_prefix(':') {
                            // SSE comment (keep-alive).
                            let _ = rest;
                            let item = Ok(StreamItem::KeepAlive);
                            return Some((item, (lines, frame_state)));
                        }

                        if let Some(value) = line.strip_prefix("event:") {
                            frame_state.event = Some(value.trim().to_string());
                        } else if let Some(value) = line.strip_prefix("data:") {
                            frame_state.push_data(value.trim());
                        } else if let Some(value) = line.strip_prefix("id:") {
                            frame_state.id = Some(value.trim().to_string());
                        }
                        // Ignore unknown fields per SSE spec.
                    }
                    Ok(None) => {
                        // Stream ended.
                        return None;
                    }
                    Err(e) => {
                        return Some((
                            Err(Error::Connection(format!("SSE stream error: {e}"))),
                            (lines, frame_state),
                        ));
                    }
                }
            }
        },
    );

    EventStream {
        inner: Box::pin(stream),
    }
}

/// Intermediate state for parsing SSE frames line-by-line.
#[derive(Default)]
struct SseFrameState {
    event: Option<String>,
    id: Option<String>,
    data: Vec<String>,
}

impl SseFrameState {
    fn push_data(&mut self, line: &str) {
        self.data.push(line.to_string());
    }

    fn take_frame(&mut self) -> Option<SseFrame> {
        if self.data.is_empty() && self.event.is_none() && self.id.is_none() {
            return None;
        }
        let frame = SseFrame {
            event: self.event.take(),
            id: self.id.take(),
            data: std::mem::take(&mut self.data).join("\n"),
        };
        Some(frame)
    }
}

/// Parse an SSE frame into a `StreamItem`.
fn parse_sse_frame(frame: &SseFrame) -> Result<StreamItem, Error> {
    let event_type = frame.event.as_deref().unwrap_or("message");

    if event_type == "lagged" {
        let skipped = serde_json::from_str::<serde_json::Value>(&frame.data)
            .ok()
            .and_then(|v| v.get("skipped")?.as_u64())
            .unwrap_or(0);
        Ok(StreamItem::Lagged { skipped })
    } else {
        let event: StreamEvent = serde_json::from_str(&frame.data)
            .map_err(|e| Error::Deserialization(format!("failed to parse SSE event: {e}")))?;
        Ok(StreamItem::Event(Box::new(event)))
    }
}

// The `map` call on `bytes_stream()` requires this import.
use futures::StreamExt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_filter_serialization() {
        let filter = StreamFilter::new()
            .namespace("alerts")
            .action_type("send_email");

        let json = serde_json::to_value(&filter).unwrap();
        assert_eq!(json["namespace"], "alerts");
        assert_eq!(json["action_type"], "send_email");
        assert!(json.get("outcome").is_none());
        assert!(json.get("event_type").is_none());
    }

    #[test]
    fn stream_filter_empty() {
        let filter = StreamFilter::new();
        let json = serde_json::to_value(&filter).unwrap();
        assert!(json.as_object().unwrap().is_empty());
    }

    #[test]
    fn parse_lagged_frame() {
        let frame = SseFrame {
            event: Some("lagged".into()),
            id: None,
            data: r#"{"skipped":42}"#.into(),
        };
        let item = parse_sse_frame(&frame).unwrap();
        match item {
            StreamItem::Lagged { skipped } => assert_eq!(skipped, 42),
            other => panic!("expected Lagged, got {other:?}"),
        }
    }

    #[test]
    fn parse_action_dispatched_frame() {
        let event = StreamEvent {
            id: "test-id".into(),
            timestamp: chrono::Utc::now(),
            event_type: acteon_core::StreamEventType::GroupFlushed {
                group_id: "g1".into(),
                event_count: 3,
            },
            namespace: "ns".into(),
            tenant: "t1".into(),
            action_type: None,
            action_id: None,
        };
        let json = serde_json::to_string(&event).unwrap();

        let frame = SseFrame {
            event: Some("group_flushed".into()),
            id: Some("test-id".into()),
            data: json,
        };

        let item = parse_sse_frame(&frame).unwrap();
        match item {
            StreamItem::Event(e) => {
                assert_eq!(e.id, "test-id");
                assert_eq!(e.namespace, "ns");
            }
            other => panic!("expected Event, got {other:?}"),
        }
    }

    #[test]
    fn sse_frame_state_collects_data_lines() {
        let mut state = SseFrameState::default();
        state.event = Some("action_dispatched".into());
        state.push_data("line1");
        state.push_data("line2");

        let frame = state.take_frame().unwrap();
        assert_eq!(frame.event.as_deref(), Some("action_dispatched"));
        assert_eq!(frame.data, "line1\nline2");
    }

    #[test]
    fn sse_frame_state_empty_returns_none() {
        let mut state = SseFrameState::default();
        assert!(state.take_frame().is_none());
    }
}
