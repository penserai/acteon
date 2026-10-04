//! Trusted, integrity-checked durable execution provenance.
//!
//! This is an internal trusted-adapter boundary, not a public authentication
//! API. Creating a root requires independently authenticated actor and evaluated
//! ceilings. Recovery verifies recorded provenance; it never authorizes an
//! effect. Every effect still requires current authority evaluation/registration.
use std::{collections::BTreeMap, sync::Arc};

use acteon_core::{PrincipalIdentity, ResourceRef};
use acteon_state::{KeyKind, StateKey, StateStore};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

use crate::{AuthorityCoordinator, AuthorityStamp, CoordinationError};

pub const CONTEXT_KIND: &str = "governance_execution_context";
const FORMAT: u32 = 1;
const MAX_BYTES: usize = 64 * 1024;
const MAX_EFFECTS: usize = 128;
const MAX_RESOURCES: usize = 16;
const MAX_KEYS: usize = 8;

/// A complete exact operation/resource tuple. Entries cannot be combined into
/// a cross-product of authority. These are accepted ceilings, not permits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedEffect {
    pub operation: String,
    pub resources: Vec<ResourceRef>,
}

/// Opaque reference, not a bearer credential. Hosts must additionally validate
/// execution ownership before using a reference received from an untrusted tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExecutionContextHandle(Uuid);

impl ExecutionContextHandle {
    /// Allocate before capture and retain across retries of the same admission.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Default for ExecutionContextHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Expected ownership and semantic input, supplied by the trusted work record.
#[derive(Debug, Clone)]
pub struct ContextBinding {
    pub execution_id: Uuid,
    pub principal: PrincipalIdentity,
    pub request_digest: String,
}

/// Admission facts supplied by a trusted adapter after authentication and
/// evaluation. Public request metadata must never be mapped directly to this.
#[derive(Debug, Clone)]
pub struct RootContextAdmission {
    pub handle: ExecutionContextHandle,
    pub binding: ContextBinding,
    pub credential_id: String,
    pub auth_method: String,
    pub accepted_ceiling_revision: String,
    pub accepted_effects: Vec<AcceptedEffect>,
    pub deadline_ms: i64,
    pub evaluated_authority: AuthorityStamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextRecord {
    schema_version: u32,
    domain: String,
    namespace: String,
    tenant: String,
    handle: ExecutionContextHandle,
    execution_id: Uuid,
    principal: PrincipalIdentity,
    credential_id: String,
    auth_method: String,
    request_digest: String,
    accepted_ceiling_revision: String,
    accepted_effects: Vec<AcceptedEffect>,
    deadline_ms: i64,
    admitted_at_ms: i64,
    authority: AuthorityStamp,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SealedRecord {
    schema_version: u32,
    key_id: String,
    // Preserve exact signed bytes rather than reserializing a parsed record.
    payload: String,
    tag: Vec<u8>,
}

/// Private construction and no Deserialize implementation. This proves the
/// provenance record was verified, not that its actor currently may execute.
#[derive(Debug, Clone)]
pub struct VerifiedExecutionContext(ContextRecord);

impl VerifiedExecutionContext {
    #[must_use]
    pub fn handle(&self) -> &ExecutionContextHandle {
        &self.0.handle
    }
    #[must_use]
    pub fn principal(&self) -> &PrincipalIdentity {
        &self.0.principal
    }
    #[must_use]
    pub const fn execution_id(&self) -> Uuid {
        self.0.execution_id
    }
    #[must_use]
    pub fn credential_id(&self) -> &str {
        &self.0.credential_id
    }
    #[must_use]
    pub fn accepted_ceiling_revision(&self) -> &str {
        &self.0.accepted_ceiling_revision
    }
    #[must_use]
    pub const fn deadline_ms(&self) -> i64 {
        self.0.deadline_ms
    }
    /// Exact tuple comparison; no wildcard, prefix or grant-union expansion.
    #[must_use]
    pub fn within_accepted_ceiling(&self, effect: &AcceptedEffect) -> bool {
        self.0.accepted_effects.iter().any(|accepted| {
            accepted.operation == effect.operation
                && accepted.resources.len() == effect.resources.len()
                && effect
                    .resources
                    .iter()
                    .all(|r| accepted.resources.contains(r))
                && effect
                    .resources
                    .iter()
                    .enumerate()
                    .all(|(i, r)| !effect.resources[..i].contains(r))
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error(transparent)]
    State(#[from] acteon_state::StateError),
    #[error(transparent)]
    Coordination(#[from] CoordinationError),
    #[error("invalid execution context configuration or admission")]
    Invalid,
    #[error("execution context missing; recovery cannot synthesize authority")]
    Missing,
    #[error("execution context integrity, ownership or format verification failed")]
    Verification,
    #[error("execution context deadline expired")]
    Expired,
    #[error("execution context belongs to a different authority incarnation")]
    Incarnation,
    #[error("execution context handle conflicts with its original admission")]
    Conflict,
}

/// Trusted deployment key material. Never serialize or include secrets in Debug.
pub struct ContextSigningKey {
    id: String,
    secret: Vec<u8>,
}
impl ContextSigningKey {
    pub fn new(id: String, secret: Vec<u8>) -> Result<Self, ContextError> {
        if !valid_text(&id) || secret.len() < 32 || secret.len() > 1024 {
            return Err(ContextError::Invalid);
        }
        Ok(Self { id, secret })
    }
}

/// Privileged infrastructure capability. Do not expose this object, its signing
/// keys or root-admission method to model tools or unauthenticated adapters.
pub struct TrustedContextStore {
    store: Arc<dyn StateStore>,
    coordinator: AuthorityCoordinator,
    domain: String,
    active_key: String,
    keys: BTreeMap<String, Vec<u8>>,
}

fn valid_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "*"
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

impl TrustedContextStore {
    /// Explicit trusted deployment configuration. Retained verification keys
    /// allow rotation without reassigning admitted work to another actor.
    pub fn new(
        store: Arc<dyn StateStore>,
        coordinator: AuthorityCoordinator,
        domain: String,
        active_key: String,
        keys: Vec<ContextSigningKey>,
    ) -> Result<Self, ContextError> {
        if !valid_text(&domain) || keys.is_empty() || keys.len() > MAX_KEYS {
            return Err(ContextError::Invalid);
        }
        let mut key_map = BTreeMap::new();
        for key in keys {
            if key_map.insert(key.id, key.secret).is_some() {
                return Err(ContextError::Invalid);
            }
        }
        if !key_map.contains_key(&active_key) {
            return Err(ContextError::Invalid);
        }
        Ok(Self {
            store,
            coordinator,
            domain,
            active_key,
            keys: key_map,
        })
    }

    fn key(&self, handle: &ExecutionContextHandle) -> StateKey {
        StateKey::new(
            self.coordinator.key.namespace.clone(),
            self.coordinator.key.tenant.clone(),
            KeyKind::Custom(CONTEXT_KIND.into()),
            handle.0.to_string(),
        )
    }

    fn validate(&self, record: &ContextRecord) -> Result<(), ContextError> {
        if record.schema_version != FORMAT
            || record.domain != self.domain
            || record.namespace != self.coordinator.key.namespace.as_str()
            || record.tenant != self.coordinator.key.tenant.as_str()
            || record.execution_id.is_nil()
            || record.handle.0.is_nil()
            || !valid_text(&record.credential_id)
            || !valid_text(&record.auth_method)
            || !valid_text(&record.accepted_ceiling_revision)
            || !valid_digest(&record.request_digest)
            || record.admitted_at_ms < 0
            || record.deadline_ms <= record.admitted_at_ms
            || record.authority.generation == 0
            || Uuid::parse_str(&record.authority.incarnation).is_err()
            || record.accepted_effects.is_empty()
            || record.accepted_effects.len() > MAX_EFFECTS
        {
            return Err(ContextError::Verification);
        }
        for effect in &record.accepted_effects {
            if !valid_text(&effect.operation)
                || effect.resources.is_empty()
                || effect.resources.len() > MAX_RESOURCES
                || effect.resources.iter().enumerate().any(|(i, r)| {
                    r.namespace() != record.namespace
                        || r.tenant() != record.tenant
                        || effect.resources[..i].contains(r)
                })
            {
                return Err(ContextError::Verification);
            }
        }
        Ok(())
    }

    /// Capture an already authenticated/evaluated root. This does not reserve
    /// attempts, spend budget or grant permission to execute. The returned handle
    /// is stored in the authoritative durable work record before its handoff.
    pub async fn capture_root(
        &self,
        admission: RootContextAdmission,
        now_ms: i64,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        let record = ContextRecord {
            schema_version: FORMAT,
            domain: self.domain.clone(),
            namespace: self.coordinator.key.namespace.to_string(),
            tenant: self.coordinator.key.tenant.to_string(),
            handle: admission.handle,
            execution_id: admission.binding.execution_id,
            principal: admission.binding.principal,
            request_digest: admission.binding.request_digest,
            credential_id: admission.credential_id,
            auth_method: admission.auth_method,
            accepted_ceiling_revision: admission.accepted_ceiling_revision,
            accepted_effects: admission.accepted_effects,
            deadline_ms: admission.deadline_ms,
            admitted_at_ms: now_ms,
            authority: admission.evaluated_authority,
        };
        self.validate(&record).map_err(|_| ContextError::Invalid)?;
        let binding = ContextBinding {
            execution_id: record.execution_id,
            principal: record.principal.clone(),
            request_digest: record.request_digest.clone(),
        };
        match self.recover(&record.handle, &binding, now_ms).await {
            Ok(existing) => return Self::check_replay(existing, &record),
            Err(ContextError::Missing) => {}
            Err(error) => return Err(error),
        }
        if self.coordinator.snapshot().await?.stamp() != record.authority {
            return Err(ContextError::Coordination(
                CoordinationError::StaleAuthority,
            ));
        }
        let payload = serde_json::to_string(&record).map_err(|_| ContextError::Invalid)?;
        let secret = self
            .keys
            .get(&self.active_key)
            .ok_or(ContextError::Invalid)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|_| ContextError::Invalid)?;
        mac.update(payload.as_bytes());
        let sealed = SealedRecord {
            schema_version: FORMAT,
            key_id: self.active_key.clone(),
            payload,
            tag: mac.finalize().into_bytes().to_vec(),
        };
        let encoded = serde_json::to_string(&sealed).map_err(|_| ContextError::Invalid)?;
        if encoded.len() > MAX_BYTES {
            return Err(ContextError::Invalid);
        }
        if !self
            .store
            .check_and_set(&self.key(&record.handle), &encoded, None)
            .await?
        {
            return Self::check_replay(
                self.recover(&record.handle, &binding, now_ms).await?,
                &record,
            );
        }
        Ok(VerifiedExecutionContext(record))
    }

    fn check_replay(
        existing: VerifiedExecutionContext,
        proposed: &ContextRecord,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        let mut original = existing.0.clone();
        original.admitted_at_ms = proposed.admitted_at_ms;
        if original != *proposed {
            return Err(ContextError::Conflict);
        }
        Ok(existing)
    }

    /// Recover only through trusted storage, bound to independently trusted work
    /// ownership/input. Generation changes are allowed: current evaluation will
    /// decide whether the next effect may run. Incarnation changes are refused.
    pub async fn recover(
        &self,
        handle: &ExecutionContextHandle,
        binding: &ContextBinding,
        now_ms: i64,
    ) -> Result<VerifiedExecutionContext, ContextError> {
        let encoded = self
            .store
            .get(&self.key(handle))
            .await?
            .ok_or(ContextError::Missing)?;
        if encoded.len() > MAX_BYTES || now_ms < 0 {
            return Err(ContextError::Verification);
        }
        let sealed: SealedRecord =
            serde_json::from_str(&encoded).map_err(|_| ContextError::Verification)?;
        if sealed.schema_version != FORMAT || sealed.tag.len() != 32 {
            return Err(ContextError::Verification);
        }
        let secret = self
            .keys
            .get(&sealed.key_id)
            .ok_or(ContextError::Verification)?;
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret).map_err(|_| ContextError::Verification)?;
        mac.update(sealed.payload.as_bytes());
        mac.verify_slice(&sealed.tag)
            .map_err(|_| ContextError::Verification)?;
        let record: ContextRecord =
            serde_json::from_str(&sealed.payload).map_err(|_| ContextError::Verification)?;
        self.validate(&record)?;
        if record.handle != *handle
            || record.execution_id != binding.execution_id
            || record.principal != binding.principal
            || record.request_digest != binding.request_digest
            || now_ms < record.admitted_at_ms
        {
            return Err(ContextError::Verification);
        }
        if now_ms >= record.deadline_ms {
            return Err(ContextError::Expired);
        }
        let current = self.coordinator.snapshot().await?.stamp();
        if current.incarnation != record.authority.incarnation {
            return Err(ContextError::Incarnation);
        }
        Ok(VerifiedExecutionContext(record))
    }
}
