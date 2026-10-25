use crate::{
    ChangeDisposition, ChangeOperation, ChangeReceipt, Error, Result, TimelineChange,
    TimelineEvent, TimelineScope, TimelineStore,
    config::TimelineConfig,
    ledger::{ChangeLedger, LedgerArchiveSink, LedgerEntry},
    store::{Projection, identity},
};
use acteon_state::{CasResult, KeyKind, StateKey, StateStore};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
struct Head {
    revision: u64,
    digest: String,
    first_ingested_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    terminal: bool,
    pending: Option<LedgerEntry>,
}
#[derive(Serialize, Deserialize)]
struct PendingCandidate {
    head: Head,
    expected_version: u64,
}
pub struct TimelineService {
    pub metrics: crate::metrics::TimelineMetrics,
    pub store: Arc<dyn TimelineStore>,
    pub ledger: Arc<dyn ChangeLedger>,
    pub state: Arc<dyn StateStore>,
    pub config: TimelineConfig,
    archive: Option<Arc<dyn LedgerArchiveSink>>,
}
impl TimelineService {
    pub fn new(
        store: Arc<dyn TimelineStore>,
        ledger: Arc<dyn ChangeLedger>,
        state: Arc<dyn StateStore>,
        config: TimelineConfig,
        archive: Option<Arc<dyn LedgerArchiveSink>>,
    ) -> Result<Self> {
        for ttl in [
            config.ttl_seconds,
            config.ledger.ttl_seconds,
            config
                .archive
                .as_ref()
                .map_or(crate::DEFAULT_ARCHIVE_TTL_SECONDS, |a| a.ttl_seconds),
        ] {
            if ttl == 0 || ttl > 315_576_000 {
                return Err(Error::Invalid(
                    "retention must be between one second and ten years".into(),
                ));
            }
        }
        if config.archive.is_some() != archive.is_some() {
            return Err(Error::Invalid(
                "archive configuration and sink must agree".into(),
            ));
        }
        Ok(Self {
            metrics: crate::metrics::TimelineMetrics::default(),
            store,
            ledger,
            state,
            config,
            archive,
        })
    }
    pub async fn ingest(
        &self,
        scope: &TimelineScope,
        change: TimelineChange,
    ) -> Result<ChangeReceipt> {
        let raw = serde_json::to_value(&change)?;
        self.ingest_raw(scope, change, raw, None).await
    }
    /// Shared REST/transport ingestion boundary. Transport metadata is supplied by the adapter.
    pub async fn ingest_raw(
        &self,
        scope: &TimelineScope,
        change: TimelineChange,
        raw: serde_json::Value,
        transport: Option<serde_json::Value>,
    ) -> Result<ChangeReceipt> {
        use std::sync::atomic::Ordering;
        let start = std::time::Instant::now();
        let result = self
            .ingest_inner(scope, change, raw, transport, false)
            .await;
        self.metrics.ingest_micros.fetch_add(
            u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        match &result {
            Ok(receipt) => {
                let counter = match receipt.disposition {
                    ChangeDisposition::Applied => &self.metrics.accepted,
                    ChangeDisposition::Quarantined => &self.metrics.quarantined,
                    ChangeDisposition::Duplicate => &self.metrics.duplicates,
                    ChangeDisposition::Stale | ChangeDisposition::Terminal => &self.metrics.stale,
                };
                counter.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.metrics.failures.fetch_add(1, Ordering::Relaxed);
            }
        }
        result
    }
    pub async fn query(
        &self,
        scope: &TimelineScope,
        query: &crate::TimelineQuery,
    ) -> Result<crate::TimelinePage> {
        use std::sync::atomic::Ordering;
        let start = std::time::Instant::now();
        let result = self.store.query(scope, query).await;
        self.metrics.query_count.fetch_add(1, Ordering::Relaxed);
        self.metrics.query_micros.fetch_add(
            u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        result
    }
    #[allow(clippy::too_many_lines)]
    async fn ingest_inner(
        &self,
        scope: &TimelineScope,
        change: TimelineChange,
        raw: serde_json::Value,
        transport: Option<serde_json::Value>,
        rejection: bool,
    ) -> Result<ChangeReceipt> {
        crate::search::validate_scope(scope)?;
        if !rejection {
            crate::search::validate_change(&change)?;
        }
        if serde_json::to_vec(&raw)?.len() > 512 * 1024 {
            return Err(Error::Invalid("raw envelope exceeds 512 KiB".into()));
        }
        let key = Self::key(scope, &change.source, &change.event_id);
        let content_hash = crate::digest(&change);
        for _ in 0..64 {
            let old = self.state.get_versioned(&key).await?;
            let head = old
                .as_ref()
                .map(|(s, _)| serde_json::from_str::<Head>(s))
                .transpose()?;
            if let Some(h) = &head
                && h.pending.is_some()
            {
                self.finish(&key, h.clone(), old.as_ref().expect("existing head").1)
                    .await?;
                continue;
            }
            let now = Utc::now();
            let disposition = if let Some(h) = &head {
                if change.revision == h.revision {
                    if content_hash != h.digest {
                        return Err(Error::Conflict);
                    }
                    return Ok(ChangeReceipt {
                        change_id: change.change_id,
                        revision: change.revision,
                        disposition: ChangeDisposition::Duplicate,
                    });
                } else if change.revision < h.revision {
                    ChangeDisposition::Stale
                } else if h.terminal || h.expires_at <= now {
                    ChangeDisposition::Terminal
                } else {
                    ChangeDisposition::Applied
                }
            } else {
                ChangeDisposition::Applied
            };
            let disposition = if rejection && disposition == ChangeDisposition::Applied {
                ChangeDisposition::Quarantined
            } else {
                disposition
            };
            let (timeline_ttl, ledger_ttl, archive_ttl) = self.retention(scope).await?;
            let first = head.as_ref().map_or(now, |h| h.first_ingested_at);
            let expires_at = head
                .as_ref()
                .map_or(add_seconds(first, timeline_ttl)?, |h| h.expires_at);
            let event = change.after.clone().map(|event| TimelineEvent {
                scope: scope.clone(),
                source: change.source.clone(),
                event_id: change.event_id.clone(),
                revision: change.revision,
                first_ingested_at: first,
                updated_at: now,
                expires_at,
                event,
            });
            let projection = (disposition == ChangeDisposition::Applied).then(|| Projection {
                scope: scope.clone(),
                source: change.source.clone(),
                event_id: change.event_id.clone(),
                revision: change.revision,
                expires_at,
                event,
            });
            let entry = LedgerEntry {
                id: uuid::Uuid::new_v4().to_string(),
                scope: scope.clone(),
                received_at: now,
                retain_until: add_seconds(now, ledger_ttl)?,
                archive_until: add_seconds(now, archive_ttl)?,
                change: change.clone(),
                raw: raw.clone(),
                transport: transport.clone(),
                disposition,
                projection,
            };
            if serde_json::to_vec(&entry)?.len() > 2 * 1024 * 1024 {
                return Err(Error::Invalid(
                    "CDC record including transport metadata exceeds 2 MiB".into(),
                ));
            }
            let mut next = head.unwrap_or(Head {
                revision: 0,
                digest: String::new(),
                first_ingested_at: first,
                expires_at,
                terminal: false,
                pending: None,
            });
            if matches!(
                disposition,
                ChangeDisposition::Applied | ChangeDisposition::Quarantined
            ) {
                next.revision = change.revision;
                next.digest.clone_from(&content_hash);
                next.terminal = matches!(
                    change.operation,
                    ChangeOperation::Delete | ChangeOperation::Rejected
                );
            }
            let pending_key = Self::pending_key(&entry.scope, &entry.id);
            next.pending = Some(entry);
            // Persist discoverable work before admission. Recovery can complete an
            // interrupted reservation; CAS prevents a losing candidate changing the head.
            self.state
                .set(
                    &pending_key,
                    &serde_json::to_string(&PendingCandidate {
                        head: next.clone(),
                        expected_version: old.as_ref().map_or(0, |(_, version)| *version),
                    })?,
                    None,
                )
                .await?;
            let value = serde_json::to_string(&next)?;
            let reserved = if let Some((_, version)) = old {
                matches!(
                    self.state
                        .compare_and_swap(&key, version, &value, None)
                        .await?,
                    CasResult::Ok
                )
            } else {
                self.state.check_and_set(&key, &value, None).await?
            };
            if !reserved {
                continue;
            }
            // Re-read the persisted version; helping writers may have already completed it.
            if let Some((value, version)) = self.state.get_versioned(&key).await? {
                self.finish(&key, serde_json::from_str(&value)?, version)
                    .await?;
            }
            return Ok(ChangeReceipt {
                change_id: change.change_id,
                revision: change.revision,
                disposition,
            });
        }
        Err(Error::Capacity)
    }
    /// Retain rejected JSON through the same recoverable persistence boundary. No projection is produced.
    pub async fn quarantine(
        &self,
        scope: &TimelineScope,
        source: &str,
        raw: serde_json::Value,
        transport: serde_json::Value,
    ) -> Result<()> {
        let change = TimelineChange {
            schema_version: 1,
            change_id: crate::digest(&raw),
            source: "_acteon_quarantine".into(),
            event_id: crate::digest(&(source, &transport)),
            revision: 1,
            operation: ChangeOperation::Rejected,
            before: None,
            after: None,
        };
        self.ingest_inner(scope, change, raw, Some(transport), true)
            .await?;
        self.metrics
            .quarantined
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    async fn finish(&self, key: &StateKey, mut head: Head, version: u64) -> Result<()> {
        let Some(entry) = head.pending.take() else {
            return Ok(());
        };
        self.ledger.append(&entry).await?;
        if let Some(archive) = &self.archive {
            archive.publish(&entry).await?;
        }
        if let Some(projection) = &entry.projection {
            self.store.apply(projection).await?;
        }
        self.ledger.complete(&entry.scope, &entry.id).await?;
        self.state
            .compare_and_swap(key, version, &serde_json::to_string(&head)?, None)
            .await?;
        self.state
            .delete(&Self::pending_key(&entry.scope, &entry.id))
            .await?;
        Ok(())
    }
    fn key(scope: &TimelineScope, source: &str, event_id: &str) -> StateKey {
        StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::Custom("timeline_head".into()),
            identity(scope, source, event_id),
        )
    }
    fn pending_key(scope: &TimelineScope, id: &str) -> StateKey {
        StateKey::new(
            scope.namespace.clone(),
            scope.tenant.clone(),
            KeyKind::Custom("timeline_pending".into()),
            id,
        )
    }
    async fn recover_candidate(&self, candidate: PendingCandidate) -> Result<bool> {
        let entry = candidate
            .head
            .pending
            .as_ref()
            .ok_or_else(|| Error::Storage("pending candidate without change".into()))?;
        let key = Self::key(&entry.scope, &entry.change.source, &entry.change.event_id);
        let current = self.state.get_versioned(&key).await?;
        let current_version = current.as_ref().map_or(0, |(_, version)| *version);
        if current_version == candidate.expected_version {
            let value = serde_json::to_string(&candidate.head)?;
            if current_version == 0 {
                self.state.check_and_set(&key, &value, None).await?;
            } else {
                self.state
                    .compare_and_swap(&key, current_version, &value, None)
                    .await?;
            }
        }
        if let Some((value, version)) = self.state.get_versioned(&key).await? {
            let head: Head = serde_json::from_str(&value)?;
            if head
                .pending
                .as_ref()
                .is_some_and(|pending| pending.id == entry.id)
            {
                self.finish(&key, head, version).await?;
                return Ok(true);
            }
            // A newer state version permanently fences this candidate's reservation.
            if version > candidate.expected_version {
                self.state
                    .delete(&Self::pending_key(&entry.scope, &entry.id))
                    .await?;
            }
        }
        Ok(false)
    }
    /// Resume pending work without scanning the retained identity/version fences.
    pub async fn recover(&self, scope: &TimelineScope) -> Result<u64> {
        let mut cursor = None;
        let mut count = 0;
        loop {
            let page = self
                .state
                .scan_keys_page(
                    &scope.namespace,
                    &scope.tenant,
                    KeyKind::Custom("timeline_pending".into()),
                    None,
                    cursor.as_deref(),
                    100,
                )
                .await?;
            for (_, value) in page.entries {
                count += u64::from(
                    self.recover_candidate(serde_json::from_str(&value)?)
                        .await?,
                );
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(count);
            }
        }
    }
    pub async fn cleanup_archive(&self) -> Result<u64> {
        if let Some(archive) = &self.archive {
            archive.cleanup(Utc::now()).await
        } else {
            Ok(0)
        }
    }
    pub async fn recover_all(&self) -> Result<u64> {
        let mut cursor = None;
        let mut count = 0;
        loop {
            let page = self
                .state
                .scan_keys_by_kind_page(
                    KeyKind::Custom("timeline_pending".into()),
                    cursor.as_deref(),
                    100,
                )
                .await?;
            for (_, value) in page.entries {
                count += u64::from(
                    self.recover_candidate(serde_json::from_str(&value)?)
                        .await?,
                );
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(count);
            }
        }
    }
    /// Reapply retained records to a projection. Existing durable heads fence deleted/expired identities.
    pub async fn replay_local(&self, scope: &TimelineScope) -> Result<u64> {
        self.recover(scope).await?;
        let mut cursor = None;
        let mut count = 0;
        loop {
            let entries = self.ledger.read(scope, cursor.as_deref(), 100).await?;
            if entries.is_empty() {
                break;
            }
            for entry in entries {
                cursor = Some(crate::digest(&entry.id));
                if let Some(projection) = entry.projection {
                    let key = Self::key(scope, &projection.source, &projection.event_id);
                    if let Some(value) = self.state.get(&key).await? {
                        let head: Head = serde_json::from_str(&value)?;
                        if projection.revision == head.revision {
                            let mut projection = projection;
                            if head.terminal || head.expires_at <= Utc::now() {
                                projection.event = None;
                            }
                            self.store.apply(&projection).await?;
                            count += 1;
                        }
                    }
                }
            }
        }
        Ok(count)
    }
    async fn retention(&self, scope: &TimelineScope) -> Result<(u64, u64, u64)> {
        let defaults = (
            self.config.ttl_seconds,
            self.config.ledger.ttl_seconds,
            self.config
                .archive
                .as_ref()
                .map_or(crate::DEFAULT_ARCHIVE_TTL_SECONDS, |a| a.ttl_seconds),
        );
        let index = StateKey::new(
            "_system",
            "_retention",
            KeyKind::Retention,
            format!("idx:{}:{}", scope.namespace, scope.tenant),
        );
        let Some(id) = self.state.get(&index).await? else {
            return Ok(defaults);
        };
        let key = StateKey::new("_system", "_retention", KeyKind::Retention, id);
        let Some(value) = self.state.get(&key).await? else {
            return Ok(defaults);
        };
        let policy: acteon_core::RetentionPolicy = serde_json::from_str(&value)?;
        if !policy.enabled || policy.namespace != scope.namespace || policy.tenant != scope.tenant {
            return Ok(defaults);
        }
        Ok((
            policy.timeline_ttl_seconds.unwrap_or(defaults.0),
            policy.cdc_ledger_ttl_seconds.unwrap_or(defaults.1),
            policy.cdc_archive_ttl_seconds.unwrap_or(defaults.2),
        ))
    }
}
fn add_seconds(time: DateTime<Utc>, seconds: u64) -> Result<DateTime<Utc>> {
    if seconds == 0 || seconds > 315_576_000 {
        return Err(Error::Invalid(
            "retention must be between one second and ten years".into(),
        ));
    }
    time.checked_add_signed(chrono::Duration::seconds(
        i64::try_from(seconds).map_err(|_| Error::Invalid("retention overflow".into()))?,
    ))
    .ok_or_else(|| Error::Invalid("retention overflow".into()))
}
