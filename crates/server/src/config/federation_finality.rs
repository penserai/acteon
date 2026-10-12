//! Deployment declarations for independently qualified peer finality sources.

use std::{collections::BTreeSet, sync::Arc};

use acteon_executor::federation_finality::HmacFederationFinalityVerifier;
use serde::Deserialize;
use zeroize::Zeroizing;

use super::{ExecutionAuthorityConfig, ReconciliationKeyConfig, ReconciliationSourceConfig};
use crate::execution_authority::TrustedFederationFinalityInstallation;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationFinalitySourceConfig {
    pub namespace: String,
    pub tenant: String,
    pub export_profile_id: String,
    pub issuer_epoch: u64,
    pub source_id: String,
    pub qualification_ref: String,
    pub verifier_revision: String,
    pub keys: Vec<ReconciliationKeyConfig>,
}

/// Prevent provider-attempt and federation-stage trust domains from sharing a
/// key by alias or by equal decoded material. This runs before either verifier
/// set is installed.
pub fn validate_finality_source_key_separation(
    provider_sources: &[ReconciliationSourceConfig],
    federation_sources: &[FederationFinalitySourceConfig],
    mut read_secret: impl FnMut(&str) -> Option<Zeroizing<String>>,
) -> Result<(), String> {
    let mut names = BTreeSet::new();
    let mut material = BTreeSet::new();
    for reference in provider_sources
        .iter()
        .flat_map(|source| &source.keys)
        .chain(federation_sources.iter().flat_map(|source| &source.keys))
    {
        if !names.insert(reference.secret_env.clone()) {
            return Err("finality trust domains require distinct key references".into());
        }
        let encoded =
            read_secret(&reference.secret_env).ok_or("finality verification key unavailable")?;
        let decoded = Zeroizing::new(
            hex::decode(encoded.as_bytes()).map_err(|_| "invalid finality key material")?,
        );
        if !material.insert(decoded.to_vec()) {
            return Err("finality trust domains require distinct key material".into());
        }
    }
    Ok(())
}

/// Keep capability-card signing independent from execution/auth authority,
/// outbound envelope signing and credentials, and every finality trust domain.
pub fn validate_federation_capability_key_separation(
    execution: Option<&ExecutionAuthorityConfig>,
    provider_sources: &[ReconciliationSourceConfig],
    federation_sources: &[FederationFinalitySourceConfig],
    mut read_secret: impl FnMut(&str) -> Option<Zeroizing<String>>,
) -> Result<(), String> {
    let Some(execution) = execution else {
        return Ok(());
    };
    let publishers: Vec<_> = execution
        .scopes
        .iter()
        .filter_map(|scope| scope.federation.as_ref()?.capability_publisher.as_ref())
        .collect();
    if publishers.is_empty() {
        return Ok(());
    }
    let publisher_names = publishers
        .iter()
        .map(|publisher| publisher.signer_key_env.as_str())
        .collect::<BTreeSet<_>>();
    if publisher_names.len() != publishers.len() {
        return Err("federation capability publishers require distinct key references".into());
    }
    let reserved_names = [
        "ACTEON_EXECUTION_AUTHORITY_KEY",
        "ACTEON_AUTH_AUTHORITY_KEY",
        "ACTEON_AUTH_KEY",
    ]
    .into_iter()
    .chain(
        execution
            .scopes
            .iter()
            .filter_map(|scope| scope.federation.as_ref())
            .flat_map(|federation| &federation.exports)
            .flat_map(|export| &export.epochs)
            .flat_map(|epoch| [epoch.signer_key_env.as_str(), epoch.credential_env.as_str()]),
    )
    .chain(
        provider_sources
            .iter()
            .flat_map(|source| &source.keys)
            .map(|key| key.secret_env.as_str()),
    )
    .chain(
        federation_sources
            .iter()
            .flat_map(|source| &source.keys)
            .map(|key| key.secret_env.as_str()),
    )
    .collect::<BTreeSet<_>>();
    if publisher_names
        .iter()
        .any(|name| reserved_names.contains(name))
    {
        return Err("federation capability signing requires dedicated key references".into());
    }
    let mut reserved_material = Vec::new();
    for name in reserved_names {
        if let Some(secret) = read_secret(name) {
            push_material(&mut reserved_material, secret.as_bytes());
        }
    }
    let mut publisher_material = Vec::new();
    for publisher in publishers {
        let encoded = read_secret(&publisher.signer_key_env)
            .ok_or("federation capability signing key unavailable")?;
        let decoded = hex::decode(encoded.as_bytes())
            .map_err(|_| "federation capability signer must be a 32-byte hex secret")?;
        if decoded.len() != 32 {
            return Err("federation capability signer must be a 32-byte hex secret".into());
        }
        let mut candidates = vec![encoded.as_bytes().to_vec(), decoded];
        if candidates.iter().any(|candidate| {
            reserved_material.contains(candidate) || publisher_material.contains(candidate)
        }) {
            return Err("federation capability signing requires dedicated key material".into());
        }
        publisher_material.append(&mut candidates);
    }
    Ok(())
}

fn push_material(material: &mut Vec<Vec<u8>>, raw: &[u8]) {
    material.push(raw.to_vec());
    if let Ok(decoded) = hex::decode(raw) {
        material.push(decoded);
    }
}

/// Resolve every key before state publication. The profile and epoch are
/// resolved again against the installed runtime binding, so a declaration
/// cannot redirect a verifier by supplying a digest.
pub fn prepare_federation_finality_sources(
    sources: &[FederationFinalitySourceConfig],
    execution: Option<&ExecutionAuthorityConfig>,
    mut read_secret: impl FnMut(&str) -> Option<Zeroizing<String>>,
) -> Result<Vec<TrustedFederationFinalityInstallation>, String> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let execution = execution.ok_or("federation finality sources require execution authority")?;
    if sources.len() > 128 {
        return Err("too many federation finality sources".into());
    }
    let authority_keys: Vec<_> = [
        "ACTEON_EXECUTION_AUTHORITY_KEY",
        "ACTEON_AUTH_AUTHORITY_KEY",
        "ACTEON_AUTH_KEY",
    ]
    .into_iter()
    .filter_map(&mut read_secret)
    .collect();
    let mut selected = BTreeSet::new();
    let mut installations = Vec::new();
    for source in sources {
        let scope = execution
            .scopes
            .iter()
            .find(|scope| scope.namespace == source.namespace && scope.tenant == source.tenant)
            .ok_or("undeclared federation finality scope")?;
        let federation = scope
            .federation
            .as_ref()
            .ok_or("federation finality scope has no exports")?;
        let profile = federation
            .exports
            .iter()
            .find(|profile| profile.id == source.export_profile_id)
            .ok_or("unknown federation finality export profile")?;
        let epoch = profile
            .epochs
            .iter()
            .find(|epoch| epoch.issuer_epoch == source.issuer_epoch)
            .ok_or("unknown federation finality export epoch")?;
        if scope.history_only
            || source.issuer_epoch == 0
            || !text(&source.source_id)
            || !text(&source.qualification_ref)
            || !text(&source.verifier_revision)
            || source.keys.is_empty()
            || source.keys.len() > 16
            || source.keys.iter().any(|key| {
                key.secret_env == epoch.signer_key_env || key.secret_env == epoch.credential_env
            })
            || !selected.insert((
                source.namespace.clone(),
                source.tenant.clone(),
                source.export_profile_id.clone(),
                source.issuer_epoch,
            ))
        {
            return Err("invalid federation finality source declaration".into());
        }
        let mut reserved_keys = authority_keys.clone();
        reserved_keys.push(
            read_secret(&epoch.signer_key_env)
                .ok_or("federation signing key unavailable during finality preparation")?,
        );
        reserved_keys.push(
            read_secret(&epoch.credential_env)
                .ok_or("federation credential unavailable during finality preparation")?,
        );
        let keys = resolve_keys(&source.keys, &reserved_keys, &mut read_secret)?;
        let verifier = HmacFederationFinalityVerifier::new_trusted(
            &source.source_id,
            &source.qualification_ref,
            &source.verifier_revision,
            keys,
        )
        .map_err(|_| "invalid federation finality verifier")?;
        installations.push(TrustedFederationFinalityInstallation {
            namespace: source.namespace.clone(),
            tenant: source.tenant.clone(),
            profile_id: source.export_profile_id.clone(),
            issuer_epoch: source.issuer_epoch,
            verifier: Arc::new(verifier),
        });
    }
    Ok(installations)
}

fn resolve_keys(
    declarations: &[ReconciliationKeyConfig],
    authority_keys: &[Zeroizing<String>],
    read_secret: &mut impl FnMut(&str) -> Option<Zeroizing<String>>,
) -> Result<std::collections::BTreeMap<String, Vec<u8>>, String> {
    let mut keys = std::collections::BTreeMap::new();
    for key in declarations {
        if !text(&key.id)
            || !key.secret_env.starts_with("ACTEON_FEDERATION_FINALITY_")
            || !key
                .secret_env
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            || key.secret_env.len() > 256
            || keys.contains_key(&key.id)
        {
            return Err("invalid federation finality key reference".into());
        }
        let encoded = read_secret(&key.secret_env)
            .ok_or("federation finality verification key unavailable")?;
        if !(64..=2048).contains(&encoded.len()) {
            return Err("invalid federation finality key material".into());
        }
        let decoded = Zeroizing::new(
            hex::decode(encoded.as_bytes())
                .map_err(|_| "invalid federation finality key material")?,
        );
        if !(32..=1024).contains(&decoded.len())
            || authority_keys.iter().any(|authority| {
                authority.as_bytes() == decoded.as_slice()
                    || authority.as_bytes() == encoded.as_bytes()
                    || hex::decode(authority.as_bytes())
                        .is_ok_and(|bytes| Zeroizing::new(bytes).as_slice() == decoded.as_slice())
            })
        {
            return Err("federation finality requires dedicated key material".into());
        }
        keys.insert(key.id.clone(), decoded.to_vec());
    }
    Ok(keys)
}

fn text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.trim() == value
        && value != "*"
        && !value.chars().any(char::is_control)
}
