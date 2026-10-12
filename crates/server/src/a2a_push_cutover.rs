//! Reviewed state-backend cutover from standalone A2A callback rows to the
//! durable Task envelope.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use acteon_core::{
    MAX_DLQ_ERROR_BYTES, MAX_DLQ_EVENT_BYTES, PushDeliveryDlqEntry, TaskPushNotificationConfig,
};
use acteon_gateway::{TaskEngine, TaskEngineError, TaskScope};
use acteon_state::{KeyKind, StateError, StateKey, StateStore};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CUTOVER_VERSION: u32 = 1;
const CUTOVER_ID: &str = "a2a-push-task-envelope-v1";
const SYSTEM_NAMESPACE: &str = "_system";
const SYSTEM_TENANT: &str = "_migrations";

#[derive(Debug, thiserror::Error)]
pub enum A2aPushCutoverError {
    #[error("invalid A2A push cutover: {0}")]
    Invalid(String),
    #[error("A2A push cutover inventory changed; generate and review a new preview")]
    InventoryChanged,
    #[error("post-cutover legacy A2A callback rows detected")]
    PostCutoverLegacyRows,
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Task(#[from] TaskEngineError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct TaskIdentity {
    namespace: String,
    tenant: String,
    task_id: String,
}

impl TaskIdentity {
    fn scope(&self) -> TaskScope {
        TaskScope::new(&self.namespace, &self.tenant)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct A2aPushCutoverEvidence {
    pub kind: String,
    pub key: String,
    pub content_digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct A2aPushCutoverReport {
    pub protocol_version: u32,
    pub actor: String,
    pub reason: String,
    pub task_count: usize,
    pub legacy_config_count: usize,
    pub legacy_dead_letter_count: usize,
    pub evidence: Vec<A2aPushCutoverEvidence>,
    pub inventory_digest: String,
    pub review_digest: String,
}

#[derive(Debug, Clone)]
struct LegacyConfigRow {
    key: StateKey,
    version: u64,
    raw: String,
    config: TaskPushNotificationConfig,
}

#[derive(Debug, Clone)]
struct LegacyDlqRow {
    key: StateKey,
    version: u64,
    raw: String,
    entry: PushDeliveryDlqEntry,
}

#[derive(Debug)]
struct Inventory {
    tasks: Vec<TaskIdentity>,
    configs: Vec<LegacyConfigRow>,
    dead_letters: Vec<LegacyDlqRow>,
    evidence: Vec<A2aPushCutoverEvidence>,
    digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CutoverMarker {
    protocol_version: u32,
    review_digest: String,
    inventory_digest: String,
    actor: String,
    reason: String,
    completed_at: DateTime<Utc>,
}

/// A preview whose digest binds operator intent to an exact backend inventory.
pub struct A2aPushCutoverPlan {
    state: Arc<dyn StateStore>,
    report: A2aPushCutoverReport,
}

impl A2aPushCutoverPlan {
    pub async fn preview(
        state: Arc<dyn StateStore>,
        actor: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, A2aPushCutoverError> {
        let actor = actor.into();
        let reason = reason.into();
        validate_operator_text("actor", &actor, 256)?;
        validate_operator_text("reason", &reason, 2_048)?;
        let inventory = inventory(state.clone()).await?;
        let review_digest = digest_json(&(
            "acteon.a2a-push-cutover.review.v1",
            CUTOVER_VERSION,
            &actor,
            &reason,
            &inventory.digest,
        ))?;
        let report = A2aPushCutoverReport {
            protocol_version: CUTOVER_VERSION,
            actor,
            reason,
            task_count: inventory.tasks.len(),
            legacy_config_count: inventory.configs.len(),
            legacy_dead_letter_count: inventory.dead_letters.len(),
            evidence: inventory.evidence,
            inventory_digest: inventory.digest,
            review_digest,
        };
        Ok(Self { state, report })
    }

    #[must_use]
    pub fn report(&self) -> &A2aPushCutoverReport {
        &self.report
    }

    /// Apply the exact reviewed inventory. Returns `false` when a valid marker
    /// already exists and the backend contains no post-cutover legacy rows.
    pub async fn apply(&self, review_digest: &str) -> Result<bool, A2aPushCutoverError> {
        if review_digest != self.report.review_digest {
            return Err(A2aPushCutoverError::Invalid(
                "review digest does not match this preview".into(),
            ));
        }
        if cutover_marker(self.state.as_ref()).await?.is_some() {
            ensure_no_legacy_rows(self.state.as_ref()).await?;
            return Ok(false);
        }

        let inventory = inventory(self.state.clone()).await?;
        if inventory.digest != self.report.inventory_digest {
            return Err(A2aPushCutoverError::InventoryChanged);
        }

        let engine = TaskEngine::new(self.state.clone());
        let mut configs = BTreeMap::<TaskIdentity, Vec<LegacyConfigRow>>::new();
        for row in inventory.configs {
            configs
                .entry(task_identity_from_config(&row.config))
                .or_default()
                .push(row);
        }
        let mut dead_letters = BTreeMap::<TaskIdentity, Vec<LegacyDlqRow>>::new();
        for row in inventory.dead_letters {
            dead_letters
                .entry(task_identity_from_dlq(&row.entry))
                .or_default()
                .push(row);
        }

        for task in inventory.tasks {
            let config_rows = configs.remove(&task).unwrap_or_default();
            verify_config_rows(self.state.as_ref(), &config_rows).await?;
            engine
                .finalize_legacy_push_configs(
                    &task.scope(),
                    &task.task_id,
                    config_rows.iter().map(|row| row.config.clone()).collect(),
                )
                .await?;

            let dlq_rows = dead_letters.remove(&task).unwrap_or_default();
            verify_dlq_rows(self.state.as_ref(), &dlq_rows).await?;
            engine
                .import_legacy_push_dead_letters(
                    &task.scope(),
                    &task.task_id,
                    dlq_rows.iter().map(|row| row.entry.clone()).collect(),
                )
                .await?;

            delete_config_rows(self.state.as_ref(), &config_rows).await?;
            delete_dlq_rows(self.state.as_ref(), &dlq_rows).await?;
        }
        if !configs.is_empty() || !dead_letters.is_empty() {
            return Err(A2aPushCutoverError::Invalid(
                "legacy callback row references a missing A2A Task".into(),
            ));
        }
        ensure_no_legacy_rows(self.state.as_ref()).await?;

        let marker = CutoverMarker {
            protocol_version: CUTOVER_VERSION,
            review_digest: self.report.review_digest.clone(),
            inventory_digest: self.report.inventory_digest.clone(),
            actor: self.report.actor.clone(),
            reason: self.report.reason.clone(),
            completed_at: Utc::now(),
        };
        let created = self
            .state
            .check_and_set(&marker_key(), &serde_json::to_string(&marker)?, None)
            .await?;
        if !created {
            cutover_marker(self.state.as_ref())
                .await?
                .ok_or_else(|| A2aPushCutoverError::Invalid("cutover marker disappeared".into()))?;
        }
        // Close the scan-to-marker race. The marker deliberately remains if a
        // stopped legacy writer reappears, making every new binary fail visibly.
        ensure_no_legacy_rows(self.state.as_ref()).await?;
        Ok(created)
    }
}

/// Whether the backend has completed the reviewed A2A callback cutover.
pub async fn a2a_push_cutover_complete(
    state: &dyn StateStore,
) -> Result<bool, A2aPushCutoverError> {
    Ok(cutover_marker(state).await?.is_some())
}

/// Enforce the global post-cutover fence during process startup.
pub async fn verify_a2a_push_cutover_fence(
    state: &dyn StateStore,
) -> Result<(), A2aPushCutoverError> {
    if cutover_marker(state).await?.is_some() {
        ensure_no_legacy_rows(state).await?;
    }
    Ok(())
}

async fn cutover_marker(
    state: &dyn StateStore,
) -> Result<Option<CutoverMarker>, A2aPushCutoverError> {
    let Some(raw) = state.get(&marker_key()).await? else {
        return Ok(None);
    };
    let marker: CutoverMarker = serde_json::from_str(&raw)?;
    if marker.protocol_version != CUTOVER_VERSION
        || !valid_digest(&marker.review_digest)
        || !valid_digest(&marker.inventory_digest)
    {
        return Err(A2aPushCutoverError::Invalid(
            "malformed durable cutover marker".into(),
        ));
    }
    Ok(Some(marker))
}

fn marker_key() -> StateKey {
    StateKey::new(
        SYSTEM_NAMESPACE,
        SYSTEM_TENANT,
        KeyKind::Migration,
        CUTOVER_ID,
    )
}

async fn inventory(state: Arc<dyn StateStore>) -> Result<Inventory, A2aPushCutoverError> {
    let engine = TaskEngine::new(state.clone());
    let tasks = engine
        .list_all_task_refs()
        .await?
        .into_iter()
        .map(|(scope, task_id)| TaskIdentity {
            namespace: scope.namespace,
            tenant: scope.tenant,
            task_id,
        })
        .collect::<Vec<_>>();
    let task_set = tasks.iter().cloned().collect::<BTreeSet<_>>();
    let configs = inventory_configs(state.as_ref()).await?;
    let dead_letters = inventory_dlq(state.as_ref()).await?;
    for row in &configs {
        if !task_set.contains(&task_identity_from_config(&row.config)) {
            return Err(A2aPushCutoverError::Invalid(
                "legacy callback configuration references a missing Task".into(),
            ));
        }
    }
    for row in &dead_letters {
        if !task_set.contains(&task_identity_from_dlq(&row.entry)) {
            return Err(A2aPushCutoverError::Invalid(
                "legacy callback dead letter references a missing Task".into(),
            ));
        }
    }
    let mut evidence = configs
        .iter()
        .map(|row| A2aPushCutoverEvidence {
            kind: KeyKind::A2aTaskPushConfig.to_string(),
            key: row.key.canonical(),
            content_digest: digest_bytes(row.raw.as_bytes()),
        })
        .chain(dead_letters.iter().map(|row| A2aPushCutoverEvidence {
            kind: KeyKind::A2aPushDlq.to_string(),
            key: row.key.canonical(),
            content_digest: digest_bytes(row.raw.as_bytes()),
        }))
        .collect::<Vec<_>>();
    evidence.sort_by(|left, right| (&left.kind, &left.key).cmp(&(&right.kind, &right.key)));
    let digest = digest_json(&("acteon.a2a-push-cutover.inventory.v1", &tasks, &evidence))?;
    Ok(Inventory {
        tasks,
        configs,
        dead_letters,
        evidence,
        digest,
    })
}

async fn inventory_configs(
    state: &dyn StateStore,
) -> Result<Vec<LegacyConfigRow>, A2aPushCutoverError> {
    let scanned = scan_kind(state, KeyKind::A2aTaskPushConfig).await?;
    let mut rows = Vec::with_capacity(scanned.len());
    for (canonical, scanned_raw) in scanned {
        let config: TaskPushNotificationConfig = serde_json::from_str(&scanned_raw)?;
        config
            .validate()
            .map_err(|error| A2aPushCutoverError::Invalid(error.to_string()))?;
        let key = StateKey::new(
            config.namespace.as_str(),
            config.tenant.as_str(),
            KeyKind::A2aTaskPushConfig,
            format!("{}:{}", config.task_id, config.id),
        );
        if canonical != key.canonical() {
            return Err(A2aPushCutoverError::Invalid(
                "legacy callback configuration key/value identity mismatch".into(),
            ));
        }
        let Some((raw, version)) = state.get_versioned(&key).await? else {
            return Err(A2aPushCutoverError::InventoryChanged);
        };
        if raw != scanned_raw {
            return Err(A2aPushCutoverError::InventoryChanged);
        }
        rows.push(LegacyConfigRow {
            key,
            version,
            raw,
            config,
        });
    }
    rows.sort_by_key(|row| row.key.canonical());
    Ok(rows)
}

async fn inventory_dlq(state: &dyn StateStore) -> Result<Vec<LegacyDlqRow>, A2aPushCutoverError> {
    let scanned = scan_kind(state, KeyKind::A2aPushDlq).await?;
    let mut rows = Vec::with_capacity(scanned.len());
    let mut entry_ids = BTreeSet::new();
    for (canonical, scanned_raw) in scanned {
        let entry: PushDeliveryDlqEntry = serde_json::from_str(&scanned_raw)?;
        let key = StateKey::new(
            entry.namespace.as_str(),
            entry.tenant.as_str(),
            KeyKind::A2aPushDlq,
            entry.storage_id(),
        );
        if canonical != key.canonical()
            || entry.sequence.is_some()
            || entry.event_id.is_some()
            || uuid::Uuid::parse_str(&entry.id).is_err()
            || entry.config_id.is_empty()
            || entry.url.is_empty()
            || entry.attempts == 0
            || entry.first_failed_at > entry.last_failed_at
            || entry.last_error.len() > MAX_DLQ_ERROR_BYTES
            || entry.event_json.len() > MAX_DLQ_EVENT_BYTES
            || !entry_ids.insert(entry.id.clone())
        {
            return Err(A2aPushCutoverError::Invalid(
                "invalid or duplicate legacy callback dead letter".into(),
            ));
        }
        let Some((raw, version)) = state.get_versioned(&key).await? else {
            return Err(A2aPushCutoverError::InventoryChanged);
        };
        if raw != scanned_raw {
            return Err(A2aPushCutoverError::InventoryChanged);
        }
        rows.push(LegacyDlqRow {
            key,
            version,
            raw,
            entry,
        });
    }
    rows.sort_by_key(|row| row.key.canonical());
    Ok(rows)
}

async fn verify_config_rows(
    state: &dyn StateStore,
    rows: &[LegacyConfigRow],
) -> Result<(), A2aPushCutoverError> {
    for row in rows {
        let Some((raw, version)) = state.get_versioned(&row.key).await? else {
            return Err(A2aPushCutoverError::InventoryChanged);
        };
        if version != row.version || raw != row.raw {
            return Err(A2aPushCutoverError::InventoryChanged);
        }
    }
    Ok(())
}

async fn verify_dlq_rows(
    state: &dyn StateStore,
    rows: &[LegacyDlqRow],
) -> Result<(), A2aPushCutoverError> {
    for row in rows {
        let Some((raw, version)) = state.get_versioned(&row.key).await? else {
            return Err(A2aPushCutoverError::InventoryChanged);
        };
        if version != row.version || raw != row.raw {
            return Err(A2aPushCutoverError::InventoryChanged);
        }
    }
    Ok(())
}

async fn delete_config_rows(
    state: &dyn StateStore,
    rows: &[LegacyConfigRow],
) -> Result<(), A2aPushCutoverError> {
    for row in rows {
        if !state.compare_and_delete(&row.key, row.version).await? {
            return Err(A2aPushCutoverError::InventoryChanged);
        }
    }
    Ok(())
}

async fn delete_dlq_rows(
    state: &dyn StateStore,
    rows: &[LegacyDlqRow],
) -> Result<(), A2aPushCutoverError> {
    for row in rows {
        if !state.compare_and_delete(&row.key, row.version).await? {
            return Err(A2aPushCutoverError::InventoryChanged);
        }
    }
    Ok(())
}

async fn ensure_no_legacy_rows(state: &dyn StateStore) -> Result<(), A2aPushCutoverError> {
    if state
        .scan_keys_by_kind_page(KeyKind::A2aTaskPushConfig, None, 1)
        .await?
        .entries
        .is_empty()
        && state
            .scan_keys_by_kind_page(KeyKind::A2aPushDlq, None, 1)
            .await?
            .entries
            .is_empty()
    {
        Ok(())
    } else {
        Err(A2aPushCutoverError::PostCutoverLegacyRows)
    }
}

async fn scan_kind(
    state: &dyn StateStore,
    kind: KeyKind,
) -> Result<Vec<(String, String)>, A2aPushCutoverError> {
    let mut entries = Vec::new();
    let mut after = None;
    loop {
        let page = state
            .scan_keys_by_kind_page(kind.clone(), after.as_deref(), 500)
            .await?;
        entries.extend(page.entries);
        let Some(cursor) = page.next_cursor else {
            break;
        };
        if after.as_ref().is_some_and(|previous| previous >= &cursor) {
            return Err(A2aPushCutoverError::Invalid(
                "state inventory cursor did not advance".into(),
            ));
        }
        after = Some(cursor);
    }
    Ok(entries)
}

fn task_identity_from_config(config: &TaskPushNotificationConfig) -> TaskIdentity {
    TaskIdentity {
        namespace: config.namespace.clone(),
        tenant: config.tenant.clone(),
        task_id: config.task_id.clone(),
    }
}

fn task_identity_from_dlq(entry: &PushDeliveryDlqEntry) -> TaskIdentity {
    TaskIdentity {
        namespace: entry.namespace.clone(),
        tenant: entry.tenant.clone(),
        task_id: entry.task_id.clone(),
    }
}

fn validate_operator_text(
    field: &str,
    value: &str,
    max_bytes: usize,
) -> Result<(), A2aPushCutoverError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(A2aPushCutoverError::Invalid(format!(
            "{field} must be non-empty, bounded text"
        )));
    }
    Ok(())
}

fn digest_json(value: &impl Serialize) -> Result<String, serde_json::Error> {
    Ok(digest_bytes(&serde_json::to_vec(value)?))
}

fn digest_bytes(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::{DlqFailureKind, Task};
    use acteon_state_memory::MemoryStateStore;

    #[tokio::test]
    async fn reviewed_cutover_moves_rows_hides_secrets_and_is_idempotent() {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let engine = TaskEngine::new(state.clone());
        let task = Task::new("task-1", "agents", "demo");
        engine.create_task(task.clone()).await.unwrap();

        let mut config = TaskPushNotificationConfig::new(
            "callback-1",
            &task.id,
            task.namespace.as_str(),
            task.tenant.as_str(),
            "https://receiver.example/a2a",
        );
        config.token = Some("never-print-this-secret".into());
        let config_key = StateKey::new(
            task.namespace.as_str(),
            task.tenant.as_str(),
            KeyKind::A2aTaskPushConfig,
            format!("{}:{}", task.id, config.id),
        );
        state
            .set(&config_key, &serde_json::to_string(&config).unwrap(), None)
            .await
            .unwrap();

        let entry = PushDeliveryDlqEntry::new(
            uuid::Uuid::now_v7().to_string(),
            &config.id,
            &task.id,
            &task.namespace,
            &task.tenant,
            &config.url,
            DlqFailureKind::Exhausted,
            "receiver failed",
            3,
            "{}".into(),
        );
        let dlq_key = StateKey::new(
            task.namespace.as_str(),
            task.tenant.as_str(),
            KeyKind::A2aPushDlq,
            entry.storage_id(),
        );
        state
            .set(&dlq_key, &serde_json::to_string(&entry).unwrap(), None)
            .await
            .unwrap();

        let plan = A2aPushCutoverPlan::preview(
            state.clone(),
            "operator@example.com",
            "retire standalone callback rows",
        )
        .await
        .unwrap();
        let report = serde_json::to_string(plan.report()).unwrap();
        assert!(!report.contains("never-print-this-secret"));
        assert_eq!(plan.report().legacy_config_count, 1);
        assert_eq!(plan.report().legacy_dead_letter_count, 1);
        assert!(plan.apply("wrong").await.is_err());
        assert!(plan.apply(&plan.report().review_digest).await.unwrap());
        assert!(!plan.apply(&plan.report().review_digest).await.unwrap());
        assert!(a2a_push_cutover_complete(state.as_ref()).await.unwrap());
        assert!(state.get(&config_key).await.unwrap().is_none());
        assert!(state.get(&dlq_key).await.unwrap().is_none());

        let scope = TaskScope::new(&task.namespace, &task.tenant);
        let imported = engine
            .get_push_config(&scope, &task.id, &config.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(imported).unwrap(),
            serde_json::to_value(&config).unwrap()
        );
        assert_eq!(
            engine
                .list_push_dead_letters(&scope, &task.id)
                .await
                .unwrap(),
            vec![entry]
        );

        state
            .set(&config_key, &serde_json::to_string(&config).unwrap(), None)
            .await
            .unwrap();
        assert!(matches!(
            verify_a2a_push_cutover_fence(state.as_ref()).await,
            Err(A2aPushCutoverError::PostCutoverLegacyRows)
        ));
    }

    #[tokio::test]
    async fn apply_rejects_inventory_drift() {
        let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
        let engine = TaskEngine::new(state.clone());
        let task = Task::new("task-1", "agents", "demo");
        engine.create_task(task.clone()).await.unwrap();
        let plan = A2aPushCutoverPlan::preview(state.clone(), "operator", "cut over")
            .await
            .unwrap();
        engine
            .create_task(Task::new("task-2", "agents", "demo"))
            .await
            .unwrap();
        assert!(matches!(
            plan.apply(&plan.report().review_digest).await,
            Err(A2aPushCutoverError::InventoryChanged)
        ));
    }
}
