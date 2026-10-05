//! Explicit, reviewed authority protocol cutover. Startup never invokes this.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use acteon_state::{CasResult, KeyKind, StateKey, StateStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AuthorityChange, AuthorityCoordinator, AuthorityStamp, COORDINATOR_KIND, ChangeRecord,
    CoordinationError, FORMAT, ScopePurpose, valid_text,
};

/// Safe inspection data for a proposed cutover. No credentials or raw state.
#[derive(Debug, Clone, Serialize)]
pub struct ScopeUpgradeReport {
    pub namespace: String,
    pub tenant: String,
    pub from_protocol: u32,
    pub to_protocol: u32,
    pub purpose: ScopePurpose,
    pub actor: String,
    pub reason: String,
    pub closed_resources: usize,
    pub revoked_subjects: usize,
    pub original_authority: AuthorityStamp,
    pub proposed_authority: AuthorityStamp,
    pub permits: usize,
    pub credentials: usize,
    pub roots: usize,
    pub starts: usize,
    pub unsettled_starts: usize,
    pub retained_changes: usize,
    /// Binds the exact observed record/version and proposed policy/audit inputs.
    pub review_digest: String,
}

/// Private construction, no deserializer. The host must stop/drain old workers
/// before applying; protocol fencing cannot cancel already registered effects.
pub struct ScopeUpgradePlan {
    coordinator: AuthorityCoordinator,
    version: u64,
    original_digest: String,
    proposed: String,
    report: ScopeUpgradeReport,
}

impl ScopeUpgradePlan {
    #[must_use]
    pub fn report(&self) -> &ScopeUpgradeReport {
        &self.report
    }

    /// Apply only the reviewed record by CAS. Exact replay reconciles a lost
    /// acknowledgement. Changes since review conflict; no implicit reevaluation.
    pub async fn apply(&self, reviewed_digest: &str) -> Result<bool, CoordinationError> {
        if reviewed_digest != self.report.review_digest {
            return Err(CoordinationError::Conflict);
        }
        let (raw, version) = self
            .coordinator
            .store
            .get_versioned(&self.coordinator.key)
            .await?
            .ok_or_else(|| CoordinationError::Invalid("authority missing during cutover".into()))?;
        if raw == self.proposed {
            return Ok(false);
        }
        if version != self.version || digest(&raw) != self.original_digest {
            return Err(CoordinationError::Conflict);
        }
        match self
            .coordinator
            .store
            .compare_and_swap(&self.coordinator.key, version, &self.proposed, None)
            .await
        {
            Ok(CasResult::Ok) => Ok(true),
            Ok(CasResult::Conflict { current_value, .. })
                if current_value.as_deref() == Some(&self.proposed) =>
            {
                Ok(false)
            }
            Ok(CasResult::Conflict { .. }) => Err(CoordinationError::Conflict),
            Err(error) => {
                // Acknowledgement may be lost after the authoritative write.
                if self
                    .coordinator
                    .store
                    .get(&self.coordinator.key)
                    .await?
                    .is_some_and(|value| value == self.proposed)
                {
                    Ok(false)
                } else {
                    Err(error.into())
                }
            }
        }
    }
}
fn digest(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

impl AuthorityCoordinator {
    /// Read-only preparation for protocol 7 or unclaimed protocol 8. Preserve
    /// incarnation, all authority history, roots, spending and reconciliation
    /// records. Scope purpose classification must match the complete state.
    pub async fn plan_scope_upgrade(
        store: Arc<dyn StateStore>,
        namespace: &str,
        tenant: &str,
        purpose: ScopePurpose,
        actor: &str,
        reason: &str,
    ) -> Result<ScopeUpgradePlan, CoordinationError> {
        if ![namespace, tenant, actor, reason]
            .into_iter()
            .all(valid_text)
            || namespace.contains(':')
            || tenant.contains(':')
            || !purpose.valid()
            || purpose == ScopePurpose::Unclaimed
        {
            return Err(CoordinationError::Invalid("scope cutover inputs".into()));
        }
        let coordinator = Self {
            store,
            key: StateKey::new(
                namespace,
                tenant,
                KeyKind::Custom(COORDINATOR_KIND.into()),
                "authority",
            ),
        };
        let (raw, version) = coordinator
            .store
            .get_versioned(&coordinator.key)
            .await?
            .ok_or_else(|| {
                CoordinationError::Invalid("authority missing; cutover cannot bootstrap".into())
            })?;
        let (mut state, from_protocol) = coordinator.decode_upgrade_source(&raw)?;
        if state.purpose != ScopePurpose::Unclaimed || state.changes.contains_key("scope-purpose") {
            return Err(CoordinationError::Conflict);
        }
        let original_authority = state.stamp();
        state.purpose = purpose.clone();
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or(CoordinationError::Capacity)?;
        state.changes.insert(
            "scope-purpose".into(),
            ChangeRecord {
                change: AuthorityChange::ReserveScope {
                    purpose: purpose.clone(),
                },
                actor: actor.into(),
                reason: reason.into(),
                generation: state.generation,
                pending: true,
            },
        );
        let proposed = Self::encode(&state)?;
        // Run the same full reconstruction and accounting checks as every load.
        coordinator.decode(&proposed)?;
        let original_digest = digest(&raw);
        let review_inputs = serde_json::to_string(&(
            "acteon.scope_upgrade.review.v1",
            namespace,
            tenant,
            version,
            &original_digest,
            digest(&proposed),
        ))
        .map_err(|_| CoordinationError::Invalid("cutover review inputs".into()))?;
        let report = ScopeUpgradeReport {
            namespace: namespace.into(),
            tenant: tenant.into(),
            from_protocol,
            to_protocol: FORMAT,
            purpose,
            actor: actor.into(),
            reason: reason.into(),
            closed_resources: state.closed_resources.len(),
            revoked_subjects: state.revoked_subjects.len(),
            original_authority,
            proposed_authority: state.stamp(),
            permits: state.permits.len(),
            credentials: state.credentials.len(),
            roots: state.roots.len(),
            starts: state.starts.len(),
            unsettled_starts: state
                .starts
                .values()
                .filter(|s| s.status != crate::AttemptStatus::Settled)
                .count(),
            retained_changes: state.changes.len() - 1,
            review_digest: digest(&review_inputs),
        };
        Ok(ScopeUpgradePlan {
            coordinator,
            version,
            original_digest,
            proposed,
            report,
        })
    }
}

// Exact protocol-7 shape. Required ownership remains mandatory for ordinary
// protocol-8 decoding; this compatibility reader exists only in explicit cutover.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyScope {
    schema_version: u32,
    incarnation: String,
    namespace: String,
    tenant: String,
    limits: crate::CoordinatorLimits,
    generation: u64,
    closed_resources: BTreeSet<acteon_core::ResourceRef>,
    revoked_subjects: BTreeSet<String>,
    starts: BTreeMap<String, crate::StartRecord>,
    roots: BTreeMap<String, crate::RootBudget>,
    changes: BTreeMap<String, ChangeRecord>,
    permits: BTreeMap<String, crate::permit::PermitRecord>,
    credentials: BTreeMap<String, crate::credential::CredentialRecord>,
    credential_configurations:
        BTreeMap<String, crate::configuration::CredentialConfigurationRecord>,
}
impl AuthorityCoordinator {
    fn decode_upgrade_source(
        &self,
        raw: &str,
    ) -> Result<(crate::CoordinatorSnapshot, u32), CoordinationError> {
        let value: serde_json::Value = serde_json::from_str(raw)
            .map_err(|_| CoordinationError::Invalid("invalid source authority".into()))?;
        match value["schema_version"].as_u64() {
            Some(7) => {
                let legacy: LegacyScope = serde_json::from_str(raw).map_err(|_| {
                    CoordinationError::Invalid("invalid protocol-7 authority".into())
                })?;
                if legacy.schema_version != 7 || raw.len() > legacy.limits.max_bytes {
                    return Err(CoordinationError::Invalid("source protocol or size".into()));
                }
                let state = crate::CoordinatorSnapshot {
                    schema_version: FORMAT,
                    purpose: ScopePurpose::Unclaimed,
                    incarnation: legacy.incarnation,
                    namespace: legacy.namespace,
                    tenant: legacy.tenant,
                    limits: legacy.limits,
                    generation: legacy.generation,
                    closed_resources: legacy.closed_resources,
                    revoked_subjects: legacy.revoked_subjects,
                    starts: legacy.starts,
                    roots: legacy.roots,
                    changes: legacy.changes,
                    permits: legacy.permits,
                    credentials: legacy.credentials,
                    credential_configurations: legacy.credential_configurations,
                };
                let encoded = Self::encode(&state)?;
                Ok((self.decode(&encoded)?, 7))
            }
            Some(version) if version == u64::from(FORMAT) => Ok((self.decode(raw)?, FORMAT)),
            _ => Err(CoordinationError::Invalid(
                "cutover supports only protocol 7 or unclaimed 8".into(),
            )),
        }
    }
}
