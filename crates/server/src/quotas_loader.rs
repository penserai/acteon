//! Static quota policy loader with hot-reload support.
//!
//! Operators can declare quota policies in a TOML file
//! (`[[quotas]]` array) referenced from `[server.quotas].policies_file`.
//! This module materializes those declarations into runtime
//! [`QuotaPolicy`] records and reconciles them against any
//! previously-loaded TOML records — additions are upserted, deletions
//! are pruned.
//!
//! API-managed policies are untouched: TOML-loaded records carry a
//! reserved `_source = "toml"` label that scopes the reconcile to its
//! own set, and use deterministic `UUIDv5` IDs derived from
//! `(namespace, tenant, provider, principal, per_principal)` so the
//! same TOML entry reloaded twice is a no-op rather than a duplicate.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use acteon_core::{OverageBehavior, QuotaPolicy, QuotaWindow};
use acteon_gateway::Gateway;
use acteon_state::{KeyKind, StateKey, StateStore};
use chrono::Utc;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use tokio::sync::{Notify, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Label key marking a policy as loaded from the TOML loader. The
/// reconcile step only deletes records carrying this label, so
/// API-managed quotas are never collaterally removed.
pub const SOURCE_LABEL_KEY: &str = "_source";
/// Label value paired with [`SOURCE_LABEL_KEY`].
pub const SOURCE_LABEL_TOML: &str = "toml";

/// Project-specific UUID namespace used to derive deterministic
/// policy IDs from scope tuples. Chosen once and stable forever — do
/// not change without a migration story (it would orphan every
/// existing TOML-loaded policy record).
const QUOTA_NS: Uuid = Uuid::from_u128(0x6c_36_8e_5a_42_24_4e_8a_a5_91_15_d8_e0_3a_77_61);

/// State-store coordinates that the runtime uses for quota policy
/// records. Mirrors the constants in `api/quotas.rs`; duplicated here
/// to avoid making them `pub` across crate boundaries.
const QUOTA_STORE_NS: &str = "_system";
const QUOTA_STORE_TENANT: &str = "_quotas";

/// Default debounce interval between file change events and the
/// resulting reload, matching the auth watcher.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(500);

/// `AppState` handle for static quota administration. Carries the
/// configured file path and a nudger that the reload endpoint can
/// poke when a file watcher is also running, so the two paths
/// coalesce instead of racing.
#[derive(Clone)]
pub struct StaticQuotasHandle {
    pub path: PathBuf,
    pub nudge: Arc<Notify>,
}

/// One `[[quotas]]` entry as parsed from the TOML file.
#[derive(Debug, Clone, Deserialize)]
pub struct StaticQuotaEntry {
    pub namespace: String,
    pub tenant: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub principal: Option<String>,
    #[serde(default)]
    pub per_principal: bool,
    pub max_actions: u64,
    /// `"hourly"`, `"daily"`, `"weekly"`, `"monthly"`, or an integer
    /// number of seconds for a custom window.
    pub window: WindowSpec,
    pub overage_behavior: OverageBehavior,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

fn default_enabled() -> bool {
    true
}

/// Window field that accepts either a named variant or a custom
/// integer seconds count.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum WindowSpec {
    Named(String),
    Seconds(u64),
}

impl WindowSpec {
    fn into_window(self) -> Result<QuotaWindow, String> {
        match self {
            Self::Named(s) => match s.as_str() {
                "hourly" => Ok(QuotaWindow::Hourly),
                "daily" => Ok(QuotaWindow::Daily),
                "weekly" => Ok(QuotaWindow::Weekly),
                "monthly" => Ok(QuotaWindow::Monthly),
                other => Err(format!(
                    "invalid window {other:?} (expected hourly/daily/weekly/monthly or integer seconds)"
                )),
            },
            Self::Seconds(0) => {
                Err("invalid window: custom seconds must be greater than 0".into())
            }
            Self::Seconds(s) => Ok(QuotaWindow::Custom { seconds: s }),
        }
    }
}

/// Top-level TOML structure: `[[quotas]]` array at the file root.
#[derive(Debug, Deserialize, Default)]
pub struct StaticQuotaFile {
    #[serde(default)]
    pub quotas: Vec<StaticQuotaEntry>,
}

/// Summary returned by [`reconcile`] and the reload endpoint.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReconcileReport {
    pub upserted: usize,
    pub deleted: usize,
    pub skipped: usize,
}

/// Deterministic ID for a TOML-loaded policy. Derived from the scope
/// tuple so the same entry reloaded twice keeps the same record.
pub fn derive_id(
    namespace: &str,
    tenant: &str,
    provider: Option<&str>,
    principal: Option<&str>,
    per_principal: bool,
) -> String {
    let name = format!(
        "{namespace}|{tenant}|{}|{}|{per_principal}",
        provider.unwrap_or(""),
        principal.unwrap_or(""),
    );
    Uuid::new_v5(&QUOTA_NS, name.as_bytes()).to_string()
}

/// Convert a [`StaticQuotaEntry`] into the runtime [`QuotaPolicy`],
/// applying the deterministic ID and source label.
pub fn entry_to_policy(entry: StaticQuotaEntry) -> Result<QuotaPolicy, String> {
    let window = entry.window.into_window()?;
    let id = derive_id(
        &entry.namespace,
        &entry.tenant,
        entry.provider.as_deref(),
        entry.principal.as_deref(),
        entry.per_principal,
    );
    let mut labels = entry.labels;
    labels.insert(SOURCE_LABEL_KEY.into(), SOURCE_LABEL_TOML.into());
    let now = Utc::now();
    let policy = QuotaPolicy {
        id,
        namespace: entry.namespace,
        tenant: entry.tenant,
        provider: entry.provider,
        principal: entry.principal,
        per_principal: entry.per_principal,
        max_actions: entry.max_actions,
        window,
        overage_behavior: entry.overage_behavior,
        enabled: entry.enabled,
        created_at: now,
        updated_at: now,
        description: entry.description,
        labels,
    };
    policy.validate_scope()?;
    Ok(policy)
}

/// Parse and reconcile a static quotas file against the running
/// gateway + state store. Idempotent: running it twice with the same
/// file is a no-op aside from `updated_at` bookkeeping.
pub async fn reload_from_file(
    path: &Path,
    gateway: &Arc<RwLock<Gateway>>,
    state: &Arc<dyn StateStore>,
) -> Result<ReconcileReport, String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let parsed: StaticQuotaFile =
        toml::from_str(&contents).map_err(|e| format!("parse {}: {e}", path.display()))?;

    let mut policies: Vec<QuotaPolicy> = Vec::with_capacity(parsed.quotas.len());
    let mut skipped = 0usize;
    for entry in parsed.quotas {
        match entry_to_policy(entry) {
            Ok(p) => policies.push(p),
            Err(e) => {
                warn!(error = %e, "skipping invalid static quota entry");
                skipped += 1;
            }
        }
    }
    let mut report = reconcile(policies, gateway, state).await?;
    report.skipped += skipped;
    Ok(report)
}

/// Reconcile the given set of TOML-derived policies against the
/// gateway + state store. Adds/updates entries in the set, deletes
/// TOML-tagged records that are no longer present.
pub async fn reconcile(
    desired: Vec<QuotaPolicy>,
    gateway: &Arc<RwLock<Gateway>>,
    state: &Arc<dyn StateStore>,
) -> Result<ReconcileReport, String> {
    let desired_ids: HashSet<String> = desired.iter().map(|p| p.id.clone()).collect();

    // Snapshot existing TOML-tagged records before mutating, so the
    // delete pass only considers stale ones.
    let existing_toml = load_existing_toml_ids(state.as_ref()).await?;

    // Upsert desired policies into the state store and the gateway's
    // in-memory bucket. The state-store write also fixes up the
    // per-(ns, tenant) index so the gateway cold-path loader can find
    // the record.
    let mut upserted = 0usize;
    for policy in desired {
        write_policy(state.as_ref(), &policy).await?;
        let gw = gateway.read().await;
        gw.set_quota_policy(policy);
        drop(gw);
        upserted += 1;
    }

    // Delete TOML-tagged records that are no longer in the desired
    // set. This must run AFTER the upserts so an in-place edit (same
    // id) never traverses a deleted state.
    let mut deleted = 0usize;
    for existing in existing_toml {
        if !desired_ids.contains(&existing.id) {
            delete_policy(state.as_ref(), &existing).await?;
            let gw = gateway.read().await;
            gw.remove_quota_policy_by_id(&existing.namespace, &existing.tenant, &existing.id);
            drop(gw);
            deleted += 1;
        }
    }

    Ok(ReconcileReport {
        upserted,
        deleted,
        skipped: 0,
    })
}

/// Minimal projection of an existing record used by the reconcile
/// delete pass.
#[derive(Debug, Clone)]
struct ExistingPolicyRef {
    id: String,
    namespace: String,
    tenant: String,
}

async fn load_existing_toml_ids(state: &dyn StateStore) -> Result<Vec<ExistingPolicyRef>, String> {
    let scanned = state
        .scan_keys_by_kind(KeyKind::Quota)
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (_key, raw) in scanned {
        // Index entries are JSON arrays of ids and don't deserialize
        // as a QuotaPolicy — skip them silently.
        let Ok(policy) = serde_json::from_str::<QuotaPolicy>(&raw) else {
            continue;
        };
        if policy.labels.get(SOURCE_LABEL_KEY).map(String::as_str) == Some(SOURCE_LABEL_TOML) {
            out.push(ExistingPolicyRef {
                id: policy.id,
                namespace: policy.namespace,
                tenant: policy.tenant,
            });
        }
    }
    Ok(out)
}

/// Persist a policy to the state store and update the per-tenant
/// index. The index format mirrors `api/quotas.rs` so the gateway
/// cold-path loader picks it up identically to API-created records.
async fn write_policy(state: &dyn StateStore, policy: &QuotaPolicy) -> Result<(), String> {
    let policy_key = StateKey::new(
        QUOTA_STORE_NS,
        QUOTA_STORE_TENANT,
        KeyKind::Quota,
        &policy.id,
    );
    let data = serde_json::to_string(policy).map_err(|e| e.to_string())?;
    state
        .set(&policy_key, &data, None)
        .await
        .map_err(|e| e.to_string())?;

    let idx_suffix = format!("idx:{}:{}", policy.namespace, policy.tenant);
    let idx_key = StateKey::new(
        QUOTA_STORE_NS,
        QUOTA_STORE_TENANT,
        KeyKind::Quota,
        &idx_suffix,
    );
    let mut ids = read_index(state, &idx_key).await?;
    if !ids.contains(&policy.id) {
        ids.push(policy.id.clone());
        write_index(state, &idx_key, &ids).await?;
    }
    Ok(())
}

async fn delete_policy(state: &dyn StateStore, p: &ExistingPolicyRef) -> Result<(), String> {
    let key = StateKey::new(QUOTA_STORE_NS, QUOTA_STORE_TENANT, KeyKind::Quota, &p.id);
    state.delete(&key).await.map_err(|e| e.to_string())?;
    let idx_suffix = format!("idx:{}:{}", p.namespace, p.tenant);
    let idx_key = StateKey::new(
        QUOTA_STORE_NS,
        QUOTA_STORE_TENANT,
        KeyKind::Quota,
        &idx_suffix,
    );
    let mut ids = read_index(state, &idx_key).await?;
    ids.retain(|id| id != &p.id);
    write_index(state, &idx_key, &ids).await?;
    Ok(())
}

async fn read_index(state: &dyn StateStore, key: &StateKey) -> Result<Vec<String>, String> {
    let Some(raw) = state.get(key).await.map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    match serde_json::from_str::<Vec<String>>(&raw) {
        Ok(ids) => Ok(ids),
        Err(_) => Ok(vec![raw]), // legacy: bare UUID
    }
}

async fn write_index(state: &dyn StateStore, key: &StateKey, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        state.delete(key).await.map_err(|e| e.to_string())?;
        return Ok(());
    }
    let json = serde_json::to_string(ids).map_err(|e| e.to_string())?;
    state.set(key, &json, None).await.map_err(|e| e.to_string())
}

/// Background file watcher mirroring [`crate::auth::watcher::AuthWatcher`].
/// Watches the parent directory (so atomic-replace editor saves are
/// caught) and fires the reload routine on debounced change events.
pub struct QuotaWatcher {
    path: PathBuf,
    gateway: Arc<RwLock<Gateway>>,
    state: Arc<dyn StateStore>,
    debounce: Duration,
    /// External nudger — `POST /v1/quotas/reload` notifies this so
    /// the watcher loop coalesces with manual reloads.
    nudge: Arc<Notify>,
}

impl QuotaWatcher {
    pub fn new(
        path: impl Into<PathBuf>,
        gateway: Arc<RwLock<Gateway>>,
        state: Arc<dyn StateStore>,
        nudge: Arc<Notify>,
    ) -> Self {
        Self {
            path: path.into(),
            gateway,
            state,
            debounce: DEFAULT_DEBOUNCE,
            nudge,
        }
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.run().await {
                error!(error = %e, "quota watcher exited with error");
            }
        })
    }

    async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(16);
        let watch_dir = self.path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let filename = self
            .path
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_default();

        let _watcher = {
            let tx = tx.clone();
            let filename = filename.clone();
            let mut watcher = RecommendedWatcher::new(
                move |res: Result<notify::Event, notify::Error>| match res {
                    Ok(event) => {
                        if is_relevant_event(event.kind) {
                            let is_our_file = event.paths.iter().any(|p| {
                                p.file_name()
                                    .is_some_and(|name| name == filename.as_os_str())
                            });
                            if is_our_file {
                                let _ = tx.try_send(());
                            }
                        }
                    }
                    Err(e) => warn!(error = %e, "quota watcher filesystem error"),
                },
                notify::Config::default(),
            )?;
            watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;
            info!(path = %self.path.display(), "quota watcher started");
            watcher
        };

        loop {
            tokio::select! {
                file_event = rx.recv() => {
                    if file_event.is_none() {
                        debug!("quota watcher channel closed");
                        break;
                    }
                }
                () = self.nudge.notified() => {
                    debug!("quota watcher received manual nudge");
                }
            }

            // Debounce: absorb any further events arriving within the window.
            tokio::time::sleep(self.debounce).await;
            while rx.try_recv().is_ok() {}

            self.do_reload().await;
        }
        Ok(())
    }

    async fn do_reload(&self) {
        info!(path = %self.path.display(), "reloading static quotas");
        match reload_from_file(&self.path, &self.gateway, &self.state).await {
            Ok(report) => info!(
                upserted = report.upserted,
                deleted = report.deleted,
                skipped = report.skipped,
                "static quotas reloaded"
            ),
            Err(e) => error!(error = %e, "static quota reload failed; keeping previous state"),
        }
    }
}

fn is_relevant_event(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_id_is_deterministic() {
        let a = derive_id("ns", "t", Some("slack"), Some("alice"), false);
        let b = derive_id("ns", "t", Some("slack"), Some("alice"), false);
        assert_eq!(a, b);
    }

    #[test]
    fn derive_id_changes_with_each_dimension() {
        let base = derive_id("ns", "t", None, None, false);
        assert_ne!(base, derive_id("ns2", "t", None, None, false));
        assert_ne!(base, derive_id("ns", "t2", None, None, false));
        assert_ne!(base, derive_id("ns", "t", Some("x"), None, false));
        assert_ne!(base, derive_id("ns", "t", None, Some("x"), false));
        assert_ne!(base, derive_id("ns", "t", None, None, true));
    }

    #[test]
    fn entry_to_policy_round_trip() {
        let entry = StaticQuotaEntry {
            namespace: "n".into(),
            tenant: "t".into(),
            provider: None,
            principal: None,
            per_principal: true,
            max_actions: 10,
            window: WindowSpec::Named("hourly".into()),
            overage_behavior: OverageBehavior::Block,
            enabled: true,
            description: None,
            labels: HashMap::new(),
        };
        let p = entry_to_policy(entry).unwrap();
        assert!(p.per_principal);
        assert_eq!(
            p.labels.get(SOURCE_LABEL_KEY),
            Some(&SOURCE_LABEL_TOML.to_string())
        );
    }

    #[test]
    fn window_spec_named_and_seconds() {
        assert!(matches!(
            WindowSpec::Named("hourly".into()).into_window().unwrap(),
            QuotaWindow::Hourly
        ));
        assert!(matches!(
            WindowSpec::Seconds(60).into_window().unwrap(),
            QuotaWindow::Custom { seconds: 60 }
        ));
        assert!(WindowSpec::Seconds(0).into_window().is_err());
        assert!(WindowSpec::Named("monthly2".into()).into_window().is_err());
    }

    #[test]
    fn parse_static_quota_file() {
        let toml_src = r#"
            [[quotas]]
            namespace = "notifications"
            tenant = "acme"
            max_actions = 1000
            window = "daily"
            overage_behavior = "block"

            [[quotas]]
            namespace = "notifications"
            tenant = "acme"
            principal = "svc-billing"
            max_actions = 100
            window = "hourly"
            overage_behavior = "block"
            description = "Billing service cap"

            [[quotas]]
            namespace = "messaging"
            tenant = "acme"
            per_principal = true
            max_actions = 50
            window = 3600
            overage_behavior = { degrade = { fallback_provider = "log" } }
        "#;
        let parsed: StaticQuotaFile = toml::from_str(toml_src).unwrap();
        assert_eq!(parsed.quotas.len(), 3);
        assert_eq!(parsed.quotas[2].per_principal, true);
        assert!(matches!(parsed.quotas[2].window, WindowSpec::Seconds(3600)));
    }
}
