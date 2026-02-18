//! Compliance-aware audit store decorators.
//!
//! This module provides two decorators:
//!
//! - [`HashChainAuditStore`] — computes `SHA-256` hash chains across audit records
//!   within each `(namespace, tenant)` pair, enabling tamper-evident logging.
//! - [`ComplianceAuditStore`] — enforces compliance rules such as immutable audit
//!   (blocking deletions when enabled).
//!
//! Wrapping order should be:
//! `ComplianceAuditStore(HashChainAuditStore(EncryptingAuditStore(RedactingAuditStore(Inner))))`

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::warn;

use acteon_core::compliance::{ComplianceConfig, HashChainVerification};

use crate::error::AuditError;
use crate::record::{AuditPage, AuditQuery, AuditRecord};
use crate::store::AuditStore;

/// Key for tracking the last hash and sequence number per `(namespace, tenant)`.
type ChainKey = (String, String);

/// Cached state for the most recent record in a `(namespace, tenant)` chain.
#[derive(Clone, Debug)]
struct ChainTip {
    /// Hash of the most recent record.
    record_hash: String,
    /// Sequence number of the most recent record.
    sequence_number: u64,
}

/// An audit store decorator that computes `SHA-256` hash chains.
///
/// Each audit record receives:
/// - `previous_hash` — the `record_hash` of the preceding record in the same
///   `(namespace, tenant)` pair (or `None` for the first record).
/// - `record_hash` — `SHA-256` hex digest of canonicalized record fields.
/// - `sequence_number` — monotonically increasing counter within the pair.
///
/// The chain tip is cached in memory. On first write for a pair, the decorator
/// queries the inner store to discover the current tip.
pub struct HashChainAuditStore {
    inner: Arc<dyn AuditStore>,
    /// Per-(namespace, tenant) chain tip cache, protected by async mutex
    /// to ensure sequential hash chain computation.
    tips: Mutex<HashMap<ChainKey, Option<ChainTip>>>,
}

impl HashChainAuditStore {
    /// Create a new `HashChainAuditStore` wrapping the given inner store.
    pub fn new(inner: Arc<dyn AuditStore>) -> Self {
        Self {
            inner,
            tips: Mutex::new(HashMap::new()),
        }
    }

    /// Compute the canonical `SHA-256` hash of an audit record.
    ///
    /// The hash covers a deterministic subset of fields to ensure reproducibility.
    fn compute_record_hash(record: &AuditRecord) -> String {
        let canonical = serde_json::json!({
            "id": record.id,
            "action_id": record.action_id,
            "namespace": record.namespace,
            "tenant": record.tenant,
            "provider": record.provider,
            "action_type": record.action_type,
            "verdict": record.verdict,
            "outcome": record.outcome,
            "dispatched_at": record.dispatched_at.to_rfc3339(),
            "previous_hash": record.previous_hash,
        });

        let bytes = canonical.to_string().into_bytes();
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        format!("{:x}", hasher.finalize())
    }

    /// Look up the chain tip for a `(namespace, tenant)` pair by querying the
    /// inner store if not cached.
    async fn get_or_fetch_tip(
        &self,
        namespace: &str,
        tenant: &str,
        tips: &mut HashMap<ChainKey, Option<ChainTip>>,
    ) -> Result<Option<ChainTip>, AuditError> {
        let key = (namespace.to_string(), tenant.to_string());
        if let Some(tip) = tips.get(&key) {
            return Ok(tip.clone());
        }

        // Query the inner store for the most recent record in this pair.
        let query = AuditQuery {
            namespace: Some(namespace.to_string()),
            tenant: Some(tenant.to_string()),
            limit: Some(1),
            ..AuditQuery::default()
        };

        let page = self.inner.query(&query).await?;
        let tip = page.records.into_iter().next().and_then(|r| {
            r.record_hash.map(|hash| ChainTip {
                record_hash: hash,
                sequence_number: r.sequence_number.unwrap_or(0),
            })
        });

        tips.insert(key, tip.clone());
        Ok(tip)
    }

    /// Verify the integrity of the hash chain for a `(namespace, tenant)` pair.
    ///
    /// Queries all records in the pair (paginated), re-computes each hash, and
    /// verifies that `previous_hash` links form an unbroken chain.
    pub async fn verify_chain(
        &self,
        namespace: &str,
        tenant: &str,
        from: Option<chrono::DateTime<chrono::Utc>>,
        to: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<HashChainVerification, AuditError> {
        let mut all_records = Vec::new();
        let mut offset = 0u32;
        let page_size: u32 = 1000;

        // Fetch all records in the range, paginated.
        loop {
            let query = AuditQuery {
                namespace: Some(namespace.to_string()),
                tenant: Some(tenant.to_string()),
                from,
                to,
                limit: Some(page_size),
                offset: Some(offset),
                ..AuditQuery::default()
            };

            let page = self.inner.query(&query).await?;
            let fetched = page.records.len();
            all_records.extend(page.records);

            if fetched < usize::try_from(page_size).unwrap_or(usize::MAX) {
                break;
            }
            offset = offset.saturating_add(page_size);
        }

        if all_records.is_empty() {
            return Ok(HashChainVerification {
                valid: true,
                records_checked: 0,
                first_broken_at: None,
                first_record_id: None,
                last_record_id: None,
            });
        }

        // Sort by sequence number ascending (first record first).
        all_records.sort_by_key(|r| r.sequence_number.unwrap_or(0));

        let first_id = Some(all_records.first().unwrap().id.clone());
        let last_id = Some(all_records.last().unwrap().id.clone());

        let mut previous_hash: Option<String> = None;
        let mut first_broken_at: Option<String> = None;

        for record in &all_records {
            // Verify previous_hash linkage.
            if record.previous_hash != previous_hash {
                first_broken_at = Some(record.id.clone());
                break;
            }

            // Re-compute the hash and verify it matches.
            let expected_hash = Self::compute_record_hash(record);
            match &record.record_hash {
                Some(hash) if *hash == expected_hash => {
                    previous_hash = Some(hash.clone());
                }
                _ => {
                    first_broken_at = Some(record.id.clone());
                    break;
                }
            }
        }

        Ok(HashChainVerification {
            valid: first_broken_at.is_none(),
            records_checked: all_records.len() as u64,
            first_broken_at,
            first_record_id: first_id,
            last_record_id: last_id,
        })
    }
}

#[async_trait]
impl AuditStore for HashChainAuditStore {
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
        let mut tips = self.tips.lock().await;
        let tip = self
            .get_or_fetch_tip(&entry.namespace, &entry.tenant, &mut tips)
            .await?;

        let mut chained = entry;
        let (prev_hash, seq) = match tip {
            Some(t) => (Some(t.record_hash), t.sequence_number + 1),
            None => (None, 0),
        };

        chained.previous_hash = prev_hash;
        chained.sequence_number = Some(seq);
        chained.record_hash = Some(Self::compute_record_hash(&chained));

        // Update the cached tip.
        let key = (chained.namespace.clone(), chained.tenant.clone());
        tips.insert(
            key,
            Some(ChainTip {
                record_hash: chained.record_hash.clone().unwrap(),
                sequence_number: seq,
            }),
        );

        // Release the lock before the async inner write.
        drop(tips);

        self.inner.record(chained).await
    }

    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError> {
        self.inner.get_by_action_id(action_id).await
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
        self.inner.get_by_id(id).await
    }

    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        self.inner.query(query).await
    }

    async fn cleanup_expired(&self) -> Result<u64, AuditError> {
        self.inner.cleanup_expired().await
    }
}

/// An audit store decorator that enforces compliance rules.
///
/// When `immutable_audit` is enabled in the [`ComplianceConfig`], this decorator
/// blocks `cleanup_expired()` calls, preventing any record deletion.
///
/// This should be the outermost decorator in the wrapping chain.
pub struct ComplianceAuditStore {
    inner: Arc<dyn AuditStore>,
    config: ComplianceConfig,
}

impl ComplianceAuditStore {
    /// Create a new `ComplianceAuditStore` wrapping the given inner store.
    pub fn new(inner: Arc<dyn AuditStore>, config: ComplianceConfig) -> Self {
        Self { inner, config }
    }

    /// Returns the compliance configuration.
    pub fn config(&self) -> &ComplianceConfig {
        &self.config
    }
}

#[async_trait]
impl AuditStore for ComplianceAuditStore {
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
        self.inner.record(entry).await
    }

    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError> {
        self.inner.get_by_action_id(action_id).await
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
        self.inner.get_by_id(id).await
    }

    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        self.inner.query(query).await
    }

    async fn cleanup_expired(&self) -> Result<u64, AuditError> {
        if self.config.immutable_audit {
            warn!("cleanup_expired blocked: immutable audit is enabled");
            return Ok(0);
        }
        self.inner.cleanup_expired().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{AuditPage, AuditQuery, AuditRecord};
    use serde_json::json;
    use std::sync::Mutex as StdMutex;

    /// In-memory audit store for testing.
    struct MemoryAudit {
        records: StdMutex<Vec<AuditRecord>>,
        cleanup_count: StdMutex<u64>,
    }

    impl MemoryAudit {
        fn new() -> Self {
            Self {
                records: StdMutex::new(Vec::new()),
                cleanup_count: StdMutex::new(0),
            }
        }

        fn raw_records(&self) -> Vec<AuditRecord> {
            self.records.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AuditStore for MemoryAudit {
        async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
            self.records.lock().unwrap().push(entry);
            Ok(())
        }

        async fn get_by_action_id(
            &self,
            action_id: &str,
        ) -> Result<Option<AuditRecord>, AuditError> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.action_id == action_id)
                .cloned())
        }

        async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.id == id)
                .cloned())
        }

        async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
            let all = self.records.lock().unwrap().clone();
            let mut filtered: Vec<AuditRecord> = all
                .into_iter()
                .filter(|r| {
                    query
                        .namespace
                        .as_ref()
                        .map_or(true, |ns| r.namespace == *ns)
                        && query.tenant.as_ref().map_or(true, |t| r.tenant == *t)
                })
                .collect();

            // Sort by sequence number ascending for chain verification.
            filtered.sort_by_key(|r| r.sequence_number.unwrap_or(0));

            let total = filtered.len() as u64;
            let offset = query.effective_offset() as usize;
            let limit = query.effective_limit() as usize;
            let records: Vec<AuditRecord> = filtered.into_iter().skip(offset).take(limit).collect();

            Ok(AuditPage {
                records,
                total,
                limit: limit as u32,
                offset: offset as u32,
            })
        }

        async fn cleanup_expired(&self) -> Result<u64, AuditError> {
            let mut count = self.cleanup_count.lock().unwrap();
            *count += 1;
            Ok(*count)
        }
    }

    fn make_record(id: &str, ns: &str, tenant: &str) -> AuditRecord {
        let now = chrono::Utc::now();
        AuditRecord {
            id: id.to_string(),
            action_id: format!("action-{id}"),
            chain_id: None,
            namespace: ns.to_string(),
            tenant: tenant.to_string(),
            provider: "webhook".to_string(),
            action_type: "test".to_string(),
            verdict: "allow".to_string(),
            matched_rule: None,
            outcome: "executed".to_string(),
            action_payload: Some(json!({"data": "value"})),
            verdict_details: json!({}),
            outcome_details: json!({}),
            metadata: json!({}),
            dispatched_at: now,
            completed_at: now,
            duration_ms: 10,
            expires_at: None,
            caller_id: String::new(),
            auth_method: String::new(),
            record_hash: None,
            previous_hash: None,
            sequence_number: None,
        }
    }

    // ---- HashChainAuditStore tests ----

    #[tokio::test]
    async fn hash_chain_first_record_no_previous() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();

        let records = inner.raw_records();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert!(r.previous_hash.is_none(), "first record has no previous");
        assert!(r.record_hash.is_some(), "first record should have a hash");
        assert_eq!(r.sequence_number, Some(0));
    }

    #[tokio::test]
    async fn hash_chain_links_records() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();
        store.record(make_record("r2", "ns", "t1")).await.unwrap();
        store.record(make_record("r3", "ns", "t1")).await.unwrap();

        let records = inner.raw_records();
        assert_eq!(records.len(), 3);

        // First record: no previous.
        assert!(records[0].previous_hash.is_none());
        assert_eq!(records[0].sequence_number, Some(0));

        // Second record: previous_hash = first record_hash.
        assert_eq!(records[1].previous_hash, records[0].record_hash);
        assert_eq!(records[1].sequence_number, Some(1));

        // Third record: previous_hash = second record_hash.
        assert_eq!(records[2].previous_hash, records[1].record_hash);
        assert_eq!(records[2].sequence_number, Some(2));

        // All hashes are unique.
        let hashes: Vec<_> = records
            .iter()
            .filter_map(|r| r.record_hash.as_ref())
            .collect();
        assert_eq!(hashes.len(), 3);
        assert_ne!(hashes[0], hashes[1]);
        assert_ne!(hashes[1], hashes[2]);
    }

    #[tokio::test]
    async fn hash_chain_separate_tenants() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        store
            .record(make_record("a1", "ns", "tenant-a"))
            .await
            .unwrap();
        store
            .record(make_record("b1", "ns", "tenant-b"))
            .await
            .unwrap();
        store
            .record(make_record("a2", "ns", "tenant-a"))
            .await
            .unwrap();

        let records = inner.raw_records();

        // tenant-a: r1 -> seq 0, r3 -> seq 1 with previous = r1 hash.
        assert_eq!(records[0].sequence_number, Some(0));
        assert!(records[0].previous_hash.is_none());
        assert_eq!(records[2].sequence_number, Some(1));
        assert_eq!(records[2].previous_hash, records[0].record_hash);

        // tenant-b: r2 -> seq 0 with no previous.
        assert_eq!(records[1].sequence_number, Some(0));
        assert!(records[1].previous_hash.is_none());
    }

    #[tokio::test]
    async fn hash_chain_deterministic_hashing() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();

        let records = inner.raw_records();
        let stored = &records[0];

        // Re-compute the hash independently and verify.
        let expected = HashChainAuditStore::compute_record_hash(stored);
        assert_eq!(stored.record_hash.as_ref().unwrap(), &expected);
    }

    #[tokio::test]
    async fn verify_chain_valid() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();
        store.record(make_record("r2", "ns", "t1")).await.unwrap();
        store.record(make_record("r3", "ns", "t1")).await.unwrap();

        let result = store.verify_chain("ns", "t1", None, None).await.unwrap();
        assert!(result.valid);
        assert_eq!(result.records_checked, 3);
        assert!(result.first_broken_at.is_none());
        assert_eq!(result.first_record_id.as_deref(), Some("r1"));
        assert_eq!(result.last_record_id.as_deref(), Some("r3"));
    }

    #[tokio::test]
    async fn verify_chain_empty() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        let result = store.verify_chain("ns", "t1", None, None).await.unwrap();
        assert!(result.valid);
        assert_eq!(result.records_checked, 0);
    }

    #[tokio::test]
    async fn verify_chain_detects_tampered_hash() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();
        store.record(make_record("r2", "ns", "t1")).await.unwrap();

        // Tamper with the second record's hash.
        {
            let mut records = inner.records.lock().unwrap();
            records[1].record_hash = Some("tampered_hash_value".to_string());
        }

        let result = store.verify_chain("ns", "t1", None, None).await.unwrap();
        assert!(!result.valid);
        assert_eq!(result.first_broken_at.as_deref(), Some("r2"));
    }

    #[tokio::test]
    async fn verify_chain_detects_broken_link() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();
        store.record(make_record("r2", "ns", "t1")).await.unwrap();

        // Break the link by changing previous_hash on the second record.
        {
            let mut records = inner.records.lock().unwrap();
            records[1].previous_hash = Some("wrong_previous_hash".to_string());
        }

        let result = store.verify_chain("ns", "t1", None, None).await.unwrap();
        assert!(!result.valid);
        assert_eq!(result.first_broken_at.as_deref(), Some("r2"));
    }

    #[tokio::test]
    async fn hash_chain_passthrough_reads() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();

        // Read methods should pass through to inner store.
        let by_id = store.get_by_id("r1").await.unwrap();
        assert!(by_id.is_some());

        let by_action = store.get_by_action_id("action-r1").await.unwrap();
        assert!(by_action.is_some());

        let page = store
            .query(&AuditQuery {
                limit: Some(10),
                ..AuditQuery::default()
            })
            .await
            .unwrap();
        assert_eq!(page.records.len(), 1);
    }

    #[tokio::test]
    async fn hash_chain_cleanup_passthrough() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        let count = store.cleanup_expired().await.unwrap();
        assert_eq!(count, 1);
    }

    // ---- ComplianceAuditStore tests ----

    #[tokio::test]
    async fn compliance_immutable_blocks_cleanup() {
        let inner = Arc::new(MemoryAudit::new());
        let config = ComplianceConfig::new(acteon_core::compliance::ComplianceMode::Hipaa);
        let store = ComplianceAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, config);

        // Immutable audit should block cleanup.
        let count = store.cleanup_expired().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn compliance_non_immutable_allows_cleanup() {
        let inner = Arc::new(MemoryAudit::new());
        let config = ComplianceConfig::new(acteon_core::compliance::ComplianceMode::Soc2);
        let store = ComplianceAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, config);

        // SOC2 does not set immutable_audit, so cleanup should pass through.
        let count = store.cleanup_expired().await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn compliance_none_allows_cleanup() {
        let inner = Arc::new(MemoryAudit::new());
        let config = ComplianceConfig::new(acteon_core::compliance::ComplianceMode::None);
        let store = ComplianceAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, config);

        let count = store.cleanup_expired().await.unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn compliance_passthrough_record() {
        let inner = Arc::new(MemoryAudit::new());
        let config = ComplianceConfig::new(acteon_core::compliance::ComplianceMode::Hipaa);
        let store = ComplianceAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, config);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();
        assert_eq!(inner.raw_records().len(), 1);
    }

    #[tokio::test]
    async fn compliance_passthrough_reads() {
        let inner = Arc::new(MemoryAudit::new());
        let config = ComplianceConfig::new(acteon_core::compliance::ComplianceMode::Hipaa);
        let store = ComplianceAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, config);

        // Insert directly into inner.
        inner.record(make_record("r1", "ns", "t1")).await.unwrap();

        let by_id = store.get_by_id("r1").await.unwrap();
        assert!(by_id.is_some());

        let by_action = store.get_by_action_id("action-r1").await.unwrap();
        assert!(by_action.is_some());
    }

    #[tokio::test]
    async fn compliance_config_accessor() {
        let config = ComplianceConfig::new(acteon_core::compliance::ComplianceMode::Soc2);
        let inner = Arc::new(MemoryAudit::new());
        let store = ComplianceAuditStore::new(inner as Arc<dyn AuditStore>, config.clone());

        assert_eq!(
            store.config().mode,
            acteon_core::compliance::ComplianceMode::Soc2
        );
        assert!(store.config().sync_audit_writes);
        assert!(!store.config().immutable_audit);
        assert!(store.config().hash_chain);
    }

    #[tokio::test]
    async fn hash_chain_separate_namespaces() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        // Same tenant, different namespaces should have independent chains.
        store
            .record(make_record("a1", "ns-alpha", "t1"))
            .await
            .unwrap();
        store
            .record(make_record("b1", "ns-beta", "t1"))
            .await
            .unwrap();
        store
            .record(make_record("a2", "ns-alpha", "t1"))
            .await
            .unwrap();

        let records = inner.raw_records();

        // ns-alpha: a1 -> seq 0, a2 -> seq 1 linked to a1.
        assert_eq!(records[0].sequence_number, Some(0));
        assert!(records[0].previous_hash.is_none());
        assert_eq!(records[2].sequence_number, Some(1));
        assert_eq!(records[2].previous_hash, records[0].record_hash);

        // ns-beta: b1 -> seq 0, independent.
        assert_eq!(records[1].sequence_number, Some(0));
        assert!(records[1].previous_hash.is_none());
    }

    #[tokio::test]
    async fn verify_chain_single_record() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();

        let result = store.verify_chain("ns", "t1", None, None).await.unwrap();
        assert!(result.valid);
        assert_eq!(result.records_checked, 1);
        assert_eq!(result.first_record_id.as_deref(), Some("r1"));
        assert_eq!(result.last_record_id.as_deref(), Some("r1"));
    }

    #[tokio::test]
    async fn verify_chain_detects_missing_hash() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();
        store.record(make_record("r2", "ns", "t1")).await.unwrap();

        // Remove the hash from the second record entirely.
        {
            let mut records = inner.records.lock().unwrap();
            records[1].record_hash = None;
        }

        let result = store.verify_chain("ns", "t1", None, None).await.unwrap();
        assert!(!result.valid);
        assert_eq!(result.first_broken_at.as_deref(), Some("r2"));
    }

    #[tokio::test]
    async fn verify_chain_cross_tenant_isolation() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        // Build valid chains for two tenants.
        store.record(make_record("a1", "ns", "t-a")).await.unwrap();
        store.record(make_record("b1", "ns", "t-b")).await.unwrap();
        store.record(make_record("a2", "ns", "t-a")).await.unwrap();

        // Tamper with tenant-b's record.
        {
            let mut records = inner.records.lock().unwrap();
            records[1].record_hash = Some("corrupted".to_string());
        }

        // Tenant-a's chain should still be valid.
        let result_a = store.verify_chain("ns", "t-a", None, None).await.unwrap();
        assert!(result_a.valid);
        assert_eq!(result_a.records_checked, 2);

        // Tenant-b's chain should be broken.
        let result_b = store.verify_chain("ns", "t-b", None, None).await.unwrap();
        assert!(!result_b.valid);
    }

    #[tokio::test]
    async fn hash_chain_many_records_sequence_monotonic() {
        let inner = Arc::new(MemoryAudit::new());
        let store = HashChainAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>);

        for i in 0..20 {
            store
                .record(make_record(&format!("r{i}"), "ns", "t1"))
                .await
                .unwrap();
        }

        let records = inner.raw_records();
        assert_eq!(records.len(), 20);

        for (i, record) in records.iter().enumerate() {
            assert_eq!(record.sequence_number, Some(i as u64));
            if i == 0 {
                assert!(record.previous_hash.is_none());
            } else {
                assert_eq!(record.previous_hash, records[i - 1].record_hash);
            }
        }

        // Full chain verification should pass.
        let result = store.verify_chain("ns", "t1", None, None).await.unwrap();
        assert!(result.valid);
        assert_eq!(result.records_checked, 20);
    }

    #[tokio::test]
    async fn compliance_custom_override_immutable_on_soc2() {
        let inner = Arc::new(MemoryAudit::new());
        let config = ComplianceConfig::new(acteon_core::compliance::ComplianceMode::Soc2)
            .with_immutable_audit(true);
        let store = ComplianceAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, config);

        // SOC2 + immutable override should block cleanup.
        let count = store.cleanup_expired().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn compliance_custom_override_mutable_on_hipaa() {
        let inner = Arc::new(MemoryAudit::new());
        let config = ComplianceConfig::new(acteon_core::compliance::ComplianceMode::Hipaa)
            .with_immutable_audit(false);
        let store = ComplianceAuditStore::new(Arc::clone(&inner) as Arc<dyn AuditStore>, config);

        // HIPAA with immutable overridden to false should allow cleanup.
        let count = store.cleanup_expired().await.unwrap();
        assert_eq!(count, 1);
    }

    // ---- Combined decorator stack tests ----

    #[tokio::test]
    async fn full_decorator_stack_hash_then_compliance() {
        let inner = Arc::new(MemoryAudit::new());
        let hash_store = Arc::new(HashChainAuditStore::new(
            Arc::clone(&inner) as Arc<dyn AuditStore>
        ));
        let config = ComplianceConfig::new(acteon_core::compliance::ComplianceMode::Hipaa);
        let store = ComplianceAuditStore::new(hash_store as Arc<dyn AuditStore>, config);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();
        store.record(make_record("r2", "ns", "t1")).await.unwrap();

        let records = inner.raw_records();
        assert_eq!(records.len(), 2);

        // Hash chain should be intact.
        assert!(records[0].record_hash.is_some());
        assert!(records[0].previous_hash.is_none());
        assert_eq!(records[0].sequence_number, Some(0));

        assert_eq!(records[1].previous_hash, records[0].record_hash);
        assert_eq!(records[1].sequence_number, Some(1));

        // Cleanup should be blocked by compliance.
        let count = store.cleanup_expired().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn full_decorator_stack_soc2_allows_cleanup() {
        let inner = Arc::new(MemoryAudit::new());
        let hash_store = Arc::new(HashChainAuditStore::new(
            Arc::clone(&inner) as Arc<dyn AuditStore>
        ));
        let config = ComplianceConfig::new(acteon_core::compliance::ComplianceMode::Soc2);
        let store = ComplianceAuditStore::new(hash_store as Arc<dyn AuditStore>, config);

        store.record(make_record("r1", "ns", "t1")).await.unwrap();

        // Hash chain should still work.
        let records = inner.raw_records();
        assert!(records[0].record_hash.is_some());
        assert_eq!(records[0].sequence_number, Some(0));

        // SOC2 does not set immutable_audit, so cleanup should pass through.
        let count = store.cleanup_expired().await.unwrap();
        assert_eq!(count, 1);
    }
}
