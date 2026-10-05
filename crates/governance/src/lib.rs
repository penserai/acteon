//! Durable coordination between authority changes and effect starts.
//!
//! Exact root-profile permits share this coordinator with effect registration.
//! Trusted adapters still authenticate identity, grants and representation.
//! Qualified gateway/server execution profiles use these primitives; effect
//! coverage remains explicitly bounded by each trusted adapter.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use acteon_core::ResourceRef;
use acteon_state::{CasResult, KeyKind, StateKey, StateStore};
use serde::{Deserialize, Serialize};

mod budget;
pub mod configuration;
pub mod context;
pub mod control;
pub mod credential;
pub mod permit;
mod scope;
mod upgrade;
pub use budget::{RootBudget, RootBudgetLimits, RootReservation};
pub use scope::ScopePurpose;
pub use upgrade::{ScopeUpgradePlan, ScopeUpgradeReport};

const FORMAT: u32 = 8;
const MAX_ATTEMPT_RESOURCES: usize = 16;
const RETRIES: usize = 32;
const CONTROL_RECORD_RESERVE: usize = 16;
const CONTROL_BYTE_RESERVE: usize = 8192;

/// Non-expiring state kind. Never delete/recreate the coordinator to reopen it.
pub const COORDINATOR_KIND: &str = "governance_coordinator";

/// Persisted bounds shared by all instances using a coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorLimits {
    /// Maximum in-flight or uncertain attempts. Uncertainty retains capacity.
    pub max_active: usize,
    /// Retained permit/root/start/change records; the limit refuses new work.
    pub max_records: usize,
    /// Maximum JSON state size, checked before every mutation.
    pub max_bytes: usize,
}
impl Default for CoordinatorLimits {
    fn default() -> Self {
        Self {
            max_active: 128,
            max_records: 4096,
            max_bytes: 256 * 1024,
        }
    }
}

/// Restriction changes use stable IDs and are idempotent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthorityChange {
    /// Only created by trusted virgin-scope reservation, never generic change.
    ReserveScope { purpose: ScopePurpose },
    /// Refuse new starts targeting this exact resource reference.
    CloseResource { resource: ResourceRef },
    /// Remove this resource restriction; other restrictions remain effective.
    ReopenResource { resource: ResourceRef },
    /// Refuse this subject's subsequent starts.
    RevokeSubject { subject: String },
    /// Only publish through the bounded trusted issuance entrypoint.
    PublishPermit { permit: permit::ExecutionPermit },
    PublishCredentialConfiguration {
        configuration: configuration::CredentialConfiguration,
    },
    PublishCredential {
        credential: credential::CredentialAuthority,
    },
    RevokeCredential {
        credential_id: String,
        expected_revision: u64,
    },
    RevokePermit {
        permit_id: String,
        expected_revision: u64,
    },
}

/// Start state is honest about effects whose settlement is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    InFlight,
    Settled,
    Uncertain,
}

/// An immutable registered attempt and its independently settled status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRecord {
    pub subject: String,
    #[serde(deserialize_with = "deserialize_resources")]
    pub resources: BTreeSet<ResourceRef>,
    pub reservation: Option<RootReservation>,
    /// Digest supplied by a trusted adapter; identical IDs cannot change input.
    pub request_digest: String,
    pub authority: AuthorityStamp,
    pub token: String,
    pub status: AttemptStatus,
    /// Digest-pinned retained evidence. It is not permission to send again.
    pub evidence: Option<AttemptEvidenceReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptEvidenceReference {
    /// Scoped by this coordinator; resolved through a qualified effect adapter.
    pub id: String,
    pub digest: String,
}

fn valid_evidence(evidence: &AttemptEvidenceReference) -> bool {
    valid_text(&evidence.id)
        && evidence.digest.len() == 64
        && evidence
            .digest
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Durable control event/outbox, persisted atomically with its restriction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeRecord {
    pub change: AuthorityChange,
    pub actor: String,
    pub reason: String,
    pub generation: u64,
    /// Secondary audit/index/intervention delivery remains pending until acked.
    pub pending: bool,
}

/// Authority identity prevents stale contexts matching a recreated coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityStamp {
    pub incarnation: String,
    pub generation: u64,
}

/// Authoritative state, loaded with its `StateStore` CAS version separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorSnapshot {
    schema_version: u32,
    pub incarnation: String,
    pub purpose: ScopePurpose,
    pub namespace: String,
    pub tenant: String,
    pub limits: CoordinatorLimits,
    pub generation: u64,
    pub closed_resources: BTreeSet<ResourceRef>,
    pub revoked_subjects: BTreeSet<String>,
    pub starts: BTreeMap<String, StartRecord>,
    pub roots: BTreeMap<String, RootBudget>,
    pub changes: BTreeMap<String, ChangeRecord>,
    pub permits: BTreeMap<String, permit::PermitRecord>,
    pub credentials: BTreeMap<String, credential::CredentialRecord>,
    pub credential_configurations: BTreeMap<String, configuration::CredentialConfigurationRecord>,
}

impl CoordinatorSnapshot {
    fn record_count(&self) -> usize {
        self.roots.len()
            + self.starts.len()
            + self.changes.len()
            + self.permits.len()
            + self.credentials.len()
            + self.credential_configurations.len()
    }
    #[must_use]
    pub fn stamp(&self) -> AuthorityStamp {
        AuthorityStamp {
            incarnation: self.incarnation.clone(),
            generation: self.generation,
        }
    }
}

/// Replayed registration never authorizes another external effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartRegistration {
    New(StartRecord),
    Existing(StartRecord),
}

/// Trusted evaluated effect descriptor. Resources are a complete exact set;
/// adapters must validate permissions and lineage before submitting it.
pub struct AttemptRequest<'a> {
    pub id: &'a str,
    pub subject: &'a str,
    pub resources: &'a [ResourceRef],
    pub request_digest: &'a str,
    pub expected_authority: &'a AuthorityStamp,
    pub reservation: Option<RootReservation>,
    /// Trusted current time, checked against root deadline for fresh starts.
    pub now_ms: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum CoordinationError {
    #[error(transparent)]
    State(#[from] acteon_state::StateError),
    #[error("invalid coordinator state or request: {0}")]
    Invalid(String),
    #[error("authority changed; reevaluation required")]
    StaleAuthority,
    #[error("subject or destination is restricted")]
    Restricted,
    #[error("coordinator capacity exhausted")]
    Capacity,
    #[error("root attempt units exhausted")]
    BudgetExhausted,
    #[error("root concurrency exhausted")]
    ConcurrencyExhausted,
    #[error("root deadline exceeded")]
    DeadlineExceeded,
    #[error("execution permit denied: {0}")]
    PermitDenied(permit::PermitDenial),
    #[error("idempotency key conflicts with its original request")]
    Conflict,
    #[error("coordinator CAS contention; retry observation")]
    Contention,
}

/// Multiple instances serialize starts and authority mutations through one CAS.
#[derive(Clone)]
pub struct AuthorityCoordinator {
    store: Arc<dyn StateStore>,
    key: StateKey,
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control)
}

fn deserialize_resources<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeSet<ResourceRef>, D::Error> {
    let resources = Vec::<ResourceRef>::deserialize(deserializer)?;
    let size = resources.len();
    let set: BTreeSet<_> = resources.into_iter().collect();
    if size == 0 || size > MAX_ATTEMPT_RESOURCES || set.len() != size {
        return Err(serde::de::Error::custom(
            "invalid or duplicate attempt resources",
        ));
    }
    Ok(set)
}

impl AuthorityCoordinator {
    fn valid_start_accounting(&self, state: &CoordinatorSnapshot) -> bool {
        let mut totals: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for (id, start) in &state.starts {
            if ![
                id.as_str(),
                start.subject.as_str(),
                start.request_digest.as_str(),
            ]
            .into_iter()
            .all(valid_text)
                || uuid::Uuid::parse_str(&start.token).map_or(true, |id| id.is_nil())
                || start.resources.is_empty()
                || start.resources.len() > MAX_ATTEMPT_RESOURCES
                || start
                    .resources
                    .iter()
                    .any(|r| self.validate_resource_scope(r).is_err())
                || start.authority.incarnation != state.incarnation
                || start.authority.generation == 0
                || start.authority.generation > state.generation
                || start.evidence.as_ref().is_some_and(|e| !valid_evidence(e))
            {
                return false;
            }
            if let Some(reservation) = &start.reservation {
                if reservation.units == 0 || !state.roots.contains_key(&reservation.root_id) {
                    return false;
                }
                let total = totals.entry(&reservation.root_id).or_default();
                let Some(spent) = total.0.checked_add(reservation.units) else {
                    return false;
                };
                total.0 = spent;
                if start.status != AttemptStatus::Settled {
                    total.1 += 1;
                }
            }
        }
        state.roots.iter().all(|(id, root)| {
            valid_text(id)
                && valid_text(&root.owner_subject)
                && root.limits.max_units > 0
                && root.limits.max_concurrent > 0
                && root.limits.deadline_ms > 0
                && root.limits.max_concurrent
                    <= u64::try_from(state.limits.max_active).unwrap_or(u64::MAX)
                && root.spent_units <= root.limits.max_units
                && root.active_attempts <= root.limits.max_concurrent
                && totals.get(id.as_str()).copied().unwrap_or_default()
                    == (root.spent_units, root.active_attempts)
        })
    }
    pub async fn initialize(
        store: Arc<dyn StateStore>,
        namespace: &str,
        tenant: &str,
        limits: CoordinatorLimits,
    ) -> Result<Self, CoordinationError> {
        if !valid_text(namespace)
            || !valid_text(tenant)
            || namespace.contains(':')
            || tenant.contains(':')
            || limits.max_active == 0
            || limits.max_records <= limits.max_active.saturating_add(CONTROL_RECORD_RESERVE)
            || limits.max_bytes <= CONTROL_BYTE_RESERVE * 2
        {
            return Err(CoordinationError::Invalid("scope or limits".into()));
        }
        let key = StateKey::new(
            namespace,
            tenant,
            KeyKind::Custom(COORDINATOR_KIND.into()),
            "authority",
        );
        let initial = CoordinatorSnapshot {
            schema_version: FORMAT,
            purpose: ScopePurpose::Unclaimed,
            incarnation: uuid::Uuid::new_v4().to_string(),
            namespace: namespace.into(),
            tenant: tenant.into(),
            limits: limits.clone(),
            generation: 1,
            closed_resources: BTreeSet::new(),
            revoked_subjects: BTreeSet::new(),
            starts: BTreeMap::new(),
            roots: BTreeMap::new(),
            changes: BTreeMap::new(),
            permits: BTreeMap::new(),
            credentials: BTreeMap::new(),
            credential_configurations: BTreeMap::new(),
        };
        let coordinator = Self { store, key };
        let encoded = Self::encode(&initial)?;
        coordinator
            .store
            .check_and_set(&coordinator.key, &encoded, None)
            .await?;
        let (state, _) = coordinator.load().await?;
        if state.limits != limits {
            return Err(CoordinationError::Invalid(
                "instances disagree on persisted limits".into(),
            ));
        }
        Ok(coordinator)
    }

    /// Attach to existing authority state. Missing state fails closed; only the
    /// trusted one-time initialize operation may create the record.
    pub async fn connect(
        store: Arc<dyn StateStore>,
        namespace: &str,
        tenant: &str,
    ) -> Result<Self, CoordinationError> {
        if !valid_text(namespace)
            || !valid_text(tenant)
            || namespace.contains(':')
            || tenant.contains(':')
        {
            return Err(CoordinationError::Invalid("scope".into()));
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
        coordinator.load().await?;
        Ok(coordinator)
    }

    fn validate_resource_scope(&self, resource: &ResourceRef) -> Result<(), CoordinationError> {
        if resource.namespace() != self.key.namespace.as_str()
            || resource.tenant() != self.key.tenant.as_str()
        {
            return Err(CoordinationError::Invalid(
                "resource scope differs from coordinator".into(),
            ));
        }
        Ok(())
    }

    fn encode(state: &CoordinatorSnapshot) -> Result<String, CoordinationError> {
        if !Self::valid_scope_purpose(state) {
            return Err(CoordinationError::Invalid(
                "scope purpose forbids this state".into(),
            ));
        }
        let raw =
            serde_json::to_string(state).map_err(|e| CoordinationError::Invalid(e.to_string()))?;
        if raw.len() > state.limits.max_bytes {
            return Err(CoordinationError::Capacity);
        }
        Ok(raw)
    }

    async fn load(&self) -> Result<(CoordinatorSnapshot, u64), CoordinationError> {
        let (raw, version) = self.store.get_versioned(&self.key).await?.ok_or_else(|| {
            CoordinationError::Invalid("coordinator disappeared; refusing recreation".into())
        })?;
        Ok((self.decode(&raw)?, version))
    }

    fn decode(&self, raw: &str) -> Result<CoordinatorSnapshot, CoordinationError> {
        let state: CoordinatorSnapshot =
            serde_json::from_str(raw).map_err(|e| CoordinationError::Invalid(e.to_string()))?;
        if !Self::valid_scope_purpose(&state)
            || state.schema_version != FORMAT
            || state.namespace != self.key.namespace.as_str()
            || state.tenant != self.key.tenant.as_str()
            || uuid::Uuid::parse_str(&state.incarnation).map_or(true, |id| id.is_nil())
            || state.limits.max_active == 0
            || state.limits.max_records
                <= state
                    .limits
                    .max_active
                    .saturating_add(CONTROL_RECORD_RESERVE)
            || state.limits.max_bytes <= CONTROL_BYTE_RESERVE * 2
            || state
                .closed_resources
                .iter()
                .any(|r| self.validate_resource_scope(r).is_err())
            || !self.valid_start_accounting(&state)
            || !self.valid_permit_history(&state)
            || !self.valid_credential_history(&state)
            || state.changes.values().any(|record| match &record.change {
                AuthorityChange::ReserveScope { purpose } => !purpose.valid(),
                AuthorityChange::CloseResource { resource }
                | AuthorityChange::ReopenResource { resource } => {
                    self.validate_resource_scope(resource).is_err()
                }
                AuthorityChange::RevokeSubject { .. } => false,
                AuthorityChange::PublishCredentialConfiguration { configuration } => {
                    !self.valid_credential_configuration(configuration)
                }
                AuthorityChange::PublishCredential { credential } => {
                    !self.valid_credential(credential)
                }
                AuthorityChange::RevokeCredential {
                    credential_id,
                    expected_revision,
                } => !valid_text(credential_id) || *expected_revision == 0,
                AuthorityChange::PublishPermit { permit } => !self.valid_permit(permit),
                AuthorityChange::RevokePermit {
                    permit_id,
                    expected_revision,
                } => !valid_text(permit_id) || *expected_revision == 0,
            })
            || state.generation == 0
            || state.record_count() > state.limits.max_records
            || state
                .starts
                .values()
                .filter(|s| s.status != AttemptStatus::Settled)
                .count()
                > state.limits.max_active
            || raw.len() > state.limits.max_bytes
        {
            return Err(CoordinationError::Invalid("format, scope, or size".into()));
        }
        Ok(state)
    }

    pub async fn snapshot(&self) -> Result<CoordinatorSnapshot, CoordinationError> {
        Ok(self.load().await?.0)
    }

    /// The expected authority stamp must cover the adapter's prior authority evaluation.
    /// A New result linearizes the start; Existing is observation only.
    pub async fn register_start(
        &self,
        id: &str,
        subject: &str,
        resource: &ResourceRef,
        request_digest: &str,
        expected_authority: &AuthorityStamp,
    ) -> Result<StartRegistration, CoordinationError> {
        self.register_attempt(AttemptRequest {
            id,
            subject,
            resources: std::slice::from_ref(resource),
            request_digest,
            expected_authority,
            reservation: None,
            now_ms: 0,
        })
        .await
    }

    /// Atomically register all resources and reserve root units/concurrency.
    /// Every retry/descendant uses a fresh attempt ID; replay is observation.
    pub async fn register_attempt(
        &self,
        request: AttemptRequest<'_>,
    ) -> Result<StartRegistration, CoordinationError> {
        self.register_attempt_checked(request, None).await
    }

    async fn register_attempt_checked(
        &self,
        request: AttemptRequest<'_>,
        permit_check: Option<&permit::PermittedAttempt<'_>>,
    ) -> Result<StartRegistration, CoordinationError> {
        let AttemptRequest {
            id,
            subject,
            resources,
            request_digest,
            expected_authority,
            reservation,
            now_ms,
        } = request;
        if ![id, subject, request_digest].into_iter().all(valid_text) {
            return Err(CoordinationError::Invalid("start fields".into()));
        }
        if resources.is_empty()
            || resources.len() > MAX_ATTEMPT_RESOURCES
            || now_ms < 0
            || reservation
                .as_ref()
                .is_some_and(|r| !valid_text(&r.root_id) || r.units == 0)
        {
            return Err(CoordinationError::Invalid(
                "attempt resources or reservation".into(),
            ));
        }
        let resource_count = resources.len();
        let resources: BTreeSet<_> = resources.iter().cloned().collect();
        if resources.len() != resource_count {
            return Err(CoordinationError::Invalid("duplicate resources".into()));
        }
        for resource in &resources {
            self.validate_resource_scope(resource)?;
        }
        let token = uuid::Uuid::new_v4().to_string();
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            // A replay may observe an older generation, but never a different
            // coordinator incarnation after trusted disaster recovery.
            if state.incarnation != expected_authority.incarnation {
                return Err(CoordinationError::StaleAuthority);
            }
            if let Some(old) = state.starts.get(id) {
                if old.subject != subject
                    || old.resources != resources
                    || old.reservation != reservation
                    || old.request_digest != request_digest
                {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(StartRegistration::Existing(old.clone()));
            }
            if state.stamp() != *expected_authority {
                return Err(CoordinationError::StaleAuthority);
            }
            if resources.iter().any(|r| state.closed_resources.contains(r))
                || state.revoked_subjects.contains(subject)
            {
                return Err(CoordinationError::Restricted);
            }
            let checked_now =
                permit_check.map_or(now_ms, |check| check.clock.now().timestamp_millis());
            if let Some(check) = permit_check {
                permit::evaluate(&state, check, checked_now)?;
            }
            if state.record_count() >= state.limits.max_records - CONTROL_RECORD_RESERVE
                || state
                    .starts
                    .values()
                    .filter(|r| r.status != AttemptStatus::Settled)
                    .count()
                    >= state.limits.max_active
            {
                return Err(CoordinationError::Capacity);
            }
            if let Some(reservation) = &reservation {
                budget::reserve_root(&mut state, reservation, checked_now)?;
            }
            let record = StartRecord {
                subject: subject.into(),
                resources: resources.clone(),
                reservation: reservation.clone(),
                request_digest: request_digest.into(),
                authority: state.stamp(),
                token: token.clone(),
                status: AttemptStatus::InFlight,
                evidence: None,
            };
            state.starts.insert(id.into(), record.clone());
            // Effect admissions cannot consume the reserved control-plane space.
            if Self::encode(&state)?.len() > state.limits.max_bytes - CONTROL_BYTE_RESERVE {
                return Err(CoordinationError::Capacity);
            }
            if self.commit(&state, version).await? {
                return Ok(StartRegistration::New(record));
            }
        }
        Err(CoordinationError::Contention)
    }

    fn validate_change(&self, change: &AuthorityChange) -> Result<(), CoordinationError> {
        match change {
            AuthorityChange::CloseResource { resource }
            | AuthorityChange::ReopenResource { resource } => {
                self.validate_resource_scope(resource)?;
            }
            AuthorityChange::RevokeSubject { subject } if !valid_text(subject) => {
                return Err(CoordinationError::Invalid("subject".into()));
            }
            AuthorityChange::RevokeSubject { .. } => {}
            AuthorityChange::ReserveScope { .. }
            | AuthorityChange::PublishPermit { .. }
            | AuthorityChange::PublishCredential { .. }
            | AuthorityChange::PublishCredentialConfiguration { .. } => {
                return Err(CoordinationError::Invalid(
                    "use bounded permit publication".into(),
                ));
            }
            AuthorityChange::RevokePermit {
                permit_id,
                expected_revision,
            }
            | AuthorityChange::RevokeCredential {
                credential_id: permit_id,
                expected_revision,
            } => {
                if !valid_text(permit_id) || *expected_revision == 0 {
                    return Err(CoordinationError::Invalid("permit revocation".into()));
                }
            }
        }
        Ok(())
    }

    /// The restriction and pending control event are one authoritative write.
    pub async fn change(
        &self,
        id: &str,
        change: AuthorityChange,
        actor: &str,
        reason: &str,
    ) -> Result<ChangeRecord, CoordinationError> {
        self.change_internal(id, change, actor, reason, None).await
    }

    async fn change_internal(
        &self,
        id: &str,
        change: AuthorityChange,
        actor: &str,
        reason: &str,
        authorization: Option<&control::ControlChangeAuthorization<'_>>,
    ) -> Result<ChangeRecord, CoordinationError> {
        self.validate_change(&change)?;
        if ![id, actor, reason].into_iter().all(valid_text) {
            return Err(CoordinationError::Invalid("change fields".into()));
        }
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            if let Some(authorization) = authorization {
                authorization.validate(self, &state, &change)?;
            }
            if let Some(old) = state.changes.get(id) {
                if old.change != change || old.actor != actor || old.reason != reason {
                    return Err(CoordinationError::Conflict);
                }
                return Ok(old.clone());
            }
            if state.record_count() >= state.limits.max_records {
                return Err(CoordinationError::Capacity);
            }
            state.generation = state
                .generation
                .checked_add(1)
                .ok_or(CoordinationError::Capacity)?;
            match &change {
                AuthorityChange::CloseResource { resource } => {
                    state.closed_resources.insert(resource.clone());
                }
                AuthorityChange::ReopenResource { resource } => {
                    state.closed_resources.remove(resource);
                }
                AuthorityChange::RevokeSubject { subject } => {
                    state.revoked_subjects.insert(subject.clone());
                }
                AuthorityChange::RevokePermit {
                    permit_id,
                    expected_revision,
                } => {
                    let record = state
                        .permits
                        .get_mut(permit_id)
                        .ok_or(CoordinationError::Conflict)?;
                    if record.permit.revision != *expected_revision {
                        return Err(CoordinationError::Conflict);
                    }
                    record.revoked = true;
                }
                AuthorityChange::RevokeCredential {
                    credential_id,
                    expected_revision,
                } => {
                    let record = state
                        .credentials
                        .get_mut(credential_id)
                        .ok_or(CoordinationError::Conflict)?;
                    if record.authority.ceiling.revision != *expected_revision {
                        return Err(CoordinationError::Conflict);
                    }
                    record.revoked = true;
                }
                AuthorityChange::ReserveScope { .. }
                | AuthorityChange::PublishPermit { .. }
                | AuthorityChange::PublishCredential { .. }
                | AuthorityChange::PublishCredentialConfiguration { .. } => {
                    unreachable!("publication uses its bounded entrypoint")
                }
            }
            let record = ChangeRecord {
                change: change.clone(),
                actor: actor.into(),
                reason: reason.into(),
                generation: state.generation,
                pending: true,
            };
            state.changes.insert(id.into(), record.clone());
            if self.commit(&state, version).await? {
                return Ok(record);
            }
        }
        Err(CoordinationError::Contention)
    }

    pub async fn settle(
        &self,
        id: &str,
        token: &str,
        status: AttemptStatus,
    ) -> Result<(), CoordinationError> {
        self.settle_inner(id, token, status, None).await
    }

    /// Trusted adapter persists evidence first, then pins its immutable reference
    /// and settles accounting through one CAS. Lost acknowledgment is observation.
    pub async fn settle_with_evidence(
        &self,
        id: &str,
        token: &str,
        status: AttemptStatus,
        evidence: AttemptEvidenceReference,
    ) -> Result<(), CoordinationError> {
        if !valid_evidence(&evidence) {
            return Err(CoordinationError::Invalid("attempt evidence".into()));
        }
        self.settle_inner(id, token, status, Some(evidence)).await
    }

    async fn settle_inner(
        &self,
        id: &str,
        token: &str,
        status: AttemptStatus,
        evidence: Option<AttemptEvidenceReference>,
    ) -> Result<(), CoordinationError> {
        if status == AttemptStatus::InFlight {
            return Err(CoordinationError::Invalid(
                "settlement cannot restart an effect".into(),
            ));
        }
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            let record = state
                .starts
                .get_mut(id)
                .ok_or(CoordinationError::Conflict)?;
            if record.token != token {
                return Err(CoordinationError::Conflict);
            }
            if let Some(proposed) = &evidence
                && record
                    .evidence
                    .as_ref()
                    .is_some_and(|prior| prior != proposed)
            {
                return Err(CoordinationError::Conflict);
            }
            if record.status == status && (evidence.is_none() || record.evidence == evidence) {
                return Ok(());
            }
            if record.status == AttemptStatus::Settled && status != AttemptStatus::Settled {
                return Err(CoordinationError::Conflict);
            }
            let was_settled = record.status == AttemptStatus::Settled;
            record.status = status;
            if evidence.is_some() {
                record.evidence.clone_from(&evidence);
            }
            if !was_settled
                && status == AttemptStatus::Settled
                && let Some(reservation) = &record.reservation
            {
                let root = state
                    .roots
                    .get_mut(&reservation.root_id)
                    .ok_or(CoordinationError::Conflict)?;
                root.active_attempts = root
                    .active_attempts
                    .checked_sub(1)
                    .ok_or(CoordinationError::Conflict)?;
            }
            if self.commit(&state, version).await? {
                return Ok(());
            }
        }
        Err(CoordinationError::Contention)
    }

    /// Ack only secondary delivery; the restriction and history remain persisted.
    pub async fn acknowledge_change(&self, id: &str) -> Result<(), CoordinationError> {
        for _ in 0..RETRIES {
            let (mut state, version) = self.load().await?;
            let record = state
                .changes
                .get_mut(id)
                .ok_or(CoordinationError::Conflict)?;
            if !record.pending {
                return Ok(());
            }
            record.pending = false;
            if self.commit(&state, version).await? {
                return Ok(());
            }
        }
        Err(CoordinationError::Contention)
    }

    async fn commit(
        &self,
        state: &CoordinatorSnapshot,
        version: u64,
    ) -> Result<bool, CoordinationError> {
        let raw = Self::encode(state)?;
        Ok(matches!(
            self.store
                .compare_and_swap(&self.key, version, &raw, None)
                .await?,
            CasResult::Ok
        ))
    }
}
