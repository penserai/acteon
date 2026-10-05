//! Shared authentication-table freshness. This is not an execution permit profile.
use acteon_core::{PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef};
use acteon_governance::configuration::{CredentialConfiguration, CredentialConfigurationReference};
use acteon_governance::context::AcceptedEffect;
use acteon_governance::permit::PermitIssuanceCeiling;
use acteon_governance::{
    AuthorityCoordinator, AuthorityStamp, CoordinatorSnapshot, RootBudgetLimits,
};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::config::{AuthFileConfig, Grant};
use super::crypto::{ExposeSecret, SecretString};
use super::identity::CallerIdentity;
use super::role::Role;
use crate::config::AuthAuthorityConfig;

/// Host-created evidence of the exact tables used for authentication. Private
/// construction and no Deserialize implementation prevent request metadata from
/// establishing this binding. It is an observation, never a permanent grant.
#[derive(Debug, Clone)]
pub struct AuthenticatedConfiguration {
    reference: CredentialConfigurationReference,
    stamp: AuthorityStamp,
    principal: PrincipalIdentity,
    caller_id: String,
    auth_method: String,
}
impl AuthenticatedConfiguration {
    #[must_use]
    pub fn reference(&self) -> &CredentialConfigurationReference {
        &self.reference
    }
    #[must_use]
    pub fn stamp(&self) -> &AuthorityStamp {
        &self.stamp
    }
    #[must_use]
    pub fn principal(&self) -> &PrincipalIdentity {
        &self.principal
    }
    #[must_use]
    pub fn caller_id(&self) -> &str {
        &self.caller_id
    }
    #[must_use]
    pub fn auth_method(&self) -> &str {
        &self.auth_method
    }
}

/// Actual coordinator publisher/verifier, not a reload callback. The dedicated
/// control coordinator holds only this source's authentication epochs. It does
/// not contain resolved credential execution ceilings or work reservations.
pub struct AuthAuthority {
    coordinator: AuthorityCoordinator,
    source_id: String,
    fingerprint_key: SecretString,
    issuance: PermitIssuanceCeiling,
}
impl AuthAuthority {
    pub(super) fn control_scope(&self) -> (&str, &str) {
        let resource = &self.issuance.effects[0].resources[0];
        (resource.namespace(), resource.tenant())
    }

    pub fn new(
        coordinator: AuthorityCoordinator,
        config: &AuthAuthorityConfig,
        fingerprint_key: SecretString,
    ) -> Result<Self, String> {
        if config.source_id.is_empty()
            || config.source_id.len() > 512
            || config.source_id.trim() != config.source_id
            || config.source_id.chars().any(char::is_control)
            || fingerprint_key.expose_secret().len() < 32
        {
            return Err("invalid auth authority identity or fingerprint key".into());
        }
        let publisher = PrincipalIdentity::new(
            format!("acteon.auth.publisher/{}", config.source_id),
            PrincipalKind::System,
        )
        .map_err(|_| "invalid auth authority publisher")?;
        let resource = ResourceRef::new(
            ResourceKind::ExternalService,
            &config.namespace,
            &config.tenant,
            format!("auth-source/{}", config.source_id),
        )
        .map_err(|_| "invalid auth authority scope")?;
        Ok(Self {
            coordinator,
            source_id: config.source_id.clone(),
            fingerprint_key,
            issuance: PermitIssuanceCeiling {
                issuer: publisher.clone(),
                subjects: vec![publisher],
                effects: vec![AcceptedEffect {
                    operation: "auth.configuration.publish".into(),
                    resources: vec![resource],
                }],
                valid_from_ms: 0,
                limits: RootBudgetLimits {
                    max_units: 1,
                    max_concurrent: 1,
                    deadline_ms: i64::MAX,
                },
            },
        })
    }

    pub(super) async fn reserve_control_scope(&self) -> Result<(), String> {
        self.coordinator
            .reserve_scope(acteon_governance::ScopePurpose::AuthenticationControl {
                source_id: self.source_id.clone(),
            })
            .await
            .map_err(|_| "auth control scope is already owned or contains unclaimed work")?;
        Ok(())
    }

    fn validate_control_scope(&self, snapshot: &CoordinatorSnapshot) -> Result<(), String> {
        let resource = &self.issuance.effects[0].resources[0];
        if snapshot.purpose
            != (acteon_governance::ScopePurpose::AuthenticationControl {
                source_id: self.source_id.clone(),
            })
            || snapshot.namespace != resource.namespace()
            || snapshot.tenant != resource.tenant()
            || !snapshot.credentials.is_empty()
            || !snapshot.permits.is_empty()
            || !snapshot.roots.is_empty()
            || !snapshot.starts.is_empty()
            || !snapshot.closed_resources.is_empty()
            || snapshot
                .credential_configurations
                .keys()
                .any(|id| id != &self.source_id)
        {
            return Err("auth authority requires its own dedicated control scope".into());
        }
        Ok(())
    }

    /// Fingerprint decrypted security inputs with a separate shared secret. Raw
    /// secrets, password/key hashes and grant contents are never persisted here.
    pub(super) fn configuration(
        &self,
        config: &AuthFileConfig,
    ) -> Result<CredentialConfiguration, String> {
        let revision = config
            .authority_revision
            .filter(|r| *r > 0)
            .ok_or("auth authority requires a positive authority_revision")?;
        let mut users = Vec::new();
        for user in &config.users {
            let principal = user
                .principal
                .as_ref()
                .ok_or("auth authority requires stable principals")?;
            let role = Role::from_str_loose(&user.role).ok_or("invalid auth role")?;
            let mut entry = json!({
                "username": user.username, "principal": principal, "role": role,
                "password_hash": user.password_hash.expose_secret(),
                "grants": canonical_grants(&user.grants)?,
            });
            if let Some(id) = &user.authority_id {
                entry["authority_id"] = json!(id);
            }
            users.push(entry);
        }
        let mut keys = Vec::new();
        for key in &config.api_keys {
            let principal = key
                .principal
                .as_ref()
                .ok_or("auth authority requires stable principals")?;
            let role = Role::from_str_loose(&key.role).ok_or("invalid auth role")?;
            let mut entry = json!({
                "name": key.name, "principal": principal, "role": role,
                "key_hash": key.key_hash.expose_secret(), "grants": canonical_grants(&key.grants)?,
            });
            if let Some(id) = &key.authority_id {
                entry["authority_id"] = json!(id);
            }
            keys.push(entry);
        }
        sort_values(&mut users);
        sort_values(&mut keys);
        let scope = &self.issuance.effects[0].resources[0];
        let bytes = serde_json::to_vec(&json!({
            "format": "acteon.auth.configuration.v1", "source": self.source_id,
            "namespace": scope.namespace(), "tenant": scope.tenant(),
            "jwt_secret": config.settings.jwt_secret.expose_secret(),
            "jwt_expiry_seconds": config.settings.jwt_expiry_seconds,
            "users": users, "api_keys": keys,
        }))
        .map_err(|_| "invalid authentication configuration")?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.fingerprint_key.expose_secret().as_bytes())
                .map_err(|_| "invalid auth fingerprint key")?;
        mac.update(&bytes);
        Ok(CredentialConfiguration {
            source_id: self.source_id.clone(),
            revision,
            configuration_fingerprint: hex::encode(mac.finalize().into_bytes()),
            // Auth-source epochs are separate from per-effect credential projections.
            credentials: Vec::new(),
        })
    }

    pub(super) async fn publish(
        &self,
        config: &AuthFileConfig,
        scopes: &super::projection::ScopeReferences,
    ) -> Result<CredentialConfigurationReference, String> {
        let mut configuration = self.configuration(config)?;
        if !scopes.is_empty() {
            // Preserve the manifest wire format. The scope reference digest already
            // commits to the prepared policy; the private proof is not wire data.
            let manifest: Vec<_> = scopes
                .iter()
                .map(|(scope, binding)| (scope, &binding.reference))
                .collect();
            let bytes = serde_json::to_vec(&json!({
                "format": "acteon.auth.scope_manifest.v1",
                "authentication": configuration.configuration_fingerprint,
                "scopes": manifest,
            }))
            .map_err(|_| "invalid scope publication manifest")?;
            configuration.configuration_fingerprint = format!("{:x}", Sha256::digest(bytes));
        }
        let snapshot = self
            .coordinator
            .snapshot()
            .await
            .map_err(|_| "auth authority unavailable")?;
        self.validate_control_scope(&snapshot)?;
        let expected = snapshot
            .credential_configurations
            .get(&self.source_id)
            .map_or(0, |r| r.revision);
        let reference = configuration
            .reference(&snapshot.stamp())
            .map_err(|_| "invalid auth authority reference")?;
        // A stable ID reconciles lost acknowledgments. A conflicting fingerprint
        // is detected independently by the coordinator's same-version contract.
        let change_id = format!("auth/{}/{}", self.source_id, configuration.revision);
        self.coordinator
            .publish_credential_configuration(
                &change_id,
                &configuration,
                expected,
                &self.issuance,
                &snapshot.stamp(),
                "trusted authentication configuration",
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|_| "auth authority publication refused; reconcile configuration revision")?;
        Ok(reference)
    }

    pub(super) async fn verify(
        &self,
        reference: &CredentialConfigurationReference,
        identity: &CallerIdentity,
    ) -> Result<AuthenticatedConfiguration, String> {
        let snapshot = self
            .coordinator
            .snapshot()
            .await
            .map_err(|_| "auth authority unavailable")?;
        self.validate_control_scope(&snapshot)?;
        let current = snapshot
            .credential_configurations
            .get(&self.source_id)
            .ok_or("authentication configuration is not authoritative")?;
        let principal = identity
            .principal
            .as_ref()
            .ok_or("stable principal required")?;
        if reference.source_id != self.source_id
            || reference.incarnation != snapshot.incarnation
            || reference.revision != current.revision
            || reference.digest != current.digest
            || snapshot.revoked_subjects.contains(principal.id())
        {
            return Err("authentication configuration is stale or principal is disabled".into());
        }
        Ok(AuthenticatedConfiguration {
            reference: reference.clone(),
            stamp: snapshot.stamp(),
            principal: principal.clone(),
            caller_id: identity.id.clone(),
            auth_method: identity.auth_method.clone(),
        })
    }
}

fn sort_values(values: &mut [Value]) {
    values.sort_by_cached_key(Value::to_string);
}
pub(super) fn canonical_grants(grants: &[Grant]) -> Result<Vec<Value>, String> {
    let mut values = Vec::new();
    for grant in grants {
        let mut value = serde_json::to_value(grant).map_err(|_| "invalid auth grants")?;
        for field in ["tenants", "namespaces", "providers", "actions"] {
            let items = value[field].as_array_mut().ok_or("invalid auth grants")?;
            sort_values(items);
            items.dedup();
        }
        values.push(value);
    }
    sort_values(&mut values);
    values.dedup();
    Ok(values)
}
