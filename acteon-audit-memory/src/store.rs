use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;

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
                    .map_or(true, |b| rec.dispatched_at > b.dispatched_at)
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

    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        let limit = query.effective_limit();
        let offset = query.effective_offset();

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
                if let Some(ref rule) = query.matched_rule {
                    if rec.matched_rule.as_deref() != Some(rule.as_str()) {
                        return None;
                    }
                }
                if let Some(ref from) = query.from {
                    if rec.dispatched_at < *from {
                        return None;
                    }
                }
                if let Some(ref to) = query.to {
                    if rec.dispatched_at > *to {
                        return None;
                    }
                }
                Some(rec.clone())
            })
            .collect();

        // Sort by dispatched_at descending.
        matching.sort_by(|a, b| b.dispatched_at.cmp(&a.dispatched_at));

        let total = matching.len() as u64;
        let records: Vec<AuditRecord> = matching
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();

        Ok(AuditPage {
            records,
            total,
            limit,
            offset,
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
                if let Some(expires) = rec.expires_at {
                    if expires <= now {
                        return Some(rec.id.clone());
                    }
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
    filter.map_or(true, |f| f == value)
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
        assert_eq!(page.total, 1);
        assert_eq!(page.records[0].id, "r1");
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
        assert_eq!(page.total, 10);
        assert_eq!(page.records.len(), 3);
        assert_eq!(page.limit, 3);
        assert_eq!(page.offset, 2);
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
        assert_eq!(page.total, 1);
        assert_eq!(page.records[0].id, "r2");
    }
}
