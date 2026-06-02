//! Template and template profile CRUD methods on [`Gateway`].
//!
//! Extracted from the main gateway module to keep `gateway.rs` focused on
//! dispatch orchestration.

use std::collections::HashMap;

use acteon_state::KeyKind;

use crate::error::GatewayError;
use crate::gateway::Gateway;

impl Gateway {
    /// Return a flat snapshot of the current in-memory templates.
    ///
    /// Keyed by `"namespace:tenant:name"` for backward compatibility with
    /// CRUD handlers. **Not** used on the hot dispatch path.
    pub fn templates(&self) -> HashMap<String, acteon_core::Template> {
        self.templates
            .read()
            .iter()
            .flat_map(|((ns, t), inner)| {
                inner
                    .iter()
                    .map(move |(name, tpl)| (format!("{ns}:{t}:{name}"), tpl.clone()))
            })
            .collect()
    }

    /// Add or replace a template in the nested map.
    pub fn set_template(&self, template: acteon_core::Template) {
        let scope = (template.namespace.clone(), template.tenant.clone());
        self.templates
            .write()
            .entry(scope)
            .or_default()
            .insert(template.name.clone(), template);
    }

    /// Remove a template by scope and name.
    pub fn remove_template(
        &self,
        namespace: &str,
        tenant: &str,
        name: &str,
    ) -> Option<acteon_core::Template> {
        let scope = (namespace.to_owned(), tenant.to_owned());
        let mut guard = self.templates.write();
        let inner = guard.get_mut(&scope)?;
        let removed = inner.remove(name);
        if inner.is_empty() {
            guard.remove(&scope);
        }
        removed
    }

    /// Check whether a template exists in a given scope — O(1).
    pub fn template_exists(&self, namespace: &str, tenant: &str, name: &str) -> bool {
        let scope = (namespace.to_owned(), tenant.to_owned());
        self.templates
            .read()
            .get(&scope)
            .is_some_and(|inner| inner.contains_key(name))
    }

    /// Return a flat snapshot of the current in-memory template profiles.
    ///
    /// Keyed by `"namespace:tenant:name"` for backward compatibility with
    /// CRUD handlers. **Not** used on the hot dispatch path.
    pub fn template_profiles(&self) -> HashMap<String, acteon_core::TemplateProfile> {
        self.template_profiles
            .read()
            .iter()
            .flat_map(|((ns, t), inner)| {
                inner
                    .iter()
                    .map(move |(name, prof)| (format!("{ns}:{t}:{name}"), prof.clone()))
            })
            .collect()
    }

    /// Return all template profiles for a `(namespace, tenant)` scope — O(1).
    pub fn template_profiles_for_scope(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> HashMap<String, acteon_core::TemplateProfile> {
        let scope = (namespace.to_owned(), tenant.to_owned());
        self.template_profiles
            .read()
            .get(&scope)
            .cloned()
            .unwrap_or_default()
    }

    /// Look up a single template profile by scope and name — O(1).
    pub fn template_profile_by_scope(
        &self,
        namespace: &str,
        tenant: &str,
        name: &str,
    ) -> Option<acteon_core::TemplateProfile> {
        let scope = (namespace.to_owned(), tenant.to_owned());
        self.template_profiles
            .read()
            .get(&scope)
            .and_then(|inner| inner.get(name).cloned())
    }

    /// Add or replace a template profile in the nested map.
    pub fn set_template_profile(&self, profile: acteon_core::TemplateProfile) {
        let scope = (profile.namespace.clone(), profile.tenant.clone());
        self.template_profiles
            .write()
            .entry(scope)
            .or_default()
            .insert(profile.name.clone(), profile);
    }

    /// Remove a template profile by scope and name.
    pub fn remove_template_profile(
        &self,
        namespace: &str,
        tenant: &str,
        name: &str,
    ) -> Option<acteon_core::TemplateProfile> {
        let scope = (namespace.to_owned(), tenant.to_owned());
        let mut guard = self.template_profiles.write();
        let inner = guard.get_mut(&scope)?;
        let removed = inner.remove(name);
        if inner.is_empty() {
            guard.remove(&scope);
        }
        removed
    }

    /// Get scoped templates for a namespace + tenant pair — O(1).
    pub fn templates_for_scope(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> HashMap<String, acteon_core::Template> {
        let scope = (namespace.to_owned(), tenant.to_owned());
        self.templates
            .read()
            .get(&scope)
            .cloned()
            .unwrap_or_default()
    }

    /// Reload all templates and profiles from the state store into memory.
    ///
    /// Used by the background sync task to keep multi-node deployments
    /// consistent. Returns the total number of items synced.
    ///
    // The poll still happens at `template_sync_interval` (default 30 s), but a
    // sync-version gate (see `acteon_state::sync_version`) makes the common
    // "nothing changed" tick an O(1) counter read instead of a full-keyspace
    // scan: template CRUD bumps the `Templates` version, and this sync only
    // rescans when the version moved (or on a periodic reconcile). A true
    // push mechanism (Redis pub/sub, Postgres LISTEN/NOTIFY, DynamoDB Streams)
    // would further cut propagation latency below the poll interval, but the
    // gate already removes the per-tick full-keyspace scan that dominated cost.
    pub async fn sync_templates_from_store(&self) -> Result<usize, GatewayError> {
        let version = acteon_state::read_sync_version(
            self.state.as_ref(),
            acteon_state::SyncDomain::Templates,
        )
        .await
        .unwrap_or(0);
        if !self
            .sync_versions
            .should_sync(acteon_state::SyncDomain::Templates, version)
        {
            // Unchanged since the last sync — skip the scan, report cache size.
            let cached = self
                .templates
                .read()
                .values()
                .map(HashMap::len)
                .sum::<usize>()
                + self
                    .template_profiles
                    .read()
                    .values()
                    .map(HashMap::len)
                    .sum::<usize>();
            return Ok(cached);
        }

        let mut new_templates: HashMap<(String, String), HashMap<String, acteon_core::Template>> =
            HashMap::new();
        let mut new_profiles: HashMap<
            (String, String),
            HashMap<String, acteon_core::TemplateProfile>,
        > = HashMap::new();

        let tpl_entries = self
            .state
            .scan_keys_by_kind(KeyKind::Template)
            .await
            .map_err(|e| GatewayError::Configuration(format!("template sync scan failed: {e}")))?;

        let mut count = 0usize;
        for (_key, value) in &tpl_entries {
            if let Ok(tpl) = serde_json::from_str::<acteon_core::Template>(value) {
                let scope = (tpl.namespace.clone(), tpl.tenant.clone());
                new_templates
                    .entry(scope)
                    .or_default()
                    .insert(tpl.name.clone(), tpl);
                count += 1;
            }
        }

        let prof_entries = self
            .state
            .scan_keys_by_kind(KeyKind::TemplateProfile)
            .await
            .map_err(|e| GatewayError::Configuration(format!("profile sync scan failed: {e}")))?;

        for (_key, value) in &prof_entries {
            if let Ok(prof) = serde_json::from_str::<acteon_core::TemplateProfile>(value) {
                let scope = (prof.namespace.clone(), prof.tenant.clone());
                new_profiles
                    .entry(scope)
                    .or_default()
                    .insert(prof.name.clone(), prof);
                count += 1;
            }
        }

        *self.templates.write() = new_templates;
        *self.template_profiles.write() = new_profiles;

        self.sync_versions
            .record(acteon_state::SyncDomain::Templates, version);
        Ok(count)
    }
}
