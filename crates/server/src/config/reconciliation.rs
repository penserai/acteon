//! Operator-selected external finality trust roots, resolved before publication.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use acteon_executor::governed::reconciliation::{
    HmacFinalityVerifier, ProviderReconciliationVerifier,
};
use serde::Deserialize;
use zeroize::Zeroizing;

use super::ExecutionAuthorityConfig;
use crate::execution_authority::TrustedReconciliationInstallation;

/// An independently qualified source for one exact immutable provider binding.
/// The qualification reference identifies the operator's reviewed contract; it
/// is not proof that an external source enforces that contract.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationSourceConfig {
    pub namespace: String,
    pub tenant: String,
    pub binding_digest: String,
    pub source_id: String,
    pub qualification_ref: String,
    pub verifier_revision: String,
    pub keys: Vec<ReconciliationKeyConfig>,
}

/// Environment reference to a dedicated hex-encoded receipt verification key.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationKeyConfig {
    pub id: String,
    pub secret_env: String,
}

fn text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.trim() == value
        && value != "*"
        && !value.chars().any(char::is_control)
}

/// Resolve the entire trust-root set without accessing or changing state.
/// The environment reader is injectable for deterministic, secret-safe tests.
/// A missing declaration leaves reconciliation disabled.
pub fn prepare_reconciliation_sources(
    sources: &[ReconciliationSourceConfig],
    execution: Option<&ExecutionAuthorityConfig>,
    mut read_secret: impl FnMut(&str) -> Option<Zeroizing<String>>,
) -> Result<Vec<TrustedReconciliationInstallation>, String> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let execution = execution.ok_or("reconciliation sources require execution authority")?;
    if sources.len() > 128 {
        return Err("too many reconciliation sources".into());
    }
    let mut bindings = BTreeSet::new();
    let mut identities = BTreeMap::new();
    // Reject dedicated-key aliases as well as direct references to authority keys.
    let authority_keys: Vec<_> = [
        "ACTEON_EXECUTION_AUTHORITY_KEY",
        "ACTEON_AUTH_AUTHORITY_KEY",
        "ACTEON_AUTH_KEY",
    ]
    .into_iter()
    .filter_map(&mut read_secret)
    .collect();
    let mut installations: BTreeMap<
        (String, String),
        BTreeMap<String, Arc<dyn ProviderReconciliationVerifier>>,
    > = BTreeMap::new();
    for source in sources {
        let scope = execution
            .scopes
            .iter()
            .find(|scope| scope.namespace == source.namespace && scope.tenant == source.tenant)
            .ok_or("undeclared reconciliation scope")?;
        if scope.history_only
            || !text(&source.source_id)
            || !text(&source.qualification_ref)
            || !text(&source.verifier_revision)
            || source.binding_digest.len() != 64
            || !source
                .binding_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || source.keys.is_empty()
            || source.keys.len() > 16
        {
            return Err("invalid reconciliation source declaration".into());
        }
        if !bindings.insert((
            source.namespace.clone(),
            source.tenant.clone(),
            source.binding_digest.clone(),
        )) {
            return Err("duplicate reconciliation binding".into());
        }
        let identity = (
            source.namespace.clone(),
            source.tenant.clone(),
            source.source_id.clone(),
        );
        let contract = (
            &source.qualification_ref,
            &source.verifier_revision,
            &source.keys,
        );
        if identities
            .insert(identity, contract)
            .is_some_and(|previous| previous != contract)
        {
            return Err("conflicting reconciliation source qualification".into());
        }
        let keys = resolve_keys(&source.keys, &authority_keys, &mut read_secret)?;
        let verifier = HmacFinalityVerifier::new_trusted(&source.verifier_revision, keys)
            .map_err(|_| "invalid reconciliation verifier")?;
        installations
            .entry((source.namespace.clone(), source.tenant.clone()))
            .or_default()
            .insert(source.binding_digest.clone(), Arc::new(verifier));
    }
    Ok(installations
        .into_iter()
        .map(
            |((namespace, tenant), bindings)| TrustedReconciliationInstallation {
                namespace,
                tenant,
                bindings,
            },
        )
        .collect())
}

fn resolve_keys(
    declarations: &[ReconciliationKeyConfig],
    authority_keys: &[Zeroizing<String>],
    read_secret: &mut impl FnMut(&str) -> Option<Zeroizing<String>>,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut keys = BTreeMap::new();
    for key in declarations {
        if !text(&key.id)
            || !key.secret_env.starts_with("ACTEON_FINALITY_")
            || !key
                .secret_env
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            || key.secret_env.len() > 256
            || keys.contains_key(&key.id)
        {
            return Err("invalid reconciliation key reference".into());
        }
        let secret = read_secret(&key.secret_env).ok_or("reconciliation key unavailable")?;
        // Bound the encoded input before allocating decoded key material.
        if !(64..=2048).contains(&secret.len()) {
            return Err("invalid reconciliation key material".into());
        }
        let decoded = Zeroizing::new(
            hex::decode(secret.as_bytes()).map_err(|_| "invalid reconciliation key material")?,
        );
        if !(32..=1024).contains(&decoded.len())
            || authority_keys.iter().any(|authority| {
                authority.as_bytes() == decoded.as_slice()
                    || authority.as_bytes() == secret.as_bytes()
                    || hex::decode(authority.as_bytes()).is_ok_and(|bytes| {
                        let bytes = Zeroizing::new(bytes);
                        bytes.as_slice() == decoded.as_slice()
                    })
            })
        {
            return Err("reconciliation requires dedicated key material".into());
        }
        keys.insert(key.id.clone(), decoded.to_vec());
    }
    Ok(keys)
}
