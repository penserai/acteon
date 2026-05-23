//! Opaque pagination cursor for audit query results.
//!
//! Cursors encode the sort key of the last record on a page so the next
//! query can resume from that point without a server-side `OFFSET` scan.
//! This keeps page latency constant regardless of how deep the caller has
//! paged.
//!
//! The cursor is an opaque base64url-encoded JSON blob — callers should
//! treat it as an opaque string and round-trip it verbatim.

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::AuditError;

/// Decoded pagination cursor.
///
/// Carries the sort key of the last record on the previous page. The
/// `kind` field selects which sort key is in use; backends pick the
/// matching variant based on the query's sort order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditCursor {
    /// Cursor format version. Reserved for future incompatible changes.
    #[serde(rename = "v")]
    pub version: u8,
    /// Sort key in use: `"ts"` for `(dispatched_at, id)` or `"seq"` for
    /// `sequence_number`.
    #[serde(rename = "k")]
    pub kind: CursorKind,
    /// `dispatched_at` of the last record on the previous page, in
    /// milliseconds since epoch. Present when `kind == Ts`.
    #[serde(rename = "t", default, skip_serializing_if = "Option::is_none")]
    pub dispatched_at_ms: Option<i64>,
    /// `id` of the last record on the previous page, used as a tiebreaker
    /// when `dispatched_at` collides. Present when `kind == Ts`.
    #[serde(rename = "i", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `sequence_number` of the last record on the previous page. Present
    /// when `kind == Seq`.
    #[serde(rename = "s", default, skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<u64>,
}

/// Which sort key the cursor encodes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CursorKind {
    /// `(dispatched_at DESC, id DESC)` — the default audit query order.
    #[serde(rename = "ts")]
    Ts,
    /// `(sequence_number ASC)` — used by hash chain verification.
    #[serde(rename = "seq")]
    Seq,
}

impl AuditCursor {
    /// Build a `(dispatched_at, id)` cursor pointing at the last record on
    /// a page.
    #[must_use]
    pub fn from_timestamp(dispatched_at_ms: i64, id: impl Into<String>) -> Self {
        Self {
            version: 1,
            kind: CursorKind::Ts,
            dispatched_at_ms: Some(dispatched_at_ms),
            id: Some(id.into()),
            sequence_number: None,
        }
    }

    /// Build a `sequence_number` cursor for hash chain ordering.
    ///
    /// `id` is carried alongside the sequence number so backends that
    /// need a tiebreaker (and `DynamoDB`'s `ExclusiveStartKey`, which
    /// requires the table primary key) can resume cleanly.
    #[must_use]
    pub fn from_sequence(sequence_number: u64, id: impl Into<String>) -> Self {
        Self {
            version: 1,
            kind: CursorKind::Seq,
            dispatched_at_ms: None,
            id: Some(id.into()),
            sequence_number: Some(sequence_number),
        }
    }

    /// Encode the cursor as an opaque base64url string.
    pub fn encode(&self) -> Result<String, AuditError> {
        let json = serde_json::to_vec(self)
            .map_err(|e| AuditError::Serialization(format!("cursor encode: {e}")))?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
    }

    /// Decode an opaque cursor produced by [`AuditCursor::encode`].
    pub fn decode(s: &str) -> Result<Self, AuditError> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s.as_bytes())
            .map_err(|e| AuditError::Serialization(format!("cursor decode: {e}")))?;
        let cursor: Self = serde_json::from_slice(&bytes)
            .map_err(|e| AuditError::Serialization(format!("cursor decode: {e}")))?;
        if cursor.version != 1 {
            return Err(AuditError::Serialization(format!(
                "unsupported cursor version: {}",
                cursor.version
            )));
        }
        Ok(cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_cursor_roundtrip() {
        let cursor = AuditCursor::from_timestamp(1_700_000_000_000, "rec-123");
        let encoded = cursor.encode().unwrap();
        let decoded = AuditCursor::decode(&encoded).unwrap();
        assert_eq!(decoded, cursor);
        assert_eq!(decoded.kind, CursorKind::Ts);
        assert_eq!(decoded.dispatched_at_ms, Some(1_700_000_000_000));
        assert_eq!(decoded.id.as_deref(), Some("rec-123"));
    }

    #[test]
    fn sequence_cursor_roundtrip() {
        let cursor = AuditCursor::from_sequence(42, "rec-9");
        let encoded = cursor.encode().unwrap();
        let decoded = AuditCursor::decode(&encoded).unwrap();
        assert_eq!(decoded, cursor);
        assert_eq!(decoded.kind, CursorKind::Seq);
        assert_eq!(decoded.sequence_number, Some(42));
        assert_eq!(decoded.id.as_deref(), Some("rec-9"));
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(AuditCursor::decode("not-base64!!!").is_err());
        assert!(AuditCursor::decode("aGVsbG8").is_err()); // valid b64 but not JSON
    }

    #[test]
    fn encoded_cursor_is_opaque_url_safe() {
        let cursor = AuditCursor::from_timestamp(1_700_000_000_000, "rec-123");
        let encoded = cursor.encode().unwrap();
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains('='));
    }
}
