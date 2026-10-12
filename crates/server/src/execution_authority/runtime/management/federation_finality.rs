//! Deployment-owned peer finality verification and authorized acceptance.

use std::{collections::BTreeMap, sync::Arc};

use acteon_executor::{
    federation_finality::{FederationFinalityCorrelation, FederationFinalityVerifier},
    federation_transport::{
        FederationDeliveryEvidence, FederationDeliveryEvidenceReader,
        FederationDeliveryEvidenceStatus, FederationDeliveryFinalityReconciler,
        FederationFinalityAcceptance,
    },
};
use acteon_governance::AttemptStatus;

use super::federation_delivery_covered;
use super::{ExecutionAuthorityRuntime, ManagementError, federation_management_error};
use crate::auth::projection::AuthenticatedExecutionConfiguration;

/// One independently reviewed finality source for an exact outbound profile
/// epoch. The runtime resolves the adapter binding; configuration cannot point
/// the verifier at an unrelated digest.
pub struct TrustedFederationFinalityInstallation {
    pub namespace: String,
    pub tenant: String,
    pub profile_id: String,
    pub issuer_epoch: u64,
    pub verifier: Arc<dyn FederationFinalityVerifier>,
}

impl ExecutionAuthorityRuntime {
    /// Install the complete trusted set before serving requests. Each declared
    /// profile epoch resolves to the binding digest built by the runtime.
    pub fn with_trusted_federation_finality_verifiers(
        mut self,
        installations: Vec<TrustedFederationFinalityInstallation>,
    ) -> Result<Self, String> {
        if installations.is_empty() || installations.len() > 128 {
            return Err("invalid federation finality installation".into());
        }
        let mut grouped: BTreeMap<
            (String, String),
            BTreeMap<String, Arc<dyn FederationFinalityVerifier>>,
        > = BTreeMap::new();
        for installation in installations {
            let scope_key = (installation.namespace, installation.tenant);
            let scope = self
                .scopes
                .get(&scope_key)
                .ok_or("undeclared federation finality scope")?;
            let matches = scope
                .federation_exports
                .values()
                .filter(|export| export.declaration.id == installation.profile_id)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err("unknown federation finality export profile".into());
            }
            let binding_digest = matches[0]
                .binding_digests
                .get(&installation.issuer_epoch)
                .ok_or("unknown federation finality export epoch")?
                .clone();
            if grouped
                .entry(scope_key)
                .or_default()
                .insert(binding_digest, installation.verifier)
                .is_some()
            {
                return Err("duplicate federation finality binding".into());
            }
        }
        for (key, verifiers) in grouped {
            let scope = self
                .scopes
                .get_mut(&key)
                .ok_or("undeclared federation finality scope")?;
            if scope.federation_finality.is_some() {
                return Err("federation finality scope already installed".into());
            }
            scope.federation_finality = Some(
                FederationDeliveryFinalityReconciler::new_trusted(
                    self.state.clone(),
                    self.clock.clone(),
                    self.encryptor.clone(),
                    verifiers,
                )
                .map_err(|_| "invalid federation finality qualification")?,
            );
        }
        Ok(self)
    }

    /// Read exact correlation for an eligible ambiguous delivery. This exposes
    /// no signed payload, credential, endpoint, message body, or replay handle.
    pub async fn federation_finality_correlation(
        &self,
        namespace: &str,
        tenant: &str,
        delivery_id: uuid::Uuid,
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<FederationFinalityCorrelation, ManagementError> {
        let observation = self
            .management_observation(namespace, tenant, authentication)
            .await?;
        if !observation.policy.can_reconcile {
            return Err(ManagementError::Forbidden);
        }
        let evidence = self
            .authorized_federation_evidence(&observation, namespace, tenant, delivery_id)
            .await?;
        let reconciler = observation
            .scope
            .federation_finality
            .as_ref()
            .ok_or(ManagementError::Unavailable)?;
        let correlation = reconciler
            .correlation(namespace, tenant, delivery_id, &evidence.source)
            .await
            .map_err(|error| federation_management_error(&error))?;
        self.revalidate_management(&observation, authentication)
            .await?;
        Ok(correlation)
    }

    /// Accept qualified peer evidence without sending or replaying anything.
    /// Journal evidence commits before source accounting is settled, so exact
    /// proof replay repairs a lost settlement acknowledgment.
    pub async fn reconcile_federation_delivery(
        &self,
        namespace: &str,
        tenant: &str,
        delivery_id: uuid::Uuid,
        proof: &[u8],
        authentication: &AuthenticatedExecutionConfiguration,
    ) -> Result<FederationDeliveryEvidence, ManagementError> {
        let observation = self
            .management_observation(namespace, tenant, authentication)
            .await?;
        if !observation.policy.can_reconcile {
            return Err(ManagementError::Forbidden);
        }
        let evidence = self
            .authorized_federation_evidence(&observation, namespace, tenant, delivery_id)
            .await?;
        let attempt_id =
            acteon_executor::federation_export::federation_export_attempt_id(delivery_id);
        let attempt = observation
            .scope
            .coordinator
            .snapshot()
            .await
            .map_err(|_| ManagementError::Unavailable)?
            .starts
            .get(&attempt_id)
            .cloned()
            .ok_or(ManagementError::Conflict)?;
        if attempt.subject != evidence.source.id() {
            return Err(ManagementError::Conflict);
        }
        self.revalidate_management(&observation, authentication)
            .await?;
        let reconciler = observation
            .scope
            .federation_finality
            .as_ref()
            .ok_or(ManagementError::Unavailable)?;
        let accepted = reconciler
            .reconcile(
                namespace,
                tenant,
                delivery_id,
                &evidence.source,
                proof,
                FederationFinalityAcceptance {
                    operator: observation.policy.principal.clone(),
                    authority: observation.stamp.clone(),
                    accepted_at_ms: self.clock.now().timestamp_millis(),
                },
            )
            .await
            .map_err(|error| federation_management_error(&error))?;
        if matches!(
            accepted.status,
            FederationDeliveryEvidenceStatus::Accepted
                | FederationDeliveryEvidenceStatus::Rejected { .. }
        ) {
            observation
                .scope
                .coordinator
                .settle(&attempt_id, &attempt.token, AttemptStatus::Settled)
                .await?;
        }
        Ok(accepted)
    }

    async fn authorized_federation_evidence(
        &self,
        observation: &super::ManagementObservation<'_>,
        namespace: &str,
        tenant: &str,
        delivery_id: uuid::Uuid,
    ) -> Result<FederationDeliveryEvidence, ManagementError> {
        let evidence = FederationDeliveryEvidenceReader::new(
            self.state.clone(),
            self.clock.clone(),
            self.encryptor.clone(),
        )
        .get_evidence(namespace, tenant, delivery_id)
        .await
        .map_err(|error| federation_management_error(&error))?
        .ok_or(ManagementError::NotFound)?;
        if !federation_delivery_covered(observation.policy, &evidence) {
            return Err(ManagementError::NotFound);
        }
        Ok(evidence)
    }
}
