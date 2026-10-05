//! Durable admission around the complete dispatch pipeline.
//!
//! A receipt is an acceptance record, not an exactly-once provider guarantee.
//! Interrupted provider calls require reconciliation; prepared chain starts can
//! recover their stable execution ID without reevaluating policy.
use acteon_core::{Action, ActionOutcome, Caller, ChainConfig};
use acteon_state::{CasResult, KeyKind, StateKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::gateway::DispatchOrigin;
use crate::{Gateway, GatewayError};

const KIND: &str = "dispatch_receipt";
const MAX_CAS_ATTEMPTS: usize = 8;

/// Bounds for a durable dispatch. The first admission pins these settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchAdmissionConfig {
    /// Dispatch ownership duration. Expiration fences receipt writes, not effects.
    pub lease_seconds: u32,
    /// Maximum serialized plaintext receipt size, including action and outcome.
    pub max_receipt_bytes: usize,
}
impl Default for DispatchAdmissionConfig {
    fn default() -> Self {
        Self {
            lease_seconds: 120,
            max_receipt_bytes: 2 * 1024 * 1024,
        }
    }
}

/// The durable state of an admitted action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DispatchReceiptStatus {
    /// No dispatch attempt has started; another worker can claim it safely.
    Accepted,
    /// One attempt owns dispatch. An expired non-chain attempt is ambiguous.
    Running {
        /// Unique fencing token for this attempt.
        token: String,
        /// Exclusive UTC expiry of the attempt.
        lease_until: DateTime<Utc>,
    },
    /// The dispatch outcome is durable. `ChainStarted` means durable handoff.
    Completed {
        /// Original result, including failures; retries return this same result.
        outcome: ActionOutcome,
    },
    /// Execution may have had effects; an operator must resolve it explicitly.
    ReconciliationRequired,
}

/// Versioned, tenant-scoped durable acceptance and result record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchReceipt {
    /// Wire format version.
    pub schema_version: u32,
    /// Current `StateStore` CAS version, populated when loading.
    #[serde(default)]
    pub version: u64,
    /// Stable key supplied in the original Action's `dedup_key`.
    pub idempotency_key: String,
    /// SHA-256 of semantic action fields and caller identity.
    pub request_digest: String,
    /// Original action; a duplicate cannot replace its ID, payload, or provenance.
    pub action: Action,
    /// Identity from initial admission. Transport authorization remains required.
    pub caller: Option<Caller>,
    /// First-admission timestamp.
    pub accepted_at: DateTime<Utc>,
    /// Last durable transition.
    pub updated_at: DateTime<Utc>,
    /// Pinned settings.
    pub config: DispatchAdmissionConfig,
    /// Acceptance/execution/result state.
    pub status: DispatchReceiptStatus,
    /// Bounded operator decision history, including the prior failure state.
    #[serde(default)]
    pub resolutions: Vec<DispatchResolutionRecord>,
    /// Prepared chain start, retained even after completion for provenance.
    pub(crate) chain: Option<AdmittedChain>,
}
impl DispatchReceipt {
    /// Recorded dispatch outcome, when completed.
    #[must_use]
    pub fn outcome(&self) -> Option<&ActionOutcome> {
        match &self.status {
            DispatchReceiptStatus::Completed { outcome } => Some(outcome),
            _ => None,
        }
    }
    /// Stable chain execution ID, if the routing decision prepared a chain.
    #[must_use]
    pub fn chain_id(&self) -> Option<&str> {
        self.chain.as_ref().map(|chain| chain.chain_id.as_str())
    }
}

/// Result of initial dispatch or replay of an admitted request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableDispatchResult {
    /// Latest durable receipt. Running/reconciliation are not successful effects.
    pub receipt: DispatchReceipt,
    /// True when an existing acceptance was reused.
    pub replayed: bool,
}

/// Explicit operator decision for an ambiguous dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", content = "outcome", rename_all = "snake_case")]
pub enum DispatchResolution {
    /// Provider reconciliation established the outcome. Record it without execution.
    Complete(ActionOutcome),
    /// Operator confirmed no effects occurred and stopped the previous worker.
    /// Return the original action to Accepted for a subsequent dispatch attempt.
    RetryNotApplied,
}

/// Version-fenced reconciliation request from a trusted operator transport.
#[derive(Debug, Clone)]
pub struct DispatchResolutionRequest {
    /// CAS version returned by receipt inspection.
    pub expected_version: u64,
    /// Independently established disposition.
    pub resolution: DispatchResolution,
    /// Operator identity established by the transport, never untrusted input.
    pub resolved_by: Caller,
    /// Nonempty explanation or evidence reference, limited to 1,024 bytes.
    pub reason: String,
}

/// Durable provenance for an explicit operator decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchResolutionRecord {
    /// Operator identity supplied by the trusted transport.
    pub resolved_by: Caller,
    /// Explanation or evidence reference.
    pub reason: String,
    /// Time the decision was recorded.
    pub resolved_at: DateTime<Utc>,
    /// Previous ambiguous or failed state, preserved before resetting it.
    pub previous_status: DispatchReceiptStatus,
    /// Applied decision.
    pub resolution: DispatchResolution,
}

/// Admission errors are separate from normal provider failure outcomes.
#[derive(Debug, thiserror::Error)]
pub enum DispatchAdmissionError {
    /// Storage failure, possibly with an ambiguous committed write.
    #[error(transparent)]
    State(#[from] acteon_state::StateError),
    /// Dispatch or payload-encryption failure.
    #[error(transparent)]
    Gateway(#[from] GatewayError),
    /// Invalid request, settings, or stored receipt.
    #[error("invalid dispatch admission: {0}")]
    Invalid(String),
    /// The key was reused with different semantic fields or caller identity.
    #[error("dispatch idempotency key conflicts with the original request")]
    RequestConflict,
    /// Receipt storage bound exceeded. Effects may already have occurred;
    /// inspect the existing receipt with the same key before any retry.
    #[error("dispatch receipt exceeds its {limit}-byte limit")]
    ReceiptCapacity {
        /// Maximum plaintext size.
        limit: usize,
    },
    /// Another writer changed the receipt during a bounded CAS attempt.
    #[error("dispatch receipt changed; reload before retrying")]
    Conflict,
    /// The current attempt expired or was superseded.
    #[error("dispatch admission lease lost")]
    LeaseLost,
    /// Operator resolution is not permitted for this receipt state.
    #[error("dispatch receipt is not eligible for this resolution")]
    InvalidResolution,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AdmittedChain {
    pub chain_id: String,
    pub receipt_id: String,
    pub config: ChainConfig,
    pub action: Action,
}

pub(crate) struct DispatchAttempt {
    pub key: StateKey,
    pub token: String,
}

fn receipt_key(
    namespace: &str,
    tenant: &str,
    key: &str,
) -> Result<StateKey, DispatchAdmissionError> {
    if key.trim().is_empty() || key.len() > 512 {
        return Err(DispatchAdmissionError::Invalid(
            "idempotency key must contain 1..=512 bytes".into(),
        ));
    }
    // Hash the user key so StateKey separators cannot change its storage scope.
    Ok(StateKey::new(
        namespace,
        tenant,
        KeyKind::Custom(KIND.into()),
        hex::encode(Sha256::digest(key.as_bytes())),
    ))
}

fn validate_config(config: &DispatchAdmissionConfig) -> Result<(), DispatchAdmissionError> {
    if config.lease_seconds == 0
        || config.max_receipt_bytes == 0
        || config.max_receipt_bytes > 10 * 1024 * 1024
    {
        return Err(DispatchAdmissionError::Invalid(
            "positive lease and receipt limit <= 10 MiB required".into(),
        ));
    }
    Ok(())
}

fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> =
                map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical).collect())
        }
        other => other,
    }
}
fn request_digest(
    action: &Action,
    caller: Option<&Caller>,
) -> Result<String, DispatchAdmissionError> {
    let mut value =
        serde_json::to_value(action).map_err(|e| DispatchAdmissionError::Invalid(e.to_string()))?;
    // Delivery attempts may have fresh transport IDs, timestamps, tracing, and signatures.
    for field in [
        "id",
        "created_at",
        "trace_context",
        "signature",
        "kid",
        "signer_id",
    ] {
        value
            .as_object_mut()
            .expect("Action serializes as object")
            .remove(field);
    }
    let semantic_caller = match caller.and_then(|c| c.principal.as_ref()) {
        Some(principal) => serde_json::json!({"principal":principal}),
        None => serde_json::to_value(caller)
            .map_err(|e| DispatchAdmissionError::Invalid(e.to_string()))?,
    };
    let bytes = serde_json::to_vec(&canonical(
        serde_json::json!({"action":value,"caller":semantic_caller}),
    ))
    .map_err(|e| DispatchAdmissionError::Invalid(e.to_string()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

impl Gateway {
    /// Persist acceptance then dispatch through normal rules, quotas, audit, and chains.
    ///
    /// Requires an explicit `Action.dedup_key`. Duplicates return the original
    /// receipt and never overwrite its request. No automatic provider retry is
    /// performed by admission; executor retries still follow executor policy.
    /// Receipts have no automatic TTL, including completed receipts.
    pub async fn dispatch_durable(
        &self,
        action: Action,
        caller: Option<&Caller>,
        config: DispatchAdmissionConfig,
    ) -> Result<DurableDispatchResult, DispatchAdmissionError> {
        let admitted = self.admit_dispatch(action, caller, config).await?;
        let initial = admitted.receipt;
        let created = !admitted.replayed;
        let digest = initial.request_digest.clone();
        let key = receipt_key(
            initial.action.namespace.as_str(),
            initial.action.tenant.as_str(),
            &initial.idempotency_key,
        )?;
        for _ in 0..MAX_CAS_ATTEMPTS {
            let mut receipt = self
                .load_receipt(&key)
                .await?
                .ok_or(DispatchAdmissionError::Conflict)?;
            if receipt.request_digest != digest || receipt.config != initial.config {
                return Err(DispatchAdmissionError::RequestConflict);
            }
            let now = self.clock.now();
            match &receipt.status {
                DispatchReceiptStatus::Completed { .. }
                | DispatchReceiptStatus::ReconciliationRequired => {
                    return Ok(DurableDispatchResult {
                        receipt,
                        replayed: !created,
                    });
                }
                DispatchReceiptStatus::Running { lease_until, .. } if *lease_until > now => {
                    return Ok(DurableDispatchResult {
                        receipt,
                        replayed: true,
                    });
                }
                DispatchReceiptStatus::Running { .. } if receipt.chain.is_none() => {
                    receipt.status = DispatchReceiptStatus::ReconciliationRequired;
                    if self.swap_receipt(&key, &mut receipt).await? {
                        return Ok(DurableDispatchResult {
                            receipt,
                            replayed: true,
                        });
                    }
                    continue;
                }
                _ => {}
            }
            let token = uuid::Uuid::new_v4().to_string();
            let lease_until = now
                .checked_add_signed(chrono::Duration::seconds(i64::from(
                    receipt.config.lease_seconds,
                )))
                .ok_or_else(|| {
                    DispatchAdmissionError::Invalid("lease timestamp overflow".into())
                })?;
            receipt.status = DispatchReceiptStatus::Running {
                token: token.clone(),
                lease_until,
            };
            if !self.swap_receipt(&key, &mut receipt).await? {
                continue;
            }
            let attempt = DispatchAttempt {
                key: key.clone(),
                token,
            };
            let result = if let Some(plan) = &receipt.chain {
                self.resume_admitted_chain(plan, receipt.caller.as_ref())
                    .await
            } else {
                self.dispatch_pipeline(
                    receipt.action.clone(),
                    receipt.caller.as_ref(),
                    false,
                    DispatchOrigin::External,
                    Some(&attempt),
                    None,
                )
                .await
            };
            // An error can happen after effects. Keep Running and its prepared
            // chain so expiration/retry can reconcile instead of blindly execute.
            let outcome = result?;
            let receipt = self.complete_attempt(&attempt, outcome).await?;
            return Ok(DurableDispatchResult {
                receipt,
                replayed: !created,
            });
        }
        Err(DispatchAdmissionError::Conflict)
    }

    /// Accept an action durably without starting execution. A caller/worker
    /// must later call `dispatch_durable` with the same request to process it.
    pub async fn admit_dispatch(
        &self,
        action: Action,
        caller: Option<&Caller>,
        config: DispatchAdmissionConfig,
    ) -> Result<DurableDispatchResult, DispatchAdmissionError> {
        validate_config(&config)?;
        let id = action
            .dedup_key
            .as_deref()
            .ok_or_else(|| DispatchAdmissionError::Invalid("explicit dedup_key required".into()))?;
        let key = receipt_key(action.namespace.as_str(), action.tenant.as_str(), id)?;
        let digest = request_digest(&action, caller)?;
        let now = self.clock.now();
        let initial = DispatchReceipt {
            schema_version: 1,
            version: 0,
            idempotency_key: id.into(),
            request_digest: digest.clone(),
            action,
            caller: caller.cloned(),
            accepted_at: now,
            updated_at: now,
            config,
            status: DispatchReceiptStatus::Accepted,
            resolutions: Vec::new(),
            chain: None,
        };
        let encoded = self.encode_receipt(&initial)?;
        let created = self.state.check_and_set(&key, &encoded, None).await?;
        let receipt = self
            .load_receipt(&key)
            .await?
            .ok_or(DispatchAdmissionError::Conflict)?;
        if receipt.request_digest != digest || receipt.config != initial.config {
            return Err(DispatchAdmissionError::RequestConflict);
        }
        Ok(DurableDispatchResult {
            receipt,
            replayed: !created,
        })
    }

    /// Load a receipt within its namespace and tenant. Does not claim or execute it.
    pub async fn get_dispatch_receipt(
        &self,
        namespace: &str,
        tenant: &str,
        key: &str,
    ) -> Result<Option<DispatchReceipt>, DispatchAdmissionError> {
        self.load_receipt(&receipt_key(namespace, tenant, key)?)
            .await
    }

    /// Resolve an ambiguous receipt using the observed CAS version.
    ///
    /// `RetryNotApplied` requires independently verifying no effects and stopping
    /// the old worker: receipt fencing cannot cancel an external provider call.
    /// Prepared chains recover automatically and cannot be reset by this API.
    pub async fn resolve_dispatch_receipt(
        &self,
        namespace: &str,
        tenant: &str,
        key: &str,
        request: DispatchResolutionRequest,
    ) -> Result<DispatchReceipt, DispatchAdmissionError> {
        if request.resolved_by.id.trim().is_empty()
            || request.reason.trim().is_empty()
            || request.reason.len() > 1024
        {
            return Err(DispatchAdmissionError::Invalid(
                "operator identity and nonempty reason <= 1024 bytes required".into(),
            ));
        }
        let key = receipt_key(namespace, tenant, key)?;
        let mut receipt = self
            .load_receipt(&key)
            .await?
            .ok_or(DispatchAdmissionError::Conflict)?;
        if receipt.version != request.expected_version {
            return Err(DispatchAdmissionError::Conflict);
        }
        if receipt.chain.is_some()
            || !matches!(
                receipt.status,
                DispatchReceiptStatus::ReconciliationRequired
                    | DispatchReceiptStatus::Completed {
                        outcome: ActionOutcome::Failed(_)
                    }
            )
        {
            return Err(DispatchAdmissionError::InvalidResolution);
        }
        if receipt.resolutions.len() >= 32 {
            return Err(DispatchAdmissionError::Invalid(
                "operator decision history capacity reached".into(),
            ));
        }
        receipt.resolutions.push(DispatchResolutionRecord {
            resolved_by: request.resolved_by,
            reason: request.reason,
            resolved_at: self.clock.now(),
            previous_status: receipt.status.clone(),
            resolution: request.resolution.clone(),
        });
        receipt.status = match request.resolution {
            DispatchResolution::Complete(outcome) => DispatchReceiptStatus::Completed { outcome },
            DispatchResolution::RetryNotApplied => DispatchReceiptStatus::Accepted,
        };
        if !self.swap_receipt(&key, &mut receipt).await? {
            return Err(DispatchAdmissionError::Conflict);
        }
        Ok(receipt)
    }

    fn encode_receipt(&self, receipt: &DispatchReceipt) -> Result<String, DispatchAdmissionError> {
        let json = serde_json::to_string(receipt)
            .map_err(|e| DispatchAdmissionError::Invalid(e.to_string()))?;
        if json.len() > receipt.config.max_receipt_bytes {
            return Err(DispatchAdmissionError::ReceiptCapacity {
                limit: receipt.config.max_receipt_bytes,
            });
        }
        Ok(self.encrypt_state_value(&json)?)
    }

    async fn load_receipt(
        &self,
        key: &StateKey,
    ) -> Result<Option<DispatchReceipt>, DispatchAdmissionError> {
        let Some((encoded, version)) = self.state.get_versioned(key).await? else {
            return Ok(None);
        };
        let json = self.decrypt_state_value(&encoded)?;
        let mut receipt: DispatchReceipt = serde_json::from_str(&json)
            .map_err(|e| DispatchAdmissionError::Invalid(e.to_string()))?;
        validate_config(&receipt.config)?;
        if receipt.schema_version != 1
            || receipt.resolutions.len() > 32
            || receipt.resolutions.iter().any(|record| {
                record.reason.trim().is_empty()
                    || record.reason.len() > 1024
                    || record.resolved_by.id.trim().is_empty()
            })
            || json.len() > receipt.config.max_receipt_bytes
            || receipt_key(
                receipt.action.namespace.as_str(),
                receipt.action.tenant.as_str(),
                &receipt.idempotency_key,
            )? != *key
            || receipt.action.dedup_key.as_deref() != Some(receipt.idempotency_key.as_str())
            || request_digest(&receipt.action, receipt.caller.as_ref())? != receipt.request_digest
        {
            return Err(DispatchAdmissionError::Invalid(
                "receipt identity or format mismatch".into(),
            ));
        }
        if let Some(plan) = &receipt.chain
            && (plan.config.steps.is_empty()
                || !plan.config.validate().is_empty()
                || plan.receipt_id != key.id.as_str()
                || plan.action.namespace != receipt.action.namespace
                || plan.action.tenant != receipt.action.tenant
                || plan.action.id != receipt.action.id
                || plan.action.dedup_key != receipt.action.dedup_key
                || plan.chain_id.is_empty())
        {
            return Err(DispatchAdmissionError::Invalid(
                "prepared chain identity or definition mismatch".into(),
            ));
        }

        if receipt.updated_at < receipt.accepted_at
            || matches!(&receipt.status, DispatchReceiptStatus::Running { token, lease_until } if token.is_empty() || *lease_until <= receipt.accepted_at)
            || (receipt.chain.is_some()
                && matches!(
                    receipt.status,
                    DispatchReceiptStatus::Accepted | DispatchReceiptStatus::ReconciliationRequired
                ))
        {
            return Err(DispatchAdmissionError::Invalid(
                "receipt state or timestamp mismatch".into(),
            ));
        }
        receipt.version = version;
        Ok(Some(receipt))
    }

    async fn swap_receipt(
        &self,
        key: &StateKey,
        receipt: &mut DispatchReceipt,
    ) -> Result<bool, DispatchAdmissionError> {
        receipt.updated_at = self.clock.now();
        let encoded = self.encode_receipt(receipt)?;
        if self
            .state
            .compare_and_swap(key, receipt.version, &encoded, None)
            .await?
            == CasResult::Ok
        {
            receipt.version = receipt
                .version
                .checked_add(1)
                .ok_or_else(|| DispatchAdmissionError::Invalid("version overflow".into()))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn owned_receipt(
        &self,
        attempt: &DispatchAttempt,
    ) -> Result<DispatchReceipt, DispatchAdmissionError> {
        let receipt = self
            .load_receipt(&attempt.key)
            .await?
            .ok_or(DispatchAdmissionError::LeaseLost)?;
        if !matches!(&receipt.status, DispatchReceiptStatus::Running { token, lease_until } if token == &attempt.token && *lease_until > self.clock.now())
        {
            return Err(DispatchAdmissionError::LeaseLost);
        }
        Ok(receipt)
    }

    pub(crate) async fn verify_dispatch_attempt(
        &self,
        attempt: &DispatchAttempt,
    ) -> Result<(), GatewayError> {
        self.owned_receipt(attempt)
            .await
            .map(|_| ())
            .map_err(admission_gateway_error)
    }

    pub(crate) async fn prepare_admitted_chain(
        &self,
        attempt: &DispatchAttempt,
        action: &Action,
        name: &str,
    ) -> Result<AdmittedChain, GatewayError> {
        let mut receipt = self
            .owned_receipt(attempt)
            .await
            .map_err(admission_gateway_error)?;
        let config = self.chains.read().get(name).cloned().ok_or_else(|| {
            GatewayError::ChainError(format!("chain configuration not found: {name}"))
        })?;
        if config.steps.is_empty() || !config.validate().is_empty() {
            return Err(GatewayError::ChainError(
                "invalid admitted chain configuration".into(),
            ));
        }
        self.pin_admitted_chain_definition(action, &config).await?;
        let plan = AdmittedChain {
            chain_id: uuid::Uuid::new_v4().to_string(),
            receipt_id: attempt.key.id.clone(),
            config,
            action: action.clone(),
        };
        receipt.chain = Some(plan.clone());
        if !self
            .swap_receipt(&attempt.key, &mut receipt)
            .await
            .map_err(admission_gateway_error)?
        {
            return Err(admission_gateway_error(DispatchAdmissionError::LeaseLost));
        }
        Ok(plan)
    }

    pub(crate) async fn pin_admitted_chain_definition(
        &self,
        action: &Action,
        config: &ChainConfig,
    ) -> Result<(), GatewayError> {
        self.pin_chain_definition(action.namespace.as_str(), action.tenant.as_str(), config)
            .await?;
        let key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::Custom(crate::execution::PINNED_CHAIN_DEF_KIND.into()),
            format!("{}@{}", config.name, config.version),
        );
        let raw = self.state.get(&key).await?.ok_or_else(|| {
            GatewayError::ChainError("admitted pinned definition disappeared".into())
        })?;
        let stored: serde_json::Value = serde_json::from_str(&self.decrypt_state_value(&raw)?)
            .map_err(|error| GatewayError::ChainError(error.to_string()))?;
        let expected = serde_json::to_value(config)
            .map_err(|error| GatewayError::ChainError(error.to_string()))?;
        if stored != expected {
            return Err(GatewayError::ChainError(
                "admitted chain version has a conflicting pinned definition".into(),
            ));
        }
        Ok(())
    }

    async fn complete_attempt(
        &self,
        attempt: &DispatchAttempt,
        outcome: ActionOutcome,
    ) -> Result<DispatchReceipt, DispatchAdmissionError> {
        let mut receipt = self.owned_receipt(attempt).await?;
        receipt.status = DispatchReceiptStatus::Completed { outcome };
        if !self.swap_receipt(&attempt.key, &mut receipt).await? {
            return Err(DispatchAdmissionError::LeaseLost);
        }
        Ok(receipt)
    }

    async fn resume_admitted_chain(
        &self,
        plan: &AdmittedChain,
        caller: Option<&Caller>,
    ) -> Result<ActionOutcome, GatewayError> {
        if let Some(chain) = self
            .get_chain_status(
                plan.action.namespace.as_str(),
                plan.action.tenant.as_str(),
                &plan.chain_id,
            )
            .await?
        {
            if chain.chain_name != plan.config.name
                || chain.origin_action.id != plan.action.id
                || chain.chain_version != plan.config.version
                || chain.dispatch_receipt_id.as_deref() != Some(plan.receipt_id.as_str())
            {
                return Err(GatewayError::ChainError(
                    "admitted chain identity mismatch".into(),
                ));
            }
            self.append_admitted_chain_start_history(&chain).await?;
            if chain.status.is_active() {
                self.repair_chain_discovery(&chain).await?;
            }
            return Ok(admitted_chain_outcome(plan));
        }
        self.handle_chain(&plan.action, &plan.config.name, caller, Some(plan))
            .await
    }
}

fn admitted_chain_outcome(plan: &AdmittedChain) -> ActionOutcome {
    ActionOutcome::ChainStarted {
        chain_id: plan.chain_id.clone(),
        chain_name: plan.config.name.clone(),
        total_steps: plan.config.steps.len(),
        first_step: plan.config.steps[0].name.clone(),
    }
}
fn admission_gateway_error(error: DispatchAdmissionError) -> GatewayError {
    match error {
        DispatchAdmissionError::State(error) => GatewayError::State(error),
        DispatchAdmissionError::Gateway(error) => error,
        other => GatewayError::Configuration(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GatewayBuilder;
    use acteon_core::{ChainStatus, ChainStepConfig, ProviderResponse};
    use acteon_executor::ExecutorConfig;
    use acteon_provider::{DynProvider, ProviderError};
    use acteon_state::StateStore;
    use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
    use acteon_time::ManualClock;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use tokio::sync::Notify;

    struct Sink {
        calls: AtomicUsize,
        block: bool,
        fail_first: bool,
        response_bytes: usize,
        entered: Notify,
        release: Notify,
    }
    impl Sink {
        fn new(block: bool) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                block,
                fail_first: false,
                response_bytes: 0,
                entered: Notify::new(),
                release: Notify::new(),
            })
        }
    }
    #[async_trait]
    impl DynProvider for Sink {
        fn name(&self) -> &'static str {
            "sink"
        }
        async fn execute(&self, _: &Action) -> Result<ProviderResponse, ProviderError> {
            let previous = self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            if self.fail_first && previous == 0 {
                return Err(ProviderError::ExecutionFailed(
                    "injected provider failure".into(),
                ));
            }
            if self.block {
                self.release.notified().await;
            }
            let body = if self.response_bytes == 0 {
                json!({"accepted": true})
            } else {
                json!({"data": "x".repeat(self.response_bytes)})
            };
            Ok(ProviderResponse::success(body))
        }
        async fn health_check(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }
    fn clock() -> Arc<ManualClock> {
        Arc::new(ManualClock::new("2026-10-02T00:00:00Z".parse().unwrap()))
    }
    fn config() -> DispatchAdmissionConfig {
        DispatchAdmissionConfig {
            lease_seconds: 2,
            ..Default::default()
        }
    }
    fn action() -> Action {
        Action::new(
            "observability",
            "acme",
            "sink",
            "detect",
            json!({"score": 4_037.054_624_999_999_8}),
        )
        .with_dedup_key("incident:1")
    }
    fn gateway(
        store: Arc<MemoryStateStore>,
        clock: Arc<ManualClock>,
        sink: Arc<Sink>,
        chain: bool,
    ) -> Arc<Gateway> {
        let mut builder = GatewayBuilder::new()
            .clock(clock.clone())
            .state(store)
            .lock(Arc::new(MemoryDistributedLock::with_clock(clock)))
            .provider(sink)
            .completed_chain_ttl(Duration::from_secs(1))
            .executor_config(ExecutorConfig {
                max_retries: 0,
                execution_timeout: Duration::from_secs(300),
                ..Default::default()
            });
        if chain {
            use acteon_rules::RuleFrontend;
            let rules = acteon_rules_yaml::YamlFrontend
                .parse(
                    r"
rules:
  - name: incident
    priority: 0
    condition: {field: action.action_type, eq: detect}
    action: {type: chain, chain: incident}
",
                )
                .unwrap();
            builder = builder.rules(rules).chain(
                ChainConfig::new("incident").with_step(ChainStepConfig::new(
                    "notify",
                    "sink",
                    "notify",
                    json!({}),
                )),
            );
        }
        Arc::new(builder.build().unwrap())
    }
    fn resolution_request(
        expected_version: u64,
        resolution: DispatchResolution,
    ) -> DispatchResolutionRequest {
        DispatchResolutionRequest {
            expected_version,
            resolution,
            resolved_by: Caller {
                id: "operator".into(),
                principal: None,
                auth_method: "api_key".into(),
            },
            reason: "Verified receiver state and stopped the previous worker".into(),
        }
    }

    async fn interrupt_chain_start(
        gateway: &Gateway,
        action: Action,
    ) -> (DispatchAttempt, AdmittedChain) {
        let mut receipt = gateway
            .admit_dispatch(action.clone(), None, config())
            .await
            .unwrap()
            .receipt;
        let key = receipt_key("observability", "acme", "incident:1").unwrap();
        let attempt = DispatchAttempt {
            key,
            token: "interrupted-worker".into(),
        };
        receipt.status = DispatchReceiptStatus::Running {
            token: attempt.token.clone(),
            lease_until: gateway.clock.now() + chrono::Duration::seconds(2),
        };
        assert!(
            gateway
                .swap_receipt(&attempt.key, &mut receipt)
                .await
                .unwrap()
        );
        let plan = gateway
            .prepare_admitted_chain(&attempt, &action, "incident")
            .await
            .unwrap();
        (attempt, plan)
    }

    #[tokio::test]
    async fn accepted_action_survives_worker_replacement_and_preserves_original_provenance() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(false);
        let original = action();
        let gw = gateway(store.clone(), clock.clone(), sink.clone(), false);
        let admitted = gw
            .admit_dispatch(original.clone(), None, config())
            .await
            .unwrap();
        assert!(!admitted.replayed);
        assert_eq!(sink.calls.load(Ordering::SeqCst), 0);
        drop(gw);
        let replacement = gateway(store, clock, sink.clone(), false);
        let mut retry = action();
        retry
            .trace_context
            .insert("traceparent".into(), "new-delivery-trace".into());
        let result = replacement
            .dispatch_durable(retry, None, config())
            .await
            .unwrap();
        assert!(result.replayed);
        assert_eq!(result.receipt.action.id, original.id);
        assert!(matches!(
            result.receipt.outcome(),
            Some(ActionOutcome::Executed(_))
        ));
        let again = replacement
            .dispatch_durable(action(), None, config())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(result.receipt.outcome()).unwrap(),
            serde_json::to_value(again.receipt.outcome()).unwrap()
        );
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn original_principal_survives_replacement_gateway_and_chain_handoff() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(false);
        let original = gateway(store.clone(), clock.clone(), sink.clone(), true);
        let actor =
            acteon_core::PrincipalIdentity::new("investigator", acteon_core::PrincipalKind::Agent)
                .unwrap();
        let caller = Caller {
            id: "old-key".into(),
            auth_method: "api_key".into(),
            principal: Some(actor.clone()),
        };
        original
            .admit_dispatch(action(), Some(&caller), config())
            .await
            .unwrap();
        drop(original);
        let replacement = gateway(store, clock, sink.clone(), true);
        let rotated = Caller {
            id: "new-key".into(),
            auth_method: "api_key".into(),
            principal: Some(actor.clone()),
        };
        let result = replacement
            .dispatch_durable(action(), Some(&rotated), config())
            .await
            .unwrap();
        let retained = result.receipt.caller.as_ref().unwrap();
        assert_eq!(retained.id, "old-key");
        assert_eq!(retained.principal.as_ref(), Some(&actor));
        let chain_id = result.receipt.chain_id().unwrap();
        let chain = replacement
            .get_chain_status("observability", "acme", chain_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            chain.caller.as_ref().unwrap().principal.as_ref(),
            Some(&actor)
        );
        assert_eq!(chain.caller.as_ref().unwrap().id, "old-key");
        Box::pin(replacement.advance_chain("observability", "acme", chain_id))
            .await
            .unwrap();
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn changed_payload_or_caller_conflicts_but_tenant_keys_are_independent() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(false);
        let gw = gateway(store, clock, sink.clone(), false);
        let caller = Caller {
            id: "alice".into(),
            principal: None,
            auth_method: "api_key".into(),
        };
        gw.dispatch_durable(action(), Some(&caller), config())
            .await
            .unwrap();
        let mut changed = action();
        changed.payload = json!({"score": 0.1});
        assert!(matches!(
            gw.dispatch_durable(changed, Some(&caller), config()).await,
            Err(DispatchAdmissionError::RequestConflict)
        ));
        assert!(matches!(
            gw.dispatch_durable(action(), None, config()).await,
            Err(DispatchAdmissionError::RequestConflict)
        ));
        let mut other = action();
        other.tenant = "other".into();
        gw.dispatch_durable(other, Some(&caller), config())
            .await
            .unwrap();
        assert_eq!(sink.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn concurrent_retry_returns_running_without_invoking_provider_twice() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(true);
        let gw = gateway(store, clock, sink.clone(), false);
        let first = {
            let gw = gw.clone();
            tokio::spawn(async move { gw.dispatch_durable(action(), None, config()).await })
        };
        tokio::time::timeout(Duration::from_secs(5), sink.entered.notified())
            .await
            .expect("provider was never invoked");
        for _ in 0..10 {
            let receipt = gw
                .dispatch_durable(action(), None, config())
                .await
                .unwrap()
                .receipt;
            assert!(matches!(
                receipt.status,
                DispatchReceiptStatus::Running { .. }
            ));
        }
        sink.release.notify_one();
        assert!(first.await.unwrap().unwrap().receipt.outcome().is_some());
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_provider_is_ambiguous_and_operator_resolution_is_version_fenced() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(true);
        let gw = gateway(store, clock.clone(), sink.clone(), false);
        let first = {
            let gw = gw.clone();
            tokio::spawn(async move { gw.dispatch_durable(action(), None, config()).await })
        };
        tokio::time::timeout(Duration::from_secs(5), sink.entered.notified())
            .await
            .expect("provider was never invoked");
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        clock.advance_to(Duration::from_secs(3)).unwrap();
        let receipt = gw
            .dispatch_durable(action(), None, config())
            .await
            .unwrap()
            .receipt;
        assert!(matches!(
            receipt.status,
            DispatchReceiptStatus::ReconciliationRequired
        ));
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            gw.resolve_dispatch_receipt(
                "observability",
                "acme",
                "incident:1",
                resolution_request(receipt.version - 1, DispatchResolution::RetryNotApplied)
            )
            .await,
            Err(DispatchAdmissionError::Conflict)
        ));
        // The receiver established that the interrupted call had already applied.
        gw.resolve_dispatch_receipt(
            "observability",
            "acme",
            "incident:1",
            resolution_request(
                receipt.version,
                DispatchResolution::Complete(ActionOutcome::Executed(ProviderResponse::success(
                    json!({"reconciled":true}),
                ))),
            ),
        )
        .await
        .unwrap();
        let recovered = gw.dispatch_durable(action(), None, config()).await.unwrap();
        assert_eq!(
            serde_json::to_value(recovered.receipt.outcome().unwrap()).unwrap()["Executed"]["body"]
                ["reconciled"],
            true
        );
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn provider_failure_is_cached_until_an_explicit_not_applied_resolution() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let mut sink = Sink::new(false);
        Arc::get_mut(&mut sink).unwrap().fail_first = true;
        let gw = gateway(store, clock, sink.clone(), false);
        let failed = gw
            .dispatch_durable(action(), None, config())
            .await
            .unwrap()
            .receipt;
        assert!(matches!(failed.outcome(), Some(ActionOutcome::Failed(_))));
        let replay = gw.dispatch_durable(action(), None, config()).await.unwrap();
        assert!(matches!(
            replay.receipt.outcome(),
            Some(ActionOutcome::Failed(_))
        ));
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
        gw.resolve_dispatch_receipt(
            "observability",
            "acme",
            "incident:1",
            resolution_request(failed.version, DispatchResolution::RetryNotApplied),
        )
        .await
        .unwrap();
        let retried = gw.dispatch_durable(action(), None, config()).await.unwrap();
        assert!(matches!(
            retried.receipt.outcome(),
            Some(ActionOutcome::Executed(_))
        ));
        assert_eq!(sink.calls.load(Ordering::SeqCst), 2);
        assert_eq!(retried.receipt.resolutions.len(), 1);
        assert_eq!(retried.receipt.resolutions[0].resolved_by.id, "operator");
        assert!(matches!(
            retried.receipt.resolutions[0].previous_status,
            DispatchReceiptStatus::Completed {
                outcome: ActionOutcome::Failed(_)
            }
        ));
    }

    #[tokio::test]
    async fn expired_worker_cannot_overwrite_a_reconciliation_decision() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(true);
        let gw = gateway(store, clock.clone(), sink.clone(), false);
        let first = {
            let gw = gw.clone();
            tokio::spawn(async move { gw.dispatch_durable(action(), None, config()).await })
        };
        tokio::time::timeout(Duration::from_secs(5), sink.entered.notified())
            .await
            .expect("provider was never invoked");
        clock.advance_to(Duration::from_secs(3)).unwrap();
        let receipt = gw
            .dispatch_durable(action(), None, config())
            .await
            .unwrap()
            .receipt;
        assert!(matches!(
            receipt.status,
            DispatchReceiptStatus::ReconciliationRequired
        ));
        sink.release.notify_one();
        assert!(matches!(
            first.await.unwrap(),
            Err(DispatchAdmissionError::LeaseLost)
        ));
        assert!(matches!(
            gw.get_dispatch_receipt("observability", "acme", "incident:1")
                .await
                .unwrap()
                .unwrap()
                .status,
            DispatchReceiptStatus::ReconciliationRequired
        ));
    }

    #[tokio::test]
    async fn prepared_chain_recovers_before_creation_without_reevaluating_rules() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(false);
        let gw = gateway(store.clone(), clock.clone(), sink.clone(), true);
        let (_, plan) = interrupt_chain_start(&gw, action()).await;
        drop(gw);
        clock.advance_to(Duration::from_secs(3)).unwrap();
        // Replacement deliberately has no incident rule or configured chain.
        let replacement = gateway(store, clock, sink.clone(), false);
        let result = replacement
            .dispatch_durable(action(), None, config())
            .await
            .unwrap();
        assert_eq!(result.receipt.chain_id(), Some(plan.chain_id.as_str()));
        assert!(
            matches!(result.receipt.outcome(), Some(ActionOutcome::ChainStarted { chain_id, .. }) if chain_id == &plan.chain_id)
        );
        Box::pin(replacement.advance_chain("observability", "acme", &plan.chain_id))
            .await
            .unwrap();
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn completed_chain_survives_lost_receipt_and_terminal_ttl_without_restarting() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(false);
        let gw = gateway(store.clone(), clock.clone(), sink.clone(), true);
        let (attempt, plan) = interrupt_chain_start(&gw, action()).await;
        gw.resume_admitted_chain(&plan, None).await.unwrap();
        Box::pin(gw.advance_chain("observability", "acme", &plan.chain_id))
            .await
            .unwrap();
        clock.advance_to(Duration::from_secs(10)).unwrap();
        assert_eq!(
            gw.get_chain_status("observability", "acme", &plan.chain_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            ChainStatus::Completed
        );
        drop(gw);
        let replacement = gateway(store, clock, sink.clone(), false);
        let result = replacement
            .dispatch_durable(action(), None, config())
            .await
            .unwrap();
        assert_eq!(result.receipt.chain_id(), Some(plan.chain_id.as_str()));
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
        let history = replacement
            .get_execution_history("observability", "acme", &plan.chain_id)
            .await
            .unwrap();
        assert_eq!(
            history
                .events
                .iter()
                .filter(|event| matches!(
                    event.event,
                    acteon_core::ExecutionEventType::ExecutionStarted { .. }
                ))
                .count(),
            1
        );
        assert!(matches!(
            replacement
                .complete_attempt(&attempt, ActionOutcome::Deduplicated)
                .await,
            Err(DispatchAdmissionError::LeaseLost)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn simultaneous_first_deliveries_have_one_receipt_and_one_provider_call() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(false);
        let gw = gateway(store, clock, sink.clone(), false);
        let barrier = Arc::new(tokio::sync::Barrier::new(20));
        let mut workers = Vec::new();
        for _ in 0..20 {
            let gw = gw.clone();
            let barrier = barrier.clone();
            workers.push(tokio::spawn(async move {
                barrier.wait().await;
                gw.dispatch_durable(action(), None, config()).await
            }));
        }
        for worker in workers {
            worker.await.unwrap().unwrap();
        }
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
        assert!(
            gw.get_dispatch_receipt("observability", "acme", "incident:1")
                .await
                .unwrap()
                .unwrap()
                .outcome()
                .is_some()
        );
    }

    #[tokio::test]
    async fn expired_attempt_is_rejected_before_verdict_execution() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(false);
        let gw = gateway(store, clock.clone(), sink.clone(), false);
        let mut receipt = gw
            .admit_dispatch(action(), None, config())
            .await
            .unwrap()
            .receipt;
        let attempt = DispatchAttempt {
            key: receipt_key("observability", "acme", "incident:1").unwrap(),
            token: "stale".into(),
        };
        receipt.status = DispatchReceiptStatus::Running {
            token: attempt.token.clone(),
            lease_until: gw.clock.now() + chrono::Duration::seconds(2),
        };
        gw.swap_receipt(&attempt.key, &mut receipt).await.unwrap();
        clock.advance_to(Duration::from_secs(3)).unwrap();
        assert!(
            gw.dispatch_pipeline(
                action(),
                None,
                false,
                DispatchOrigin::External,
                Some(&attempt),
                None,
            )
            .await
            .is_err()
        );
        assert_eq!(sink.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn conflicting_pinned_chain_version_cannot_execute_a_different_definition() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(false);
        let gw = gateway(store, clock, sink.clone(), true);
        let other = ChainConfig::new("incident").with_step(ChainStepConfig::new(
            "different",
            "sink",
            "notify",
            json!({"unexpected":true}),
        ));
        gw.pin_chain_definition("observability", "acme", &other)
            .await
            .unwrap();
        assert!(gw.dispatch_durable(action(), None, config()).await.is_err());
        assert_eq!(sink.calls.load(Ordering::SeqCst), 0);
        assert!(
            gw.get_dispatch_receipt("observability", "acme", "incident:1")
                .await
                .unwrap()
                .unwrap()
                .chain_id()
                .is_none()
        );
    }

    #[tokio::test]
    async fn oversized_result_keeps_an_ambiguous_receipt_without_reexecuting_effects() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let mut sink = Sink::new(false);
        Arc::get_mut(&mut sink).unwrap().response_bytes = 5000;
        let gw = gateway(store, clock.clone(), sink.clone(), false);
        let bounded = DispatchAdmissionConfig {
            max_receipt_bytes: 2048,
            ..config()
        };
        assert!(matches!(
            gw.dispatch_durable(action(), None, bounded.clone()).await,
            Err(DispatchAdmissionError::ReceiptCapacity { .. })
        ));
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
        clock.advance_to(Duration::from_secs(3)).unwrap();
        let retry = gw.dispatch_durable(action(), None, bounded).await.unwrap();
        assert!(matches!(
            retry.receipt.status,
            DispatchReceiptStatus::ReconciliationRequired
        ));
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn encrypted_receipt_replays_without_exposing_original_payload_in_storage() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(false);
        let encryption = Arc::new(acteon_crypto::PayloadEncryptor::new(
            acteon_crypto::parse_master_key(&"ab".repeat(32)).unwrap(),
        ));
        let build = || {
            GatewayBuilder::new()
                .clock(clock.clone())
                .state(store.clone())
                .lock(Arc::new(MemoryDistributedLock::with_clock(clock.clone())))
                .payload_encryptor(encryption.clone())
                .provider(sink.clone())
                .build()
                .unwrap()
        };
        let original = Action::new(
            "observability",
            "acme",
            "sink",
            "detect",
            json!({"secret":"private-feature-evidence"}),
        )
        .with_dedup_key("encrypted");
        let gw = build();
        gw.dispatch_durable(original.clone(), None, config())
            .await
            .unwrap();
        let key = receipt_key("observability", "acme", "encrypted").unwrap();
        let stored = store.get(&key).await.unwrap().unwrap();
        assert!(stored.starts_with("ENC["));
        assert!(!stored.contains("private-feature-evidence"));
        drop(gw);
        let replacement = build();
        let result = replacement
            .dispatch_durable(original.clone(), None, config())
            .await
            .unwrap();
        assert!(result.replayed);
        assert_eq!(result.receipt.action.payload, original.payload);
        assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn corrupt_cross_tenant_chain_plan_is_rejected_before_execution() {
        let clock = clock();
        let store = Arc::new(MemoryStateStore::with_clock(clock.clone()));
        let sink = Sink::new(false);
        let gw = gateway(store.clone(), clock.clone(), sink.clone(), true);
        let (attempt, _) = interrupt_chain_start(&gw, action()).await;
        let mut receipt = gw.load_receipt(&attempt.key).await.unwrap().unwrap();
        receipt.chain.as_mut().unwrap().action.tenant = "other".into();
        store
            .set(&attempt.key, &gw.encode_receipt(&receipt).unwrap(), None)
            .await
            .unwrap();
        clock.advance_to(Duration::from_secs(3)).unwrap();
        assert!(matches!(
            gw.dispatch_durable(action(), None, config()).await,
            Err(DispatchAdmissionError::Invalid(_))
        ));
        assert_eq!(sink.calls.load(Ordering::SeqCst), 0);
    }
}
