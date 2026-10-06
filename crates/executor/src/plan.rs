//! Host-qualified durable plans. Describing a plan does not grant authority.
pub mod engine;
pub mod handoff;

use crate::{
    catalog::{QualifiedProviderCatalog, QualifiedProviderDefinition},
    governed::governed_provider_input_digest,
    mediation::{
        ProviderExecutionAdmission, ProviderExecutionAuthority, ProviderInvocation,
        ProviderInvocationOrigin,
    },
};
use acteon_core::{
    Action, ActionError, ChainConfig, ChainStepConfig, ResourceKind, ResourceRef, StepKind,
};
use acteon_governance::context::{
    AcceptedEffect, ChildContextAdmission, ContextError, ExecutionContextHandle,
    TrustedContextStore, VerifiedExecutionContext,
};
use acteon_governance::{
    RootBudgetLimits,
    permit::{PermitReference, matches_effect},
};
use acteon_provider::DynProvider;
use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const MAX_PLAN_STEPS: usize = 128;
const MAX_PLAN_DEPTH: usize = 16;
const MAX_DEFINITION_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanCallSite {
    Step { chain: String, path: Vec<String> },
    Cancellation { chain: String },
}
impl PlanCallSite {
    fn chain(&self) -> &str {
        match self {
            Self::Step { chain, .. } | Self::Cancellation { chain } => chain,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("plan definition, scope, graph or input is invalid")]
    Invalid,
    #[error("plan exceeds bounded qualification capacity")]
    Capacity,
    #[error("plan contains an unsupported execution adapter")]
    Unsupported,
    #[error("plan route or actual provider is not qualified")]
    Unqualified,
    #[error("accepted plan/input/ancestry differs from this work")]
    Binding,
}

/// Immutable host construction from complete definitions and live route bindings.
/// It deliberately has no deserializer. Every reachable branch and cancellation
/// target must qualify, including provider calls inside parallel and sub-chains.
#[derive(Clone)]
pub struct QualifiedChainPlan {
    namespace: String,
    tenant: String,
    entry: String,
    definitions: BTreeMap<String, ChainConfig>,
    routes: BTreeMap<PlanCallSite, QualifiedProviderDefinition>,
    edges: BTreeMap<String, BTreeSet<String>>,
    catalog: QualifiedProviderCatalog,
    effects: Vec<AcceptedEffect>,
    digest: String,
}
impl QualifiedChainPlan {
    pub fn new_trusted(
        namespace: &str,
        tenant: &str,
        entry: &str,
        definitions: &BTreeMap<String, ChainConfig>,
        catalog: QualifiedProviderCatalog,
    ) -> Result<Self, PlanError> {
        let mut builder = PlanBuilder {
            namespace,
            tenant,
            source: definitions,
            available: catalog.definitions(namespace, tenant),
            definitions: BTreeMap::new(),
            routes: BTreeMap::new(),
            edges: BTreeMap::new(),
            visiting: BTreeSet::new(),
            steps: 0,
        };
        builder.visit_chain(entry, 0)?;
        let bytes = canonical_bytes(&serde_json::json!({
            "format":"acteon-qualified-chain:v1", "namespace":namespace, "tenant":tenant,
            "entry":entry, "definitions":builder.definitions,
            "routes":builder.routes.iter().collect::<Vec<_>>(),
        }))?;
        if bytes.len() > MAX_DEFINITION_BYTES {
            return Err(PlanError::Capacity);
        }
        let mut effects = Vec::new();
        for name in builder.definitions.keys() {
            effects.push(AcceptedEffect {
                operation: "chain.start".into(),
                resources: vec![chain_resource(namespace, tenant, name)?],
            });
        }
        for route in builder.routes.values() {
            if !effects.iter().any(|e| matches_effect(e, &route.effect)) {
                effects.push(route.effect.clone());
            }
        }
        if effects.len() > MAX_PLAN_STEPS {
            return Err(PlanError::Capacity);
        }
        Ok(Self {
            namespace: namespace.into(),
            tenant: tenant.into(),
            entry: entry.into(),
            definitions: builder.definitions,
            routes: builder.routes,
            edges: builder.edges,
            catalog,
            effects,
            digest: format!("{:x}", Sha256::digest(bytes)),
        })
    }
    #[must_use]
    pub fn definition_digest(&self) -> &str {
        &self.digest
    }
    #[must_use]
    pub fn required_effects(&self) -> &[AcceptedEffect] {
        &self.effects
    }
    #[must_use]
    pub fn definitions(&self) -> &BTreeMap<String, ChainConfig> {
        &self.definitions
    }

    /// Bind the complete qualified definition to the actual original input.
    /// Admission must independently authorize every required effect and limits.
    pub fn bind_input(
        self: &Arc<Self>,
        origin: &Action,
    ) -> Result<QualifiedChainInvocation, PlanError> {
        if origin.namespace.as_str() != self.namespace || origin.tenant.as_str() != self.tenant {
            return Err(PlanError::Binding);
        }
        Ok(QualifiedChainInvocation {
            plan: self.clone(),
            request_digest: plan_request_digest(&self.digest, origin)?,
        })
    }
    fn restrictions(
        &self,
        site: &PlanCallSite,
        path: &[String],
    ) -> Result<Vec<ResourceRef>, PlanError> {
        if path.first().map(String::as_str) != Some(self.entry.as_str())
            || path.last().map(String::as_str) != Some(site.chain())
            || path.len() > MAX_PLAN_DEPTH
            || path.iter().collect::<BTreeSet<_>>().len() != path.len()
            || path.windows(2).any(|pair| {
                !self
                    .edges
                    .get(&pair[0])
                    .is_some_and(|e| e.contains(&pair[1]))
            })
        {
            return Err(PlanError::Binding);
        }
        path.iter()
            .map(|name| chain_resource(&self.namespace, &self.tenant, name))
            .collect()
    }
}

/// Host-derived original job input binding, not an execution grant.
pub struct QualifiedChainInvocation {
    plan: Arc<QualifiedChainPlan>,
    request_digest: String,
}
/// Inputs come from the trusted pinned work record and actual selected call.
/// Public action labels cannot supply parent/root/actor/payer or qualification.
pub struct PlanChildCapture<'a> {
    pub root: &'a VerifiedExecutionContext,
    pub parent: &'a VerifiedExecutionContext,
    pub call_site: &'a PlanCallSite,
    pub chain_path: &'a [String],
    pub action: &'a Action,
    pub selected: &'a Arc<dyn DynProvider>,
    pub admission_key: &'a str,
    pub handle: ExecutionContextHandle,
    pub execution_id: uuid::Uuid,
    pub permits: &'a [PermitReference],
    pub limits: RootBudgetLimits,
    pub clock: &'a dyn acteon_time::Clock,
}
#[derive(Debug, thiserror::Error)]
pub enum PlanAdmissionError {
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Context(#[from] ContextError),
}
impl QualifiedChainInvocation {
    #[must_use]
    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }
    pub fn verify_root(&self, root: &VerifiedExecutionContext) -> Result<(), PlanError> {
        if root.root_execution_id() != root.execution_id()
            || root
                .reference()
                .map_err(|_| PlanError::Binding)?
                .request_digest()
                != self.request_digest
            || self
                .plan
                .effects
                .iter()
                .any(|e| !root.within_accepted_ceiling(e))
        {
            return Err(PlanError::Binding);
        }
        Ok(())
    }
    /// Persist a signed actual-input child before handoff. Effect authorization
    /// and enclosing-resource closures are checked at effect registration CAS.
    pub async fn capture_provider_child(
        &self,
        contexts: &TrustedContextStore,
        request: PlanChildCapture<'_>,
    ) -> Result<VerifiedExecutionContext, PlanAdmissionError> {
        self.verify_root(request.root)?;
        if request.parent.root_execution_id() != request.root.execution_id()
            || request.parent.principal() != request.root.principal()
            || request.parent.representation() != request.root.representation()
        {
            return Err(PlanError::Binding.into());
        }
        let declared = self
            .plan
            .routes
            .get(request.call_site)
            .ok_or(PlanError::Binding)?;
        let actual = self
            .plan
            .catalog
            .resolve(request.action, request.selected)
            .map_err(|_| PlanError::Unqualified)?;
        if !matches_effect(actual.effect(), &declared.effect) {
            return Err(PlanError::Unqualified.into());
        }
        let digest =
            governed_provider_input_digest(request.action).map_err(|_| PlanError::Invalid)?;
        let restrictions = self
            .plan
            .restrictions(request.call_site, request.chain_path)?;
        let resources: BTreeSet<_> = declared
            .effect
            .resources
            .iter()
            .chain(&restrictions)
            .collect();
        if resources.len() > 16 {
            return Err(PlanError::Capacity.into());
        }
        if request.admission_key.is_empty()
            || request.admission_key.len() > 512
            || request.admission_key.trim() != request.admission_key
            || request.admission_key.chars().any(char::is_control)
        {
            return Err(PlanError::Invalid.into());
        }
        let key_bytes = canonical_bytes(&serde_json::json!({
            "format":"acteon-plan-child-admission:v1", "root":request.root.execution_id(),
            "plan_input":self.request_digest, "call_site":request.call_site,
            "chain_path":request.chain_path, "admission_key":request.admission_key,
        }))?;
        let admission_key = format!("{:x}", Sha256::digest(key_bytes));
        Ok(contexts
            .capture_child(ChildContextAdmission {
                admission_key: &admission_key,
                parent: request.parent,
                handle: request.handle,
                execution_id: request.execution_id,
                request_digest: digest,
                accepted_effects: vec![declared.effect.clone()],
                restrictions,
                permits: request.permits,
                limits: request.limits,
                clock: request.clock,
            })
            .await?)
    }
}

/// Trusted persisted work identity for one actual planned call site. Retain
/// these candidates and the admission key through admission repair and retry.
/// Public wire references alone must never construct this host adapter.
pub struct PlanProviderAdmission<'a> {
    pub root: &'a VerifiedExecutionContext,
    pub parent: &'a VerifiedExecutionContext,
    pub call_site: &'a PlanCallSite,
    pub chain_path: &'a [String],
    pub admission_key: &'a str,
    pub handle: ExecutionContextHandle,
    pub execution_id: uuid::Uuid,
    pub permits: &'a [PermitReference],
    pub limits: RootBudgetLimits,
    pub clock: &'a dyn acteon_time::Clock,
}
pub struct PlannedProviderAdmission<'a> {
    plan: &'a QualifiedChainInvocation,
    contexts: &'a TrustedContextStore,
    request: PlanProviderAdmission<'a>,
}
impl QualifiedChainInvocation {
    pub fn provider_admission<'a>(
        &'a self,
        contexts: &'a TrustedContextStore,
        request: PlanProviderAdmission<'a>,
    ) -> PlannedProviderAdmission<'a> {
        PlannedProviderAdmission {
            plan: self,
            contexts,
            request,
        }
    }
}
#[async_trait]
impl ProviderExecutionAdmission for PlannedProviderAdmission<'_> {
    async fn admit(
        &self,
        invocation: ProviderInvocation<'_>,
    ) -> Result<ProviderExecutionAuthority, ActionError> {
        let refusal = |retryable| ActionError {
            code: "PLANNED_PROVIDER_ADMISSION_DENIED".into(),
            message: "Planned provider work could not be admitted".into(),
            retryable,
            attempts: 0,
        };
        let expected = match self.request.call_site {
            PlanCallSite::Step { .. } => ProviderInvocationOrigin::ChainStep,
            PlanCallSite::Cancellation { .. } => ProviderInvocationOrigin::ChainCancellation,
        };
        if invocation.origin != expected
            || invocation.context.is_some()
            || invocation.authority.is_some()
        {
            return Err(refusal(false));
        }
        let context = self
            .plan
            .capture_provider_child(
                self.contexts,
                PlanChildCapture {
                    root: self.request.root,
                    parent: self.request.parent,
                    call_site: self.request.call_site,
                    chain_path: self.request.chain_path,
                    action: invocation.action,
                    selected: invocation.selected,
                    admission_key: self.request.admission_key,
                    handle: self.request.handle.clone(),
                    execution_id: self.request.execution_id,
                    permits: self.request.permits,
                    limits: self.request.limits.clone(),
                    clock: self.request.clock,
                },
            )
            .await
            .map_err(|error| {
                refusal(matches!(
                    error,
                    PlanAdmissionError::Context(
                        ContextError::State(_)
                            | ContextError::Coordination(
                                acteon_governance::CoordinationError::State(_)
                                    | acteon_governance::CoordinationError::Contention
                            )
                    )
                ))
            })?;
        Ok(ProviderExecutionAuthority::new_trusted(
            context.reference().map_err(|_| refusal(false))?,
            context.principal().clone(),
            self.request.permits.to_vec(),
        ))
    }
}

struct PlanBuilder<'a> {
    namespace: &'a str,
    tenant: &'a str,
    source: &'a BTreeMap<String, ChainConfig>,
    available: Vec<QualifiedProviderDefinition>,
    definitions: BTreeMap<String, ChainConfig>,
    routes: BTreeMap<PlanCallSite, QualifiedProviderDefinition>,
    edges: BTreeMap<String, BTreeSet<String>>,
    visiting: BTreeSet<String>,
    steps: usize,
}
impl PlanBuilder<'_> {
    fn visit_chain(&mut self, name: &str, depth: usize) -> Result<(), PlanError> {
        if depth >= MAX_PLAN_DEPTH || self.visiting.contains(name) {
            return Err(PlanError::Invalid);
        }
        if self.definitions.contains_key(name) {
            return Ok(());
        }
        let definition = self.source.get(name).ok_or(PlanError::Invalid)?;
        bounded_step_tree(&definition.steps)?;
        let definition = definition.clone();
        if definition.name != name || !definition.validate().is_empty() {
            return Err(PlanError::Invalid);
        }
        chain_resource(self.namespace, self.tenant, name)?;
        self.visiting.insert(name.into());
        for step in &definition.steps {
            self.visit_step(name, step, vec![step.name.clone()], depth)?;
        }
        if let Some(target) = &definition.on_cancel {
            self.route(
                PlanCallSite::Cancellation { chain: name.into() },
                &target.provider,
                &target.action_type,
            )?;
        }
        self.visiting.remove(name);
        self.definitions.insert(name.into(), definition);
        Ok(())
    }
    fn visit_step(
        &mut self,
        chain: &str,
        step: &ChainStepConfig,
        path: Vec<String>,
        depth: usize,
    ) -> Result<(), PlanError> {
        self.steps += 1;
        if self.steps > MAX_PLAN_STEPS || path.len() >= MAX_PLAN_DEPTH {
            return Err(PlanError::Capacity);
        }
        match step.kind() {
            StepKind::Provider => self.route(
                PlanCallSite::Step {
                    chain: chain.into(),
                    path,
                },
                &step.provider,
                &step.action_type,
            ),
            StepKind::Timer(_) | StepKind::Signal(_) => Ok(()),
            StepKind::Parallel(group) => {
                for child in &group.steps {
                    let mut child_path = path.clone();
                    child_path.push(child.name.clone());
                    self.visit_step(chain, child, child_path, depth)?;
                }
                Ok(())
            }
            StepKind::SubChain(name) => {
                self.edges
                    .entry(chain.into())
                    .or_default()
                    .insert(name.to_string());
                self.visit_chain(name, depth + 1)
            }
            StepKind::Worker(_) | StepKind::Dispatch(_) => Err(PlanError::Unsupported),
        }
    }
    fn route(&mut self, site: PlanCallSite, provider: &str, action: &str) -> Result<(), PlanError> {
        let definition = self
            .available
            .iter()
            .find(|d| d.provider == provider && d.action_type == action)
            .ok_or(PlanError::Unqualified)?
            .clone();
        if self.routes.insert(site, definition).is_some() {
            return Err(PlanError::Invalid);
        }
        Ok(())
    }
}
fn bounded_step_tree(steps: &[ChainStepConfig]) -> Result<(), PlanError> {
    let mut pending: Vec<_> = steps.iter().map(|s| (s, 1)).collect();
    let mut count = 0;
    while let Some((step, depth)) = pending.pop() {
        count += 1;
        if count > MAX_PLAN_STEPS || depth >= MAX_PLAN_DEPTH {
            return Err(PlanError::Capacity);
        }
        if let Some(group) = &step.parallel {
            if pending.len() + group.steps.len() > MAX_PLAN_STEPS {
                return Err(PlanError::Capacity);
            }
            pending.extend(group.steps.iter().map(|s| (s, depth + 1)));
        }
    }
    Ok(())
}
fn chain_resource(namespace: &str, tenant: &str, name: &str) -> Result<ResourceRef, PlanError> {
    ResourceRef::new(ResourceKind::Chain, namespace, tenant, name).map_err(|_| PlanError::Invalid)
}
fn plan_request_digest(plan_digest: &str, origin: &Action) -> Result<String, PlanError> {
    let input = governed_provider_input_digest(origin).map_err(|_| PlanError::Invalid)?;
    let bytes = canonical_bytes(&serde_json::json!({
        "format":"acteon-qualified-chain-root:v1", "plan":plan_digest, "input":input,
    }))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn canonical_bytes(value: &serde_json::Value) -> Result<Vec<u8>, PlanError> {
    fn canonical(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let ordered: BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
                serde_json::Value::Object(ordered.into_iter().collect())
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(canonical).collect())
            }
            other => other.clone(),
        }
    }
    serde_json::to_vec(&canonical(value)).map_err(|_| PlanError::Invalid)
}
