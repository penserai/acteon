use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;

use acteon_audit::cursor::{AuditCursor, CursorKind};
use acteon_audit::error::AuditError;
use acteon_audit::record::{AuditPage, AuditQuery, AuditRecord};
use acteon_audit::store::AuditStore;

/// In-memory audit store using `DashMap`. Suitable for development and testing.
///
/// Records are stored in a concurrent hash map keyed by record ID, with a
/// secondary index from action ID to record IDs.
pub struct MemoryAuditStore {
    /// Primary store: record ID -> `AuditRecord`.
    records: DashMap<String, AuditRecord>,
    /// Secondary index: action ID -> list of record IDs.
    action_index: DashMap<String, Vec<String>>,
}

impl MemoryAuditStore {
    /// Create a new empty in-memory audit store.
    pub fn new() -> Self {
        Self {
            records: DashMap::new(),
            action_index: DashMap::new(),
        }
    }
}

impl Default for MemoryAuditStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuditStore for MemoryAuditStore {
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
        let id = entry.id.clone();
        let action_id = entry.action_id.clone();
        self.records.insert(id.clone(), entry);
        self.action_index.entry(action_id).or_default().push(id);
        Ok(())
    }

    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError> {
        let ids = self.action_index.get(action_id);
        let Some(ids) = ids else {
            return Ok(None);
        };

        // Return the most recent record for this action ID.
        let mut best: Option<AuditRecord> = None;
        for id in ids.value() {
            if let Some(rec) = self.records.get(id) {
                let rec = rec.value();
                if best
                    .as_ref()
                    .is_none_or(|b| rec.dispatched_at > b.dispatched_at)
                {
                    best = Some(rec.clone());
                }
            }
        }
        Ok(best)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
        Ok(self.records.get(id).map(|r| r.value().clone()))
    }

    #[allow(clippy::too_many_lines)]
    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        let limit = query.effective_limit();

        // Collect all matching records.
        let mut matching: Vec<AuditRecord> = self
            .records
            .iter()
            .filter_map(|entry| {
                let rec = entry.value();
                if !matches_filter(query.namespace.as_ref(), &rec.namespace) {
                    return None;
                }
                if !matches_filter(query.tenant.as_ref(), &rec.tenant) {
                    return None;
                }
                if !matches_filter(query.provider.as_ref(), &rec.provider) {
                    return None;
                }
                if !matches_filter(query.action_type.as_ref(), &rec.action_type) {
                    return None;
                }
                if !matches_filter(query.outcome.as_ref(), &rec.outcome) {
                    return None;
                }
                if !matches_filter(query.verdict.as_ref(), &rec.verdict) {
                    return None;
                }
                if let Some(ref rule) = query.matched_rule
                    && rec.matched_rule.as_deref() != Some(rule.as_str())
                {
                    return None;
                }
                if let Some(ref cid) = query.chain_id
                    && rec.chain_id.as_deref() != Some(cid.as_str())
                {
                    return None;
                }
                if let Some(ref sid) = query.signer_id
                    && rec.signer_id.as_deref() != Some(sid.as_str())
                {
                    return None;
                }
                if let Some(ref k) = query.kid
                    && rec.kid.as_deref() != Some(k.as_str())
                {
                    return None;
                }
                if let Some(ref from) = query.from
                    && rec.dispatched_at < *from
                {
                    return None;
                }
                if let Some(ref to) = query.to
                    && rec.dispatched_at > *to
                {
                    return None;
                }
                Some(rec.clone())
            })
            .collect();

        // Sort by the requested order. The default is `dispatched_at DESC`
        // with `id DESC` as a tiebreaker so the cursor key uniquely
        // identifies a row.
        if query.sort_by_sequence_asc {
            matching.sort_by(|a, b| {
                a.sequence_number
                    .unwrap_or(u64::MAX)
                    .cmp(&b.sequence_number.unwrap_or(u64::MAX))
                    .then_with(|| a.id.cmp(&b.id))
            });
        } else {
            matching.sort_by(|a, b| {
                b.dispatched_at
                    .cmp(&a.dispatched_at)
                    .then_with(|| b.id.cmp(&a.id))
            });
        }

        let total_matches = matching.len() as u64;

        // Cursor takes precedence over offset. When neither is set the
        // request behaves like the legacy offset path.
        //
        // We fetch `limit + 1` rows so we can detect a definitive last
        // page without round-tripping an empty cursor: if the over-fetch
        // returned `> limit`, more rows exist and we trim + emit a
        // cursor; otherwise we know we're done.
        let probe = limit as usize + 1;
        let (mut records, used_cursor, offset) = if let Some(cursor_str) = query.cursor.as_deref() {
            let cursor = AuditCursor::decode(cursor_str)?;
            let after: Vec<AuditRecord> = matching
                .into_iter()
                .filter(|rec| record_after_cursor(rec, &cursor, query.sort_by_sequence_asc))
                .take(probe)
                .collect();
            (after, true, 0u32)
        } else {
            let offset = query.effective_offset();
            let page: Vec<AuditRecord> = matching
                .into_iter()
                .skip(offset as usize)
                .take(probe)
                .collect();
            (page, false, offset)
        };

        let has_more = records.len() > limit as usize;
        if has_more {
            records.truncate(limit as usize);
        }

        let next_cursor = if has_more {
            records
                .last()
                .map(|rec| build_cursor(rec, query.sort_by_sequence_asc).encode())
                .transpose()?
        } else {
            None
        };

        let total = if used_cursor {
            None
        } else {
            Some(total_matches)
        };

        Ok(AuditPage {
            records,
            total,
            limit,
            offset,
            next_cursor,
        })
    }

    async fn cleanup_expired(&self) -> Result<u64, AuditError> {
        let now = Utc::now();
        let mut removed = 0u64;

        // Collect IDs to remove (cannot mutate while iterating DashMap).
        let expired_ids: Vec<String> = self
            .records
            .iter()
            .filter_map(|entry| {
                let rec = entry.value();
                if let Some(expires) = rec.expires_at
                    && expires <= now
                {
                    return Some(rec.id.clone());
                }
                None
            })
            .collect();

        for id in expired_ids {
            if let Some((_, rec)) = self.records.remove(&id) {
                // Clean up the action index.
                if let Some(mut ids) = self.action_index.get_mut(&rec.action_id) {
                    ids.retain(|i| i != &id);
                }
                removed += 1;
            }
        }

        Ok(removed)
    }
}

/// Check if a filter matches a value. `None` filter matches everything.
fn matches_filter(filter: Option<&String>, value: &str) -> bool {
    filter.is_none_or(|f| f == value)
}

/// Build the cursor that points at `rec` for the active sort order.
fn build_cursor(rec: &AuditRecord, sort_by_sequence_asc: bool) -> AuditCursor {
    if sort_by_sequence_asc {
        AuditCursor::from_sequence(rec.sequence_number.unwrap_or(0), rec.id.clone())
    } else {
        AuditCursor::from_timestamp(rec.dispatched_at.timestamp_millis(), rec.id.clone())
    }
}

/// Filter predicate: keep records strictly after the cursor in the active
/// sort order.
fn record_after_cursor(
    rec: &AuditRecord,
    cursor: &AuditCursor,
    sort_by_sequence_asc: bool,
) -> bool {
    match (sort_by_sequence_asc, cursor.kind) {
        (true, CursorKind::Seq) => {
            let cursor_seq = cursor.sequence_number.unwrap_or(0);
            rec.sequence_number.is_some_and(|seq| seq > cursor_seq)
        }
        (false, CursorKind::Ts) => {
            let cursor_ms = cursor.dispatched_at_ms.unwrap_or(0);
            let cursor_id = cursor.id.as_deref().unwrap_or("");
            let rec_ms = rec.dispatched_at.timestamp_millis();
            // Default sort is DESC, so "after" means strictly older.
            rec_ms < cursor_ms || (rec_ms == cursor_ms && rec.id.as_str() < cursor_id)
        }
        // Cursor kind mismatch with current sort — treat as no-match.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use acteon_audit::record::{AuditQuery, AuditRecord};
    use acteon_audit::store::AuditStore;

    use super::MemoryAuditStore;

    fn make_record(id: &str, action_id: &str) -> AuditRecord {
        let now = Utc::now();
        AuditRecord {
            id: id.to_owned(),
            action_id: action_id.to_owned(),
            chain_id: None,
            namespace: "ns".to_owned(),
            tenant: "t1".to_owned(),
            provider: "email".to_owned(),
            action_type: "send_email".to_owned(),
            verdict: "allow".to_owned(),
            matched_rule: None,
            outcome: "executed".to_owned(),
            action_payload: None,
            verdict_details: serde_json::json!({}),
            outcome_details: serde_json::json!({}),
            metadata: serde_json::json!({}),
            dispatched_at: now,
            completed_at: now,
            duration_ms: 10,
            expires_at: None,
            caller_id: String::new(),
            auth_method: String::new(),
            record_hash: None,
            previous_hash: None,
            sequence_number: None,
            attachment_metadata: Vec::new(),
            signature: None,
            signer_id: None,
            kid: None,
            canonical_hash: None,
        }
    }

    #[tokio::test]
    async fn record_and_get_by_id() {
        let store = MemoryAuditStore::new();
        let rec = make_record("r1", "a1");
        store.record(rec.clone()).await.unwrap();

        let found = store.get_by_id("r1").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().action_id, "a1");
    }

    #[tokio::test]
    async fn get_by_action_id_returns_most_recent() {
        let store = MemoryAuditStore::new();
        let now = Utc::now();

        let mut r1 = make_record("r1", "a1");
        r1.dispatched_at = now - Duration::seconds(10);
        store.record(r1).await.unwrap();

        let mut r2 = make_record("r2", "a1");
        r2.dispatched_at = now;
        store.record(r2).await.unwrap();

        let found = store.get_by_action_id("a1").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "r2");
    }

    #[tokio::test]
    async fn query_with_filters() {
        let store = MemoryAuditStore::new();
        let mut r1 = make_record("r1", "a1");
        r1.namespace = "ns1".to_owned();
        store.record(r1).await.unwrap();

        let mut r2 = make_record("r2", "a2");
        r2.namespace = "ns2".to_owned();
        store.record(r2).await.unwrap();

        let q = AuditQuery {
            namespace: Some("ns1".to_owned()),
            ..Default::default()
        };
        let page = store.query(&q).await.unwrap();
        assert_eq!(page.total, Some(1));
        assert_eq!(page.records[0].id, "r1");
    }

    /// Inserts five records covering every signing shape the new
    /// filter needs to discriminate:
    /// - `r1`: signer_id=ci-bot, kid=k1
    /// - `r2`: signer_id=ci-bot, kid=k2 (same signer, rotated key)
    /// - `r3`: signer_id=deploy-svc, kid=k1 (different signer, same kid name)
    /// - `r4`: signer_id=ci-bot, no kid (legacy pre-rotation signature)
    /// - `r5`: unsigned
    ///
    /// Then exercises four queries: signer_id alone,
    /// (signer_id, kid) pair, kid alone (across signers), and a
    /// combination that narrows to a single record.
    #[tokio::test]
    async fn query_by_signer_id_and_kid() {
        let store = MemoryAuditStore::new();

        let cases = [
            ("r1", Some("ci-bot"), Some("k1")),
            ("r2", Some("ci-bot"), Some("k2")),
            ("r3", Some("deploy-svc"), Some("k1")),
            ("r4", Some("ci-bot"), None),
            ("r5", None, None),
        ];
        for (id, signer, kid) in &cases {
            let mut rec = make_record(id, id);
            rec.signer_id = signer.map(str::to_owned);
            rec.kid = kid.map(str::to_owned);
            store.record(rec).await.unwrap();
        }

        // signer_id alone — matches r1, r2, r4 (all three ci-bot
        // records regardless of kid, including the legacy no-kid one)
        let page = store
            .query(&AuditQuery {
                signer_id: Some("ci-bot".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut ids: Vec<&str> = page.records.iter().map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["r1", "r2", "r4"]);

        // (signer_id, kid) pair — narrows ci-bot to k1 only
        let page = store
            .query(&AuditQuery {
                signer_id: Some("ci-bot".to_owned()),
                kid: Some("k1".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].id, "r1");

        // kid alone — matches across signers (r1, r3 both have k1)
        let page = store
            .query(&AuditQuery {
                kid: Some("k1".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut ids: Vec<&str> = page.records.iter().map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["r1", "r3"]);

        // Unsigned actions (r5) and legacy-no-kid actions (r4) never
        // match a kid filter.
        let page = store
            .query(&AuditQuery {
                kid: Some("k2".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].id, "r2");

        // Signer query that doesn't match anything returns empty
        // rather than erroring.
        let page = store
            .query(&AuditQuery {
                signer_id: Some("phantom".to_owned()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(page.records.is_empty());
    }

    #[tokio::test]
    async fn query_pagination() {
        let store = MemoryAuditStore::new();
        for i in 0..10 {
            let mut rec = make_record(&format!("r{i}"), &format!("a{i}"));
            rec.dispatched_at = Utc::now() + Duration::seconds(i64::from(i));
            store.record(rec).await.unwrap();
        }

        let q = AuditQuery {
            limit: Some(3),
            offset: Some(2),
            ..Default::default()
        };
        let page = store.query(&q).await.unwrap();
        assert_eq!(page.total, Some(10));
        assert_eq!(page.records.len(), 3);
        assert_eq!(page.limit, 3);
        assert_eq!(page.offset, 2);
    }

    #[tokio::test]
    async fn query_cursor_pagination_walks_all_records() {
        let store = MemoryAuditStore::new();
        for i in 0..10 {
            let mut rec = make_record(&format!("r{i:02}"), &format!("a{i}"));
            rec.dispatched_at = Utc::now() + Duration::seconds(i64::from(i));
            store.record(rec).await.unwrap();
        }

        let mut cursor: Option<String> = None;
        let mut seen: Vec<String> = Vec::new();
        loop {
            let q = AuditQuery {
                limit: Some(3),
                cursor: cursor.clone(),
                ..Default::default()
            };
            let page = store.query(&q).await.unwrap();
            // Only the cursor-driven follow-up pages skip the count.
            if cursor.is_some() {
                assert!(page.total.is_none(), "cursor pagination should skip count");
            }
            for rec in &page.records {
                seen.push(rec.id.clone());
            }
            if page.next_cursor.is_none() {
                break;
            }
            cursor = page.next_cursor;
        }
        assert_eq!(seen.len(), 10);
        // Records should be in the default sort: dispatched_at DESC.
        for w in seen.windows(2) {
            assert!(w[0] > w[1], "expected DESC order: {:?}", w);
        }
    }

    #[tokio::test]
    async fn query_cursor_first_page_no_cursor() {
        let store = MemoryAuditStore::new();
        for i in 0..5 {
            let mut rec = make_record(&format!("r{i}"), &format!("a{i}"));
            rec.dispatched_at = Utc::now() + Duration::seconds(i64::from(i));
            store.record(rec).await.unwrap();
        }
        let q = AuditQuery {
            limit: Some(2),
            ..Default::default()
        };
        let page = store.query(&q).await.unwrap();
        assert_eq!(page.records.len(), 2);
        assert!(page.next_cursor.is_some());
        assert_eq!(page.total, Some(5));
    }

    #[tokio::test]
    async fn cleanup_expired() {
        let store = MemoryAuditStore::new();

        let mut r1 = make_record("r1", "a1");
        r1.expires_at = Some(Utc::now() - Duration::seconds(60));
        store.record(r1).await.unwrap();

        let mut r2 = make_record("r2", "a2");
        r2.expires_at = Some(Utc::now() + Duration::hours(1));
        store.record(r2).await.unwrap();

        let r3 = make_record("r3", "a3"); // no expiry
        store.record(r3).await.unwrap();

        let removed = store.cleanup_expired().await.unwrap();
        assert_eq!(removed, 1);

        assert!(store.get_by_id("r1").await.unwrap().is_none());
        assert!(store.get_by_id("r2").await.unwrap().is_some());
        assert!(store.get_by_id("r3").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn get_nonexistent_returns_none() {
        let store = MemoryAuditStore::new();
        assert!(store.get_by_id("nope").await.unwrap().is_none());
        assert!(store.get_by_action_id("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn query_time_range() {
        let store = MemoryAuditStore::new();
        let now = Utc::now();

        let mut r1 = make_record("r1", "a1");
        r1.dispatched_at = now - Duration::hours(2);
        store.record(r1).await.unwrap();

        let mut r2 = make_record("r2", "a2");
        r2.dispatched_at = now;
        store.record(r2).await.unwrap();

        let q = AuditQuery {
            from: Some(now - Duration::hours(1)),
            ..Default::default()
        };
        let page = store.query(&q).await.unwrap();
        assert_eq!(page.total, Some(1));
        assert_eq!(page.records[0].id, "r2");
    }
}
