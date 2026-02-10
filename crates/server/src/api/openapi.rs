#![allow(clippy::needless_for_each)]

use acteon_audit::{AuditPage, AuditQuery, AuditRecord};
use acteon_core::{
    Action, ActionError, ActionMetadata, ActionOutcome, ProviderResponse, ResponseStatus,
};

use super::approvals::{
    ApprovalActionResponse, ApprovalQueryParams, ApprovalStatusResponse, ListApprovalsResponse,
};
use super::chains::{
    ChainCancelRequest, ChainDetailResponse, ChainStepStatus, ChainSummary, ListChainsResponse,
};
use super::circuit_breakers::{
    CircuitBreakerActionResponse, CircuitBreakerStatus, ListCircuitBreakersResponse,
};
use super::dlq::{DlqDrainResponse, DlqEntry, DlqStatsResponse};
use super::embeddings::{SimilarityRequest, SimilarityResponse};
use super::events::{
    EventStateResponse, ListEventsResponse, TransitionRequest, TransitionResponse,
};
use super::groups::{FlushGroupResponse, GroupDetailResponse, GroupSummary, ListGroupsResponse};
use super::recurring::{
    CreateRecurringRequest, CreateRecurringResponse, ListRecurringResponse,
    RecurringDetailResponse, RecurringLifecycleRequest, RecurringSummary, UpdateRecurringRequest,
};
use super::replay::{ReplayResult, ReplaySummary};
use super::schemas::{
    EmbeddingMetricsResponse, ErrorResponse, HealthResponse, MetricsResponse, ReloadRequest,
    ReloadResponse, RuleSummary, SetEnabledRequest, SetEnabledResponse,
};

#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "Acteon Gateway API",
        version = "0.1.0",
        description = "HTTP API for the Acteon action gateway. Dispatch actions, manage rules, and monitor service health.",
        license(name = "MIT")
    ),
    tags(
        (name = "Health", description = "Service health and metrics"),
        (name = "Dispatch", description = "Action dispatch through the gateway pipeline"),
        (name = "Rules", description = "Rule management and lifecycle"),
        (name = "Audit", description = "Audit trail query and lookup"),
        (name = "DLQ", description = "Dead-letter queue for failed actions"),
        (name = "Events", description = "Event lifecycle state management"),
        (name = "Groups", description = "Event group management for batched notifications"),
        (name = "Approvals", description = "Human-in-the-loop approval workflow"),
        (name = "Chains", description = "Task chain orchestration"),
        (name = "Embeddings", description = "Embedding similarity testing"),
        (name = "Circuit Breakers", description = "Circuit breaker admin operations"),
        (name = "Recurring Actions", description = "Cron-scheduled recurring action management")
    ),
    paths(
        super::health::health,
        super::health::metrics,
        super::dispatch::dispatch,
        super::dispatch::dispatch_batch,
        super::rules::list_rules,
        super::rules::reload_rules,
        super::rules::set_rule_enabled,
        super::audit::query_audit,
        super::audit::get_audit_by_action,
        super::replay::replay_action,
        super::replay::replay_audit,
        super::dlq::dlq_stats,
        super::dlq::dlq_drain,
        super::events::list_events,
        super::events::get_event,
        super::events::transition_event,
        super::groups::list_groups,
        super::groups::get_group,
        super::groups::flush_group,
        super::approvals::approve,
        super::approvals::reject,
        super::approvals::get_approval,
        super::approvals::list_approvals,
        super::chains::list_chains,
        super::chains::get_chain,
        super::chains::cancel_chain,
        super::embeddings::similarity,
        super::circuit_breakers::list_circuit_breakers,
        super::circuit_breakers::trip_circuit_breaker,
        super::circuit_breakers::reset_circuit_breaker,
        super::recurring::create_recurring,
        super::recurring::list_recurring,
        super::recurring::get_recurring,
        super::recurring::update_recurring,
        super::recurring::delete_recurring,
        super::recurring::pause_recurring,
        super::recurring::resume_recurring,
    ),
    components(schemas(
        Action, ActionOutcome, ProviderResponse, ResponseStatus, ActionError,
        ActionMetadata,
        HealthResponse, MetricsResponse, RuleSummary,
        ReloadRequest, ReloadResponse, SetEnabledRequest, SetEnabledResponse,
        ErrorResponse,
        AuditRecord, AuditQuery, AuditPage,
        DlqStatsResponse, DlqEntry, DlqDrainResponse,
        ReplayResult, ReplaySummary,
        EventStateResponse, ListEventsResponse, TransitionRequest, TransitionResponse,
        GroupSummary, ListGroupsResponse, GroupDetailResponse, FlushGroupResponse,
        ApprovalActionResponse, ApprovalStatusResponse, ApprovalQueryParams, ListApprovalsResponse,
        ChainSummary, ListChainsResponse, ChainDetailResponse, ChainStepStatus, ChainCancelRequest,
        SimilarityRequest, SimilarityResponse,
        EmbeddingMetricsResponse,
        CircuitBreakerStatus, ListCircuitBreakersResponse, CircuitBreakerActionResponse,
        CreateRecurringRequest, CreateRecurringResponse, ListRecurringResponse,
        RecurringDetailResponse, RecurringSummary, UpdateRecurringRequest,
        RecurringLifecycleRequest,
    ))
)]
pub struct ApiDoc;
