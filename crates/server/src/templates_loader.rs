//! Static template / profile loader with hot-reload support.
//!
//! Mirrors [`crate::quotas_loader`]: a single TOML manifest declares
//! `[[templates]]` and `[[profiles]]` arrays, each entry gets a
//! deterministic `UUIDv5` ID derived from (namespace, tenant, name),
//! records are tagged with `_source = "toml"`, and reconciles
//! diff-prune only their own records so API-managed templates are
//! left alone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use acteon_core::{Template, TemplateProfile, TemplateProfileField};
use acteon_gateway::Gateway;
use acteon_state::{KeyKind, StateKey, StateStore};
use chrono::Utc;
use serde::Deserialize;
use tokio::sync::{Notify, RwLock};
use tracing::warn;
use uuid::Uuid;

pub const SOURCE_LABEL_KEY: &str = "_source";
pub const SOURCE_LABEL_TOML: &str = "toml";

/// Project-specific namespace for deterministic template IDs. Same
/// fixed-once-forever rule as `quotas_loader::QUOTA_NS`.
const TEMPLATE_NS: Uuid = Uuid::from_u128(0x8a_4e_d1_2c_d2_6a_4b_70_b6_31_2c_42_06_91_8c_d9);
const PROFILE_NS: Uuid = Uuid::from_u128(0x91_c0_3f_88_e4_4b_4f_92_a7_28_4d_5c_e3_30_5b_a2);

const STORE_NS: &str = "_system";
const STORE_TENANT: &str = "_templates";

/// `AppState` handle for static template administration.
#[derive(Clone)]
pub struct StaticTemplatesHandle {
    pub path: PathBuf,
    pub nudge: Arc<Notify>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StaticTemplateEntry {
    pub namespace: String,
    pub tenant: String,
    pub name: String,
    /// Inline `MiniJinja` template content. For larger payloads use
    /// `content_file = "path/to/template.jinja"` instead.
    #[serde(default)]
    pub content: Option<String>,
    /// Path to a `.jinja` file (relative to the manifest's directory)
    /// whose contents become this template's body. Mutually exclusive
    /// with `content`.
    #[serde(default)]
    pub content_file: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StaticProfileEntry {
    pub namespace: String,
    pub tenant: String,
    pub name: String,
    pub fields: HashMap<String, TemplateProfileField>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StaticTemplateFile {
    #[serde(default)]
    pub templates: Vec<StaticTemplateEntry>,
    #[serde(default)]
    pub profiles: Vec<StaticProfileEntry>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReconcileReport {
    pub templates_upserted: usize,
    pub templates_deleted: usize,
    pub profiles_upserted: usize,
    pub profiles_deleted: usize,
    pub skipped: usize,
}

fn derive_template_id(namespace: &str, tenant: &str, name: &str) -> String {
    let key = format!("{namespace}|{tenant}|{name}");
    Uuid::new_v5(&TEMPLATE_NS, key.as_bytes()).to_string()
}

fn derive_profile_id(namespace: &str, tenant: &str, name: &str) -> String {
    let key = format!("{namespace}|{tenant}|{name}");
    Uuid::new_v5(&PROFILE_NS, key.as_bytes()).to_string()
}

fn entry_to_template(entry: StaticTemplateEntry, manifest_dir: &Path) -> Result<Template, String> {
    let content = match (entry.content.as_deref(), entry.content_file.as_deref()) {
        (Some(c), None) => c.to_string(),
        (None, Some(file)) => {
            let path = manifest_dir.join(file);
            std::fs::read_to_string(&path)
                .map_err(|e| format!("read content_file {}: {e}", path.display()))?
        }
        (Some(_), Some(_)) => {
            return Err(format!(
                "template {:?}: set either `content` or `content_file`, not both",
                entry.name
            ));
        }
        (None, None) => {
            return Err(format!(
                "template {:?}: missing `content` or `content_file`",
                entry.name
            ));
        }
    };
    let mut labels = entry.labels;
    labels.insert(SOURCE_LABEL_KEY.into(), SOURCE_LABEL_TOML.into());
    let now = Utc::now();
    Ok(Template {
        id: derive_template_id(&entry.namespace, &entry.tenant, &entry.name),
        name: entry.name,
        namespace: entry.namespace,
        tenant: entry.tenant,
        content,
        description: entry.description,
        created_at: now,
        updated_at: now,
        labels,
    })
}

fn entry_to_profile(entry: StaticProfileEntry) -> TemplateProfile {
    let mut labels = entry.labels;
    labels.insert(SOURCE_LABEL_KEY.into(), SOURCE_LABEL_TOML.into());
    let now = Utc::now();
    TemplateProfile {
        id: derive_profile_id(&entry.namespace, &entry.tenant, &entry.name),
        name: entry.name,
        namespace: entry.namespace,
        tenant: entry.tenant,
        fields: entry.fields,
        description: entry.description,
        created_at: now,
        updated_at: now,
        labels,
    }
}

pub async fn reload_from_file(
    path: &Path,
    gateway: &Arc<RwLock<Gateway>>,
    state: &Arc<dyn StateStore>,
) -> Result<ReconcileReport, String> {
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let parsed: StaticTemplateFile =
        toml::from_str(&contents).map_err(|e| format!("parse {}: {e}", path.display()))?;

    let manifest_dir = path.parent().unwrap_or(Path::new("."));

    let mut templates: Vec<Template> = Vec::with_capacity(parsed.templates.len());
    let mut profiles: Vec<TemplateProfile> = Vec::with_capacity(parsed.profiles.len());
    let mut skipped = 0usize;

    for entry in parsed.templates {
        match entry_to_template(entry, manifest_dir) {
            Ok(t) => templates.push(t),
            Err(e) => {
                warn!(error = %e, "skipping invalid static template entry");
                skipped += 1;
            }
        }
    }
    for entry in parsed.profiles {
        profiles.push(entry_to_profile(entry));
    }

    let mut report = reconcile(templates, profiles, gateway, state).await?;
    report.skipped += skipped;
    Ok(report)
}

async fn reconcile(
    desired_templates: Vec<Template>,
    desired_profiles: Vec<TemplateProfile>,
    gateway: &Arc<RwLock<Gateway>>,
    state: &Arc<dyn StateStore>,
) -> Result<ReconcileReport, String> {
    let desired_template_ids: std::collections::HashSet<String> =
        desired_templates.iter().map(|t| t.id.clone()).collect();
    let desired_profile_ids: std::collections::HashSet<String> =
        desired_profiles.iter().map(|p| p.id.clone()).collect();

    // Snapshot existing TOML-tagged records before mutating.
    let existing = load_existing_toml(state.as_ref()).await?;

    // Upserts.
    let mut templates_upserted = 0usize;
    for tpl in desired_templates {
        write_template(state.as_ref(), &tpl).await?;
        let gw = gateway.read().await;
        gw.set_template(tpl);
        drop(gw);
        templates_upserted += 1;
    }
    let mut profiles_upserted = 0usize;
    for prof in desired_profiles {
        write_profile(state.as_ref(), &prof).await?;
        let gw = gateway.read().await;
        gw.set_template_profile(prof);
        drop(gw);
        profiles_upserted += 1;
    }

    // Diff-deletes.
    let mut templates_deleted = 0usize;
    for tref in existing.templates {
        if !desired_template_ids.contains(&tref.id) {
            delete_template(state.as_ref(), &tref).await?;
            let gw = gateway.read().await;
            gw.remove_template(&tref.namespace, &tref.tenant, &tref.name);
            drop(gw);
            templates_deleted += 1;
        }
    }
    let mut profiles_deleted = 0usize;
    for pref in existing.profiles {
        if !desired_profile_ids.contains(&pref.id) {
            delete_profile(state.as_ref(), &pref).await?;
            let gw = gateway.read().await;
            gw.remove_template_profile(&pref.namespace, &pref.tenant, &pref.name);
            drop(gw);
            profiles_deleted += 1;
        }
    }

    Ok(ReconcileReport {
        templates_upserted,
        templates_deleted,
        profiles_upserted,
        profiles_deleted,
        skipped: 0,
    })
}

#[derive(Debug, Clone)]
struct ExistingRef {
    id: String,
    namespace: String,
    tenant: String,
    name: String,
}

#[derive(Debug, Default)]
struct ExistingSets {
    templates: Vec<ExistingRef>,
    profiles: Vec<ExistingRef>,
}

async fn load_existing_toml(state: &dyn StateStore) -> Result<ExistingSets, String> {
    let mut out = ExistingSets::default();
    for (_key, raw) in state
        .scan_keys_by_kind(KeyKind::Template)
        .await
        .map_err(|e| e.to_string())?
    {
        let Ok(tpl) = serde_json::from_str::<Template>(&raw) else {
            continue;
        };
        if tpl.labels.get(SOURCE_LABEL_KEY).map(String::as_str) == Some(SOURCE_LABEL_TOML) {
            out.templates.push(ExistingRef {
                id: tpl.id,
                namespace: tpl.namespace,
                tenant: tpl.tenant,
                name: tpl.name,
            });
        }
    }
    for (_key, raw) in state
        .scan_keys_by_kind(KeyKind::TemplateProfile)
        .await
        .map_err(|e| e.to_string())?
    {
        let Ok(prof) = serde_json::from_str::<TemplateProfile>(&raw) else {
            continue;
        };
        if prof.labels.get(SOURCE_LABEL_KEY).map(String::as_str) == Some(SOURCE_LABEL_TOML) {
            out.profiles.push(ExistingRef {
                id: prof.id,
                namespace: prof.namespace,
                tenant: prof.tenant,
                name: prof.name,
            });
        }
    }
    Ok(out)
}

async fn write_template(state: &dyn StateStore, tpl: &Template) -> Result<(), String> {
    let key = StateKey::new(STORE_NS, STORE_TENANT, KeyKind::Template, &tpl.id);
    let data = serde_json::to_string(tpl).map_err(|e| e.to_string())?;
    state
        .set(&key, &data, None)
        .await
        .map_err(|e| e.to_string())?;
    // Mirror the API's name→id index so lookups via the gateway's
    // cold-path loader still resolve cleanly.
    let idx_suffix = format!("idx:{}:{}:{}", tpl.namespace, tpl.tenant, tpl.name);
    let idx_key = StateKey::new(STORE_NS, STORE_TENANT, KeyKind::Template, &idx_suffix);
    state
        .set(&idx_key, &tpl.id, None)
        .await
        .map_err(|e| e.to_string())
}

async fn delete_template(state: &dyn StateStore, tref: &ExistingRef) -> Result<(), String> {
    let key = StateKey::new(STORE_NS, STORE_TENANT, KeyKind::Template, &tref.id);
    state.delete(&key).await.map_err(|e| e.to_string())?;
    let idx_suffix = format!("idx:{}:{}:{}", tref.namespace, tref.tenant, tref.name);
    let idx_key = StateKey::new(STORE_NS, STORE_TENANT, KeyKind::Template, &idx_suffix);
    let _ = state.delete(&idx_key).await;
    Ok(())
}

async fn write_profile(state: &dyn StateStore, prof: &TemplateProfile) -> Result<(), String> {
    let key = StateKey::new(STORE_NS, STORE_TENANT, KeyKind::TemplateProfile, &prof.id);
    let data = serde_json::to_string(prof).map_err(|e| e.to_string())?;
    state
        .set(&key, &data, None)
        .await
        .map_err(|e| e.to_string())?;
    let idx_suffix = format!("idx:{}:{}:{}", prof.namespace, prof.tenant, prof.name);
    let idx_key = StateKey::new(
        STORE_NS,
        STORE_TENANT,
        KeyKind::TemplateProfile,
        &idx_suffix,
    );
    state
        .set(&idx_key, &prof.id, None)
        .await
        .map_err(|e| e.to_string())
}

async fn delete_profile(state: &dyn StateStore, pref: &ExistingRef) -> Result<(), String> {
    let key = StateKey::new(STORE_NS, STORE_TENANT, KeyKind::TemplateProfile, &pref.id);
    state.delete(&key).await.map_err(|e| e.to_string())?;
    let idx_suffix = format!("idx:{}:{}:{}", pref.namespace, pref.tenant, pref.name);
    let idx_key = StateKey::new(
        STORE_NS,
        STORE_TENANT,
        KeyKind::TemplateProfile,
        &idx_suffix,
    );
    let _ = state.delete(&idx_key).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_deterministic_and_orthogonal() {
        let t = derive_template_id("ns", "t", "name");
        assert_eq!(t, derive_template_id("ns", "t", "name"));
        assert_ne!(t, derive_template_id("ns2", "t", "name"));
        assert_ne!(t, derive_template_id("ns", "t", "name2"));

        let p = derive_profile_id("ns", "t", "name");
        assert_ne!(t, p, "template and profile namespaces must differ");
    }

    #[test]
    fn parse_manifest() {
        let src = r#"
            [[templates]]
            namespace = "n"
            tenant = "t"
            name = "greet"
            content = "Hello {{ user }}"

            [[profiles]]
            namespace = "n"
            tenant = "t"
            name = "welcome"
            fields = { subject = "Hi {{ user }}", body = { "$ref" = "greet" } }
        "#;
        let parsed: StaticTemplateFile = toml::from_str(src).unwrap();
        assert_eq!(parsed.templates.len(), 1);
        assert_eq!(parsed.profiles.len(), 1);
        let prof = &parsed.profiles[0];
        assert!(matches!(
            prof.fields.get("subject"),
            Some(TemplateProfileField::Inline(_))
        ));
        assert!(matches!(
            prof.fields.get("body"),
            Some(TemplateProfileField::Ref { .. })
        ));
    }

    #[test]
    fn entry_to_template_inline_content() {
        let entry = StaticTemplateEntry {
            namespace: "n".into(),
            tenant: "t".into(),
            name: "g".into(),
            content: Some("Hi".into()),
            content_file: None,
            description: None,
            labels: HashMap::new(),
        };
        let tpl = entry_to_template(entry, Path::new(".")).unwrap();
        assert_eq!(tpl.content, "Hi");
        assert_eq!(
            tpl.labels.get(SOURCE_LABEL_KEY),
            Some(&SOURCE_LABEL_TOML.to_string())
        );
    }

    #[test]
    fn entry_to_template_rejects_both_or_neither() {
        let neither = StaticTemplateEntry {
            namespace: "n".into(),
            tenant: "t".into(),
            name: "g".into(),
            content: None,
            content_file: None,
            description: None,
            labels: HashMap::new(),
        };
        assert!(entry_to_template(neither, Path::new(".")).is_err());

        let both = StaticTemplateEntry {
            namespace: "n".into(),
            tenant: "t".into(),
            name: "g".into(),
            content: Some("inline".into()),
            content_file: Some("file.jinja".into()),
            description: None,
            labels: HashMap::new(),
        };
        assert!(entry_to_template(both, Path::new(".")).is_err());
    }
}
