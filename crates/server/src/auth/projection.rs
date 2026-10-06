//! Credential policies derived from qualified actual provider effects.
//! Publication remains a privileged host boundary, not an execution permit.
use std::collections::BTreeMap;

use acteon_executor::catalog::QualifiedProviderCatalog;
use acteon_governance::configuration::{CredentialConfiguration, CredentialConfigurationReference};
use acteon_governance::credential::{CredentialAuthority, CredentialReference};
use acteon_governance::permit::{ExecutionPermit, PermitIssuanceCeiling};
use acteon_governance::{AuthorityCoordinator, AuthorityStamp, RootBudgetLimits};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::authority::AuthAuthority;
use super::authority::AuthenticatedConfiguration;
use super::config::AuthFileConfig;
use super::enrollment::AuthenticatedCredential;
use super::role::{Permission, Role};

#[derive(Debug, Clone)]
pub(super) struct PublishedScopeBinding {
    pub reference: CredentialConfigurationReference,
    pub policy_fingerprint: Option<String>,
}

pub(super) type ScopeReferences = BTreeMap<(String, String), PublishedScopeBinding>;

/// Original scope references captured with the actual authenticated credential.
/// Private construction and no deserializer prevent request labels from minting
/// this evidence. References are observations, never permanent permissions.
#[derive(Debug, Clone)]
pub struct AuthenticatedExecutionConfiguration {
    credential: AuthenticatedCredential,
    source: AuthenticatedConfiguration,
    scopes: ScopeReferences,
}

impl AuthenticatedExecutionConfiguration {
    pub(super) fn new(
        credential: AuthenticatedCredential,
        source: AuthenticatedConfiguration,
        scopes: ScopeReferences,
    ) -> Result<Self, String> {
        if credential.principal() != source.principal()
            || credential.auth_method() != source.auth_method()
            || scopes.values().any(|r| {
                r.reference.revision != source.reference().revision
                    || r.reference.source_id != source.reference().source_id
            })
        {
            return Err("execution configuration binding differs from authentication".into());
        }
        Ok(Self {
            credential,
            source,
            scopes,
        })
    }

    /// Verify the original authentication source independently of its scope projection.
    pub(crate) async fn verify_authentication_current(
        &self,
    ) -> Result<AuthorityStamp, acteon_governance::CoordinationError> {
        self.source.verify_current().await
    }

    pub(crate) fn management_scope(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<ScopedCredentialBinding, String> {
        if !self.credential.can_manage_scope(namespace, tenant) {
            return Err("original credential lacks scope management permission".into());
        }
        self.scope(namespace, tenant)
    }

    pub fn scope(&self, namespace: &str, tenant: &str) -> Result<ScopedCredentialBinding, String> {
        let configuration = self
            .scopes
            .get(&(namespace.into(), tenant.into()))
            .ok_or("authentication has no original projection for this scope")?;
        Ok(ScopedCredentialBinding {
            namespace: namespace.into(),
            tenant: tenant.into(),
            credential: self.credential.clone(),
            source: self.source.clone(),
            configuration: configuration.reference.clone(),
            policy_fingerprint: configuration.policy_fingerprint.clone(),
            reference: CredentialReference {
                id: self.credential.id().into(),
                accepted_revision: configuration.reference.revision,
            },
        })
    }
}

/// A scope-specific original credential observation. Root/effect adapters must
/// still intersect current permits, effect resources and budgets at their CAS.
#[derive(Debug, Clone)]
pub struct ScopedCredentialBinding {
    namespace: String,
    tenant: String,
    credential: AuthenticatedCredential,
    source: AuthenticatedConfiguration,
    configuration: CredentialConfigurationReference,
    policy_fingerprint: Option<String>,
    reference: CredentialReference,
}

impl ScopedCredentialBinding {
    pub(crate) fn matches_deployment_policy(&self, fingerprint: &str) -> bool {
        self.policy_fingerprint.as_deref() == Some(fingerprint)
    }

    #[must_use]
    pub fn credential_reference(&self) -> &CredentialReference {
        &self.reference
    }
    #[must_use]
    pub fn configuration_reference(&self) -> &CredentialConfigurationReference {
        &self.configuration
    }
    #[must_use]
    pub fn authentication_source(&self) -> &AuthenticatedConfiguration {
        &self.source
    }

    /// Observe all eligibility facts from one target-scope snapshot. The returned
    /// stamp must be used by execution admission; callers cannot refresh this
    /// binding to a newer configuration on behalf of an older identity.
    pub async fn verify_current(
        &self,
        coordinator: &AuthorityCoordinator,
        now_ms: i64,
    ) -> Result<AuthorityStamp, String> {
        self.verify_scope(coordinator, now_ms, false, false).await
    }

    pub(crate) async fn verify_execution_scope(
        &self,
        coordinator: &AuthorityCoordinator,
        now_ms: i64,
    ) -> Result<AuthorityStamp, String> {
        self.verify_scope(coordinator, now_ms, true, false).await
    }

    pub(crate) fn verify_management_snapshot(
        &self,
        state: &acteon_governance::CoordinatorSnapshot,
        now_ms: i64,
    ) -> Result<AuthorityStamp, String> {
        self.verify_scope_snapshot(state, now_ms, true, true)
    }

    async fn verify_scope(
        &self,
        coordinator: &AuthorityCoordinator,
        now_ms: i64,
        require_execution: bool,
        allow_disabled: bool,
    ) -> Result<AuthorityStamp, String> {
        let state = coordinator
            .snapshot()
            .await
            .map_err(|_| "scope authority unavailable")?;
        self.verify_scope_snapshot(&state, now_ms, require_execution, allow_disabled)
    }

    fn verify_scope_snapshot(
        &self,
        state: &acteon_governance::CoordinatorSnapshot,
        now_ms: i64,
        require_execution: bool,
        allow_disabled: bool,
    ) -> Result<AuthorityStamp, String> {
        let head = state
            .credential_configurations
            .get(&self.configuration.source_id)
            .ok_or("scope configuration missing")?;
        let credential = state
            .credentials
            .get(&self.reference.id)
            .ok_or("scope credential missing")?;
        if (require_execution && state.purpose != acteon_governance::ScopePurpose::Execution)
            || state.namespace != self.namespace
            || state.tenant != self.tenant
            || state.incarnation != self.configuration.incarnation
            || head.revision != self.configuration.revision
            || head.digest != self.configuration.digest
            || !head.owned_credentials.contains(&self.reference.id)
            || credential.revoked
            || (!allow_disabled && !credential.authority.execution_enabled)
            || state
                .revoked_subjects
                .contains(self.credential.principal().id())
            || credential.authority.ceiling.subject != *self.credential.principal()
            || credential.authority.auth_method != self.credential.auth_method()
            || credential.authority.ceiling.revision != self.reference.accepted_revision
            || now_ms < credential.authority.ceiling.valid_from_ms
            || now_ms >= credential.authority.ceiling.limits.deadline_ms
        {
            return Err("original scope credential binding is no longer eligible".into());
        }
        Ok(state.stamp())
    }
}

/// Immutable inputs for one scope. The independent publication ceiling must
/// cover new policies and removal of historical source-owned policies. It is
/// never inferred from the incoming credential grants or borrowed from state.
pub struct CredentialPolicyProjector {
    coordinator: AuthorityCoordinator,
    namespace: String,
    tenant: String,
    catalog: QualifiedProviderCatalog,
    issuance: PermitIssuanceCeiling,
    valid_from_ms: i64,
    limits: RootBudgetLimits,
    deployment_policy_fingerprint: Option<String>,
    chain_subjects: BTreeMap<String, Vec<acteon_core::PrincipalIdentity>>,
}

impl CredentialPolicyProjector {
    pub async fn new_trusted(
        coordinator: AuthorityCoordinator,
        catalog: QualifiedProviderCatalog,
        issuance: PermitIssuanceCeiling,
        valid_from_ms: i64,
        limits: RootBudgetLimits,
    ) -> Result<Self, String> {
        Self::new_scoped(coordinator, catalog, issuance, valid_from_ms, limits, false).await
    }

    /// Internal projection for explicitly validated retained-history deployments.
    /// No provider effects are projected into executable credentials.
    pub(crate) async fn for_history(
        coordinator: AuthorityCoordinator,
        issuance: PermitIssuanceCeiling,
        valid_from_ms: i64,
        limits: RootBudgetLimits,
    ) -> Result<Self, String> {
        Self::new_scoped(
            coordinator,
            QualifiedProviderCatalog::for_history(),
            issuance,
            valid_from_ms,
            limits,
            true,
        )
        .await
    }

    async fn new_scoped(
        coordinator: AuthorityCoordinator,
        catalog: QualifiedProviderCatalog,
        mut issuance: PermitIssuanceCeiling,
        valid_from_ms: i64,
        limits: RootBudgetLimits,
        history_only: bool,
    ) -> Result<Self, String> {
        issuance
            .validate()
            .map_err(|_| "invalid publication ceiling")?;
        for effect in &mut issuance.effects {
            effect.resources.sort();
        }
        let scope = coordinator
            .snapshot()
            .await
            .map_err(|_| "scope authority unavailable")?;
        if matches!(
            scope.purpose,
            acteon_governance::ScopePurpose::AuthenticationControl { .. }
        ) {
            return Err("authentication-control scope cannot host execution projection".into());
        }
        let definitions = catalog.definitions(&scope.namespace, &scope.tenant);
        if definitions.is_empty() != history_only
            || valid_from_ms < 0
            || valid_from_ms < issuance.valid_from_ms
            || limits.max_units == 0
            || limits.max_concurrent == 0
            || limits.deadline_ms <= valid_from_ms
            || limits.max_units > issuance.limits.max_units
            || limits.max_concurrent > issuance.limits.max_concurrent
            || limits.deadline_ms > issuance.limits.deadline_ms
            || issuance
                .effects
                .iter()
                .flat_map(|e| &e.resources)
                .any(|r| r.namespace() != scope.namespace || r.tenant() != scope.tenant)
            || definitions
                .iter()
                .any(|d| !issuance.effects.contains(&d.effect))
        {
            return Err("scope projection exceeds independent publication ceiling".into());
        }
        Ok(Self {
            coordinator,
            namespace: scope.namespace,
            tenant: scope.tenant,
            catalog,
            issuance,
            valid_from_ms,
            limits,
            deployment_policy_fingerprint: None,
            chain_subjects: BTreeMap::new(),
        })
    }

    pub(super) fn deployment_policy_fingerprint(&self) -> Option<&str> {
        self.deployment_policy_fingerprint.as_deref()
    }

    pub(crate) fn with_deployment_policy_fingerprint(mut self, fingerprint: String) -> Self {
        self.deployment_policy_fingerprint = Some(fingerprint);
        self
    }

    /// Independently declared chain rights, separate from provider grants.
    pub(crate) fn with_chain_admission_bounds(
        mut self,
        bounds: BTreeMap<String, Vec<acteon_core::PrincipalIdentity>>,
    ) -> Result<Self, String> {
        if bounds.len() > 128 {
            return Err("too many chain admission bounds".into());
        }
        for (name, subjects) in &bounds {
            let effect = acteon_governance::context::AcceptedEffect {
                operation: "chain.start".into(),
                resources: vec![
                    acteon_core::ResourceRef::new(
                        acteon_core::ResourceKind::Chain,
                        &self.namespace,
                        &self.tenant,
                        name,
                    )
                    .map_err(|_| "invalid chain admission bound")?,
                ],
            };
            if subjects.is_empty()
                || subjects.len() > 16
                || !self.issuance.effects.contains(&effect)
                || subjects
                    .iter()
                    .any(|subject| !self.issuance.subjects.contains(subject))
            {
                return Err("chain admission exceeds publication bounds".into());
            }
        }
        self.chain_subjects = bounds;
        Ok(self)
    }
    #[must_use]
    pub fn scope(&self) -> (&str, &str) {
        (&self.namespace, &self.tenant)
    }

    fn chain_effects(
        &self,
        principal: &acteon_core::PrincipalIdentity,
        grants: &[super::config::Grant],
    ) -> Result<Vec<acteon_governance::context::AcceptedEffect>, String> {
        self.chain_subjects
            .iter()
            .filter(|(_, subjects)| {
                subjects.contains(principal)
                    && grants.iter().any(|grant| {
                        super::config::tenant_matches(&grant.tenants, &self.tenant)
                            && grant
                                .namespaces
                                .iter()
                                .any(|n| n == "*" || n == &self.namespace)
                    })
            })
            .map(|(name, _)| {
                Ok(acteon_governance::context::AcceptedEffect {
                    operation: "chain.start".into(),
                    resources: vec![
                        acteon_core::ResourceRef::new(
                            acteon_core::ResourceKind::Chain,
                            &self.namespace,
                            &self.tenant,
                            name,
                        )
                        .map_err(|_| "invalid chain admission bound")?,
                    ],
                })
            })
            .collect()
    }

    /// Resolve each logical credential separately. The complete configuration
    /// fingerprint includes non-execution grants and rotation through the auth
    /// source, plus qualification and fixed per-root limits through this scope.
    pub fn project(
        &self,
        authority: &AuthAuthority,
        config: &AuthFileConfig,
    ) -> Result<CredentialConfiguration, String> {
        if authority.control_scope() == self.scope() {
            return Err("execution projection must not share the auth control scope".into());
        }
        super::AuthProvider::build_tables(config)?;
        let source = authority.configuration(config)?;
        let definitions = self.catalog.definitions(&self.namespace, &self.tenant);
        let mut credentials = BTreeMap::new();
        for (id, principal, role, grants, method) in config
            .users
            .iter()
            .map(|u| {
                (
                    u.authority_id.as_deref(),
                    u.principal.as_ref(),
                    &u.role,
                    &u.grants,
                    "jwt",
                )
            })
            .chain(config.api_keys.iter().map(|k| {
                (
                    k.authority_id.as_deref(),
                    k.principal.as_ref(),
                    &k.role,
                    &k.grants,
                    "api_key",
                )
            }))
        {
            let id = id.ok_or("execution scope projection requires enrolled credentials")?;
            let principal =
                principal.ok_or("execution scope projection requires stable principals")?;
            let role = Role::from_str_loose(role).ok_or("invalid credential role")?;
            let mut effects = if role.has_permission(Permission::Dispatch) {
                definitions
                    .iter()
                    .filter(|d| {
                        grants.iter().any(|g| {
                            g.matches(&d.tenant, &d.namespace, &d.provider, &d.action_type)
                        })
                    })
                    .map(|d| d.effect.clone())
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            if role.has_permission(Permission::Dispatch) {
                effects.extend(self.chain_effects(principal, grants)?);
            }
            if !self.issuance.subjects.contains(principal) {
                if effects.is_empty() {
                    // A shared auth file can contain actors belonging only to
                    // other scopes. Do not enroll them under this publisher.
                    continue;
                }
                return Err("credential principal exceeds publication ceiling".into());
            }
            let credential = CredentialAuthority {
                ceiling: ExecutionPermit {
                    id: id.into(),
                    revision: source.revision,
                    subject: principal.clone(),
                    effects,
                    valid_from_ms: self.valid_from_ms,
                    limits: self.limits.clone(),
                },
                auth_method: method.into(),
                execution_enabled: false,
            };
            let mut credential = credential;
            credential.execution_enabled = !credential.ceiling.effects.is_empty();
            credentials.entry(id.to_owned()).or_insert(credential);
        }
        let mut fingerprint = json!({
            "format": "acteon.auth.execution_projection.v1",
            "auth_fingerprint": source.configuration_fingerprint,
            "namespace": self.namespace, "tenant": self.tenant,
            "catalog": self.catalog.fingerprint(),
            "issuer": self.issuance.issuer,
            "valid_from_ms": self.valid_from_ms, "limits": self.limits,
        });
        if !self.chain_subjects.is_empty() {
            fingerprint["chain_subjects"] = json!(self.chain_subjects);
        }
        if let Some(policy) = &self.deployment_policy_fingerprint {
            fingerprint
                .as_object_mut()
                .ok_or("invalid execution projection")?
                .insert("deployment_policy".into(), json!(policy));
        }
        let bytes = serde_json::to_vec(&fingerprint).map_err(|_| "invalid execution projection")?;
        Ok(CredentialConfiguration {
            source_id: source.source_id,
            revision: source.revision,
            configuration_fingerprint: format!("{:x}", Sha256::digest(bytes)),
            credentials: credentials.into_values().collect(),
        })
    }

    /// One-scope publication using its actual coordinator CAS. Lost acknowledgments
    /// reconcile with identical input; this is not a multi-scope transaction.
    pub async fn publish(
        &self,
        authority: &AuthAuthority,
        config: &AuthFileConfig,
        now_ms: i64,
    ) -> Result<CredentialConfigurationReference, String> {
        let configuration = self.project(authority, config)?;
        let snapshot = self
            .coordinator
            .snapshot()
            .await
            .map_err(|_| "scope authority unavailable")?;
        if matches!(
            snapshot.purpose,
            acteon_governance::ScopePurpose::AuthenticationControl { .. }
        ) || (self.deployment_policy_fingerprint.is_some()
            && snapshot.purpose != acteon_governance::ScopePurpose::Execution)
        {
            return Err("scope purpose does not permit execution projection".into());
        }
        let expected = snapshot
            .credential_configurations
            .get(&configuration.source_id)
            .map_or(0, |record| record.revision);
        let reference = configuration
            .reference(&snapshot.stamp())
            .map_err(|_| "invalid scope reference")?;
        let change_id = format!(
            "scope-auth/{}/{}",
            configuration.source_id, configuration.revision
        );
        self.coordinator
            .publish_credential_configuration(
                &change_id,
                &configuration,
                expected,
                &self.issuance,
                &snapshot.stamp(),
                "trusted authentication scope projection",
                now_ms,
            )
            .await
            .map_err(|_| "scope publication refused; reconcile security revision")?;
        Ok(reference)
    }
}
