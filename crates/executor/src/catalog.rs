//! Immutable qualification of actual selected provider instances.
//!
//! Catalog definitions are inspection metadata. Only trusted host construction
//! from live bindings establishes a catalog; deserialized labels cannot do so.
use std::{collections::BTreeMap, sync::Arc};

use acteon_core::{Action, ResourceRef};
use acteon_governance::context::AcceptedEffect;
use acteon_provider::DynProvider;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::governed::BoundProvider;

const MAX_ENTRIES: usize = 4096;

/// Public description of one qualified route, including its complete resources.
/// This does not grant execution authority or authenticate a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QualifiedProviderDefinition {
    pub namespace: String,
    pub tenant: String,
    pub provider: String,
    pub action_type: String,
    pub endpoint: ResourceRef,
    pub revision: String,
    pub failure_revision: String,
    pub effect: AcceptedEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    #[error("qualified provider catalog is empty or exceeds capacity")]
    Capacity,
    #[error("qualified provider route has more than one binding")]
    Ambiguous,
    #[error("actual provider or action has no matching qualification")]
    Unqualified,
}

type Route = (String, String, String, String);

/// Host-created finite catalog. Instance identity is checked in addition to
/// route metadata, so a fallback or replacement cannot borrow another binding.
/// Cloning shares the exact immutable provider instances, not reconstructed ones.
#[derive(Clone)]
pub struct QualifiedProviderCatalog {
    entries: BTreeMap<Route, BoundProvider>,
    fingerprint: String,
}

impl QualifiedProviderCatalog {
    pub fn new_trusted(bindings: Vec<BoundProvider>) -> Result<Self, CatalogError> {
        if bindings.is_empty() || bindings.len() > MAX_ENTRIES {
            return Err(CatalogError::Capacity);
        }
        let mut entries = BTreeMap::new();
        for binding in bindings {
            let binding = binding
                .qualify_for_catalog()
                .map_err(|_| CatalogError::Unqualified)?;
            let definition = binding.catalog_definition();
            let route = (
                definition.namespace,
                definition.tenant,
                definition.provider,
                definition.action_type,
            );
            if entries.insert(route, binding).is_some() {
                return Err(CatalogError::Ambiguous);
            }
        }
        let definitions: Vec<_> = entries
            .values()
            .map(BoundProvider::catalog_definition)
            .collect();
        let bytes = serde_json::to_vec(&definitions).map_err(|_| CatalogError::Unqualified)?;
        let fingerprint = format!("{:x}", Sha256::digest(bytes));
        Ok(Self {
            entries,
            fingerprint,
        })
    }

    /// Resolve from the actual selected instance. The request's provider label
    /// is not the selected identity: routing may have chosen a fallback.
    pub fn resolve(
        &self,
        action: &Action,
        selected: &Arc<dyn DynProvider>,
    ) -> Result<&BoundProvider, CatalogError> {
        let route = (
            action.namespace.as_str().into(),
            action.tenant.as_str().into(),
            selected.name().into(),
            action.action_type.clone(),
        );
        self.entries
            .get(&route)
            .filter(|bound| bound.is_provider(selected))
            .ok_or(CatalogError::Unqualified)
    }

    #[must_use]
    pub fn definitions(&self, namespace: &str, tenant: &str) -> Vec<QualifiedProviderDefinition> {
        self.entries
            .iter()
            .filter(|((ns, t, _, _), _)| ns == namespace && t == tenant)
            .map(|(_, bound)| bound.catalog_definition())
            .collect()
    }

    /// Canonical descriptor fingerprint. Host revisions must bind actual
    /// immutable settings and must not contain secrets. This is not a permit.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}
