//! Common operations layer for the Acteon CLI and MCP server.
//!
//! Wraps [`acteon_client::ActeonClient`] with configuration management.
//! Both the CLI and MCP server build on top of this crate.

mod config;
mod error;
pub mod test_rules;

pub use config::OpsConfig;
pub use error::OpsError;

use acteon_client::{
    ActeonClient, ActeonClientBuilder, ApprovalListResponse, AuditPage, AuditQuery, AuditRecord,
    BatchResult, ChainDetailResponse, ComplianceStatus, CreateProfileRequest, CreateQuotaRequest,
    CreateRecurringAction, CreateRecurringResponse, CreateRetentionRequest, CreateTemplateRequest,
    DagResponse, DlqDrainResponse, DlqStatsResponse, EvaluateRulesOptions, EventListResponse,
    EventQuery, EventState, FlushGroupResponse, GroupDetail, GroupListResponse,
    HashChainVerification, ListChainDefinitionsResponse, ListChainsResponse, ListPluginsResponse,
    ListProfilesResponse, ListQuotasResponse, ListRecurringResponse, ListTemplatesResponse,
    QuotaPolicy, QuotaUsage, RecurringDetail, RecurringFilter, ReloadResult, RenderPreviewRequest,
    RenderPreviewResponse, ReplayQuery, ReplayResult, ReplaySummary, RetentionPolicy,
    RuleEvaluationTrace, RuleInfo, TemplateInfo, TemplateProfileInfo, TransitionResponse,
    UpdateProfileRequest, UpdateQuotaRequest, UpdateRecurringAction, UpdateRetentionRequest,
    UpdateTemplateRequest, VerifyHashChainRequest,
};
use acteon_core::{
    Action, ActionOutcome, CircuitBreakerActionResponse, ListCircuitBreakersResponse,
    ListProviderHealthResponse,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Re-export client and core types for consumers.
pub mod acteon_client {
    pub use acteon_client::*;
}
pub mod acteon_core {
    pub use acteon_core::*;
}

/// Options for dispatching an action.
#[derive(Debug, Clone, Default)]
pub struct DispatchOptions {
    pub metadata: HashMap<String, String>,
    pub dry_run: bool,
}

/// High-level operations client for Acteon.
///
/// Wraps the HTTP client with configuration management and provides
/// the shared interface consumed by both the CLI and MCP server.
#[derive(Clone)]
pub struct OpsClient {
    inner: Arc<ActeonClient>,
}

impl OpsClient {
    /// Create a new operations client from configuration.
    pub fn from_config(config: &OpsConfig) -> Result<Self, OpsError> {
        let mut builder = ActeonClientBuilder::new(&config.endpoint);

        if let Some(ref timeout) = config.timeout {
            builder = builder.timeout(*timeout);
        }

        if let Some(ref api_key) = config.api_key {
            builder = builder.api_key(api_key);
        }

        let client = builder
            .build()
            .map_err(|e| OpsError::Configuration(e.to_string()))?;

        Ok(Self {
            inner: Arc::new(client),
        })
    }

    /// Access the underlying HTTP client directly.
    pub fn client(&self) -> &ActeonClient {
        &self.inner
    }

    // =========================================================================
    // High-level Operations
    // =========================================================================

    /// Dispatch an action with optional metadata and dry-run mode.
    pub async fn dispatch(
        &self,
        namespace: String,
        tenant: String,
        provider: String,
        action_type: String,
        payload: serde_json::Value,
        options: DispatchOptions,
    ) -> Result<ActionOutcome, OpsError> {
        let mut action = Action::new(namespace, tenant, provider, &action_type, payload);
        if !options.metadata.is_empty() {
            action.metadata.labels = options.metadata;
        }

        let outcome = if options.dry_run {
            self.inner.dispatch_dry_run(&action).await?
        } else {
            self.inner.dispatch(&action).await?
        };

        Ok(outcome)
    }

    /// Query the audit trail.
    pub async fn query_audit(&self, query: AuditQuery) -> Result<AuditPage, OpsError> {
        Ok(self.inner.query_audit(&query).await?)
    }

    /// List routing rules.
    pub async fn list_rules(&self) -> Result<Vec<RuleInfo>, OpsError> {
        Ok(self.inner.list_rules().await?)
    }

    /// Evaluate rules against a test action.
    pub async fn evaluate_rules(
        &self,
        namespace: String,
        tenant: String,
        provider: String,
        action_type: String,
        payload: serde_json::Value,
        include_disabled: bool,
    ) -> Result<RuleEvaluationTrace, OpsError> {
        let action = Action::new(namespace, tenant, provider, &action_type, payload);
        let options = EvaluateRulesOptions {
            include_disabled,
            ..EvaluateRulesOptions::default()
        };

        Ok(self.inner.evaluate_rules(&action, &options).await?)
    }

    /// Transition a stateful event.
    pub async fn transition_event(
        &self,
        fingerprint: String,
        to_state: String,
        namespace: String,
        tenant: String,
    ) -> Result<TransitionResponse, OpsError> {
        Ok(self
            .inner
            .transition_event(&fingerprint, &to_state, &namespace, &tenant)
            .await?)
    }

    /// List stateful events.
    pub async fn list_events(&self, query: EventQuery) -> Result<EventListResponse, OpsError> {
        Ok(self.inner.list_events(&query).await?)
    }

    /// List action chains.
    pub async fn list_chains(
        &self,
        namespace: String,
        tenant: String,
        status: Option<String>,
    ) -> Result<ListChainsResponse, OpsError> {
        Ok(self
            .inner
            .list_chains(&namespace, &tenant, status.as_deref())
            .await?)
    }

    /// Enable or disable a rule.
    pub async fn set_rule_enabled(&self, rule_name: String, enabled: bool) -> Result<(), OpsError> {
        Ok(self.inner.set_rule_enabled(&rule_name, enabled).await?)
    }

    /// Check health.
    pub async fn health(&self) -> Result<bool, OpsError> {
        Ok(self.inner.health().await?)
    }

    /// List all circuit breakers.
    pub async fn list_circuit_breakers(&self) -> Result<ListCircuitBreakersResponse, OpsError> {
        Ok(self.inner.list_circuit_breakers().await?)
    }

    /// Trip a circuit breaker.
    pub async fn trip_circuit_breaker(
        &self,
        provider: String,
    ) -> Result<CircuitBreakerActionResponse, OpsError> {
        Ok(self.inner.trip_circuit_breaker(&provider).await?)
    }

    /// List retention policies, optionally filtered by namespace and tenant.
    pub async fn list_retention(
        &self,
        namespace: Option<String>,
        tenant: Option<String>,
    ) -> Result<acteon_client::ListRetentionResponse, OpsError> {
        Ok(self
            .inner
            .list_retention(namespace.as_deref(), tenant.as_deref())
            .await?)
    }

    /// Query aggregated action analytics.
    pub async fn query_analytics(
        &self,
        query: acteon_client::AnalyticsQuery,
    ) -> Result<acteon_client::AnalyticsResponse, OpsError> {
        Ok(self.inner.query_analytics(&query).await?)
    }

    /// Reset a circuit breaker.
    pub async fn reset_circuit_breaker(
        &self,
        provider: String,
    ) -> Result<CircuitBreakerActionResponse, OpsError> {
        Ok(self.inner.reset_circuit_breaker(&provider).await?)
    }

    // =========================================================================
    // Dispatch — batch
    // =========================================================================

    /// Dispatch a batch of actions.
    pub async fn dispatch_batch(
        &self,
        actions: &[Action],
        dry_run: bool,
    ) -> Result<Vec<BatchResult>, OpsError> {
        let result = if dry_run {
            self.inner.dispatch_batch_dry_run(actions).await?
        } else {
            self.inner.dispatch_batch(actions).await?
        };
        Ok(result)
    }

    // =========================================================================
    // Rules — reload
    // =========================================================================

    /// Reload rules from the YAML directory.
    pub async fn reload_rules(&self) -> Result<ReloadResult, OpsError> {
        Ok(self.inner.reload_rules().await?)
    }

    // =========================================================================
    // Audit — get record, replay
    // =========================================================================

    /// Get a single audit record by action ID.
    pub async fn get_audit_record(&self, action_id: &str) -> Result<Option<AuditRecord>, OpsError> {
        Ok(self.inner.get_audit_record(action_id).await?)
    }

    /// Replay a single action from the audit trail.
    pub async fn replay_action(&self, action_id: &str) -> Result<ReplayResult, OpsError> {
        Ok(self.inner.replay_action(action_id).await?)
    }

    /// Replay multiple actions matching a query.
    pub async fn replay_audit(&self, query: ReplayQuery) -> Result<ReplaySummary, OpsError> {
        Ok(self.inner.replay_audit(&query).await?)
    }

    // =========================================================================
    // Events — get single
    // =========================================================================

    /// Get a single event by fingerprint.
    pub async fn get_event(
        &self,
        fingerprint: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<Option<EventState>, OpsError> {
        Ok(self.inner.get_event(fingerprint, namespace, tenant).await?)
    }

    // =========================================================================
    // Chains — get, cancel, DAG, definitions CRUD
    // =========================================================================

    /// Get the full details of a chain by ID.
    pub async fn get_chain(
        &self,
        chain_id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<ChainDetailResponse, OpsError> {
        Ok(self.inner.get_chain(chain_id, namespace, tenant).await?)
    }

    /// Cancel a running chain.
    pub async fn cancel_chain(
        &self,
        chain_id: &str,
        namespace: &str,
        tenant: &str,
        reason: Option<&str>,
        cancelled_by: Option<&str>,
    ) -> Result<ChainDetailResponse, OpsError> {
        Ok(self
            .inner
            .cancel_chain(chain_id, namespace, tenant, reason, cancelled_by)
            .await?)
    }

    /// Get the DAG for a chain instance.
    pub async fn get_chain_dag(
        &self,
        chain_id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<DagResponse, OpsError> {
        Ok(self
            .inner
            .get_chain_dag(chain_id, namespace, tenant)
            .await?)
    }

    /// Get the DAG for a chain definition.
    pub async fn get_chain_definition_dag(&self, name: &str) -> Result<DagResponse, OpsError> {
        Ok(self.inner.get_chain_definition_dag(name).await?)
    }

    /// List all chain definitions.
    pub async fn list_chain_definitions(&self) -> Result<ListChainDefinitionsResponse, OpsError> {
        Ok(self.inner.list_chain_definitions().await?)
    }

    /// Get a chain definition by name.
    pub async fn get_chain_definition(&self, name: &str) -> Result<Value, OpsError> {
        Ok(self.inner.get_chain_definition(name).await?)
    }

    /// Create or update a chain definition.
    pub async fn put_chain_definition(
        &self,
        name: &str,
        config: &Value,
    ) -> Result<Value, OpsError> {
        Ok(self.inner.put_chain_definition(name, config).await?)
    }

    /// Delete a chain definition.
    pub async fn delete_chain_definition(&self, name: &str) -> Result<(), OpsError> {
        Ok(self.inner.delete_chain_definition(name).await?)
    }

    // =========================================================================
    // Recurring actions
    // =========================================================================

    /// Create a recurring action.
    pub async fn create_recurring(
        &self,
        req: &CreateRecurringAction,
    ) -> Result<CreateRecurringResponse, OpsError> {
        Ok(self.inner.create_recurring(req).await?)
    }

    /// List recurring actions.
    pub async fn list_recurring(
        &self,
        filter: &RecurringFilter,
    ) -> Result<ListRecurringResponse, OpsError> {
        Ok(self.inner.list_recurring(filter).await?)
    }

    /// Get a recurring action by ID.
    pub async fn get_recurring(
        &self,
        id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<Option<RecurringDetail>, OpsError> {
        Ok(self.inner.get_recurring(id, namespace, tenant).await?)
    }

    /// Update a recurring action.
    pub async fn update_recurring(
        &self,
        id: &str,
        update: &UpdateRecurringAction,
    ) -> Result<RecurringDetail, OpsError> {
        Ok(self.inner.update_recurring(id, update).await?)
    }

    /// Delete a recurring action.
    pub async fn delete_recurring(
        &self,
        id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<(), OpsError> {
        Ok(self.inner.delete_recurring(id, namespace, tenant).await?)
    }

    /// Pause a recurring action.
    pub async fn pause_recurring(
        &self,
        id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<RecurringDetail, OpsError> {
        Ok(self.inner.pause_recurring(id, namespace, tenant).await?)
    }

    /// Resume a recurring action.
    pub async fn resume_recurring(
        &self,
        id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<RecurringDetail, OpsError> {
        Ok(self.inner.resume_recurring(id, namespace, tenant).await?)
    }

    // =========================================================================
    // Quotas
    // =========================================================================

    /// Create a quota policy.
    pub async fn create_quota(&self, req: &CreateQuotaRequest) -> Result<QuotaPolicy, OpsError> {
        Ok(self.inner.create_quota(req).await?)
    }

    /// List quota policies.
    pub async fn list_quotas(
        &self,
        namespace: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<ListQuotasResponse, OpsError> {
        Ok(self.inner.list_quotas(namespace, tenant).await?)
    }

    /// Get a quota policy by ID.
    pub async fn get_quota(&self, id: &str) -> Result<Option<QuotaPolicy>, OpsError> {
        Ok(self.inner.get_quota(id).await?)
    }

    /// Update a quota policy.
    pub async fn update_quota(
        &self,
        id: &str,
        update: &UpdateQuotaRequest,
    ) -> Result<QuotaPolicy, OpsError> {
        Ok(self.inner.update_quota(id, update).await?)
    }

    /// Delete a quota policy.
    pub async fn delete_quota(
        &self,
        id: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<(), OpsError> {
        Ok(self.inner.delete_quota(id, namespace, tenant).await?)
    }

    /// Get quota usage.
    pub async fn get_quota_usage(&self, id: &str) -> Result<QuotaUsage, OpsError> {
        Ok(self.inner.get_quota_usage(id).await?)
    }

    // =========================================================================
    // Retention
    // =========================================================================

    /// Create a retention policy.
    pub async fn create_retention(
        &self,
        req: &CreateRetentionRequest,
    ) -> Result<RetentionPolicy, OpsError> {
        Ok(self.inner.create_retention(req).await?)
    }

    /// Get a retention policy by ID.
    pub async fn get_retention(&self, id: &str) -> Result<Option<RetentionPolicy>, OpsError> {
        Ok(self.inner.get_retention(id).await?)
    }

    /// Update a retention policy.
    pub async fn update_retention(
        &self,
        id: &str,
        update: &UpdateRetentionRequest,
    ) -> Result<RetentionPolicy, OpsError> {
        Ok(self.inner.update_retention(id, update).await?)
    }

    /// Delete a retention policy.
    pub async fn delete_retention(&self, id: &str) -> Result<(), OpsError> {
        Ok(self.inner.delete_retention(id).await?)
    }

    // =========================================================================
    // Templates
    // =========================================================================

    /// Create a template.
    pub async fn create_template(
        &self,
        req: &CreateTemplateRequest,
    ) -> Result<TemplateInfo, OpsError> {
        Ok(self.inner.create_template(req).await?)
    }

    /// List templates.
    pub async fn list_templates(
        &self,
        namespace: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<ListTemplatesResponse, OpsError> {
        Ok(self.inner.list_templates(namespace, tenant).await?)
    }

    /// Get a template by ID.
    pub async fn get_template(&self, id: &str) -> Result<Option<TemplateInfo>, OpsError> {
        Ok(self.inner.get_template(id).await?)
    }

    /// Update a template.
    pub async fn update_template(
        &self,
        id: &str,
        update: &UpdateTemplateRequest,
    ) -> Result<TemplateInfo, OpsError> {
        Ok(self.inner.update_template(id, update).await?)
    }

    /// Delete a template.
    pub async fn delete_template(&self, id: &str) -> Result<(), OpsError> {
        Ok(self.inner.delete_template(id).await?)
    }

    /// Create a template profile.
    pub async fn create_profile(
        &self,
        req: &CreateProfileRequest,
    ) -> Result<TemplateProfileInfo, OpsError> {
        Ok(self.inner.create_profile(req).await?)
    }

    /// List template profiles.
    pub async fn list_profiles(
        &self,
        namespace: Option<&str>,
        tenant: Option<&str>,
    ) -> Result<ListProfilesResponse, OpsError> {
        Ok(self.inner.list_profiles(namespace, tenant).await?)
    }

    /// Get a template profile by ID.
    pub async fn get_profile(&self, id: &str) -> Result<Option<TemplateProfileInfo>, OpsError> {
        Ok(self.inner.get_profile(id).await?)
    }

    /// Update a template profile.
    pub async fn update_profile(
        &self,
        id: &str,
        update: &UpdateProfileRequest,
    ) -> Result<TemplateProfileInfo, OpsError> {
        Ok(self.inner.update_profile(id, update).await?)
    }

    /// Delete a template profile.
    pub async fn delete_profile(&self, id: &str) -> Result<(), OpsError> {
        Ok(self.inner.delete_profile(id).await?)
    }

    /// Render a template preview.
    pub async fn render_preview(
        &self,
        req: &RenderPreviewRequest,
    ) -> Result<RenderPreviewResponse, OpsError> {
        Ok(self.inner.render_preview(req).await?)
    }

    // =========================================================================
    // WASM Plugins
    // =========================================================================

    /// List WASM plugins.
    pub async fn list_plugins(&self) -> Result<ListPluginsResponse, OpsError> {
        Ok(self.inner.list_plugins().await?)
    }

    /// Delete a WASM plugin.
    pub async fn delete_plugin(&self, name: &str) -> Result<(), OpsError> {
        Ok(self.inner.delete_plugin(name).await?)
    }

    // =========================================================================
    // Groups
    // =========================================================================

    /// List event groups.
    pub async fn list_groups(&self) -> Result<GroupListResponse, OpsError> {
        Ok(self.inner.list_groups().await?)
    }

    /// Get an event group by key.
    pub async fn get_group(&self, key: &str) -> Result<Option<GroupDetail>, OpsError> {
        Ok(self.inner.get_group(key).await?)
    }

    /// Flush an event group.
    pub async fn flush_group(&self, key: &str) -> Result<FlushGroupResponse, OpsError> {
        Ok(self.inner.flush_group(key).await?)
    }

    // =========================================================================
    // DLQ
    // =========================================================================

    /// Get DLQ statistics.
    pub async fn dlq_stats(&self) -> Result<DlqStatsResponse, OpsError> {
        Ok(self.inner.dlq_stats().await?)
    }

    /// Drain the DLQ.
    pub async fn dlq_drain(&self) -> Result<DlqDrainResponse, OpsError> {
        Ok(self.inner.dlq_drain().await?)
    }

    // =========================================================================
    // Compliance
    // =========================================================================

    /// Get compliance status.
    pub async fn get_compliance_status(&self) -> Result<ComplianceStatus, OpsError> {
        Ok(self.inner.get_compliance_status().await?)
    }

    /// Verify audit hash chain integrity.
    pub async fn verify_audit_chain(
        &self,
        req: &VerifyHashChainRequest,
    ) -> Result<HashChainVerification, OpsError> {
        Ok(self.inner.verify_audit_chain(req).await?)
    }

    // =========================================================================
    // Providers
    // =========================================================================

    /// List provider health status.
    pub async fn list_provider_health(&self) -> Result<ListProviderHealthResponse, OpsError> {
        Ok(self.inner.list_provider_health().await?)
    }

    // =========================================================================
    // Approvals
    // =========================================================================

    /// List pending approvals.
    pub async fn list_approvals(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<ApprovalListResponse, OpsError> {
        Ok(self.inner.list_approvals(namespace, tenant).await?)
    }
}
