"""Data models for the Acteon client."""

from dataclasses import dataclass, field
from typing import Any, Dict, Iterator, List, Optional
from datetime import datetime
import json
import uuid


@dataclass
class Action:
    """An action to be dispatched through Acteon.

    Attributes:
        namespace: Logical grouping for the action.
        tenant: Tenant identifier for multi-tenancy.
        provider: Target provider name (e.g., "email", "sms").
        action_type: Type of action (e.g., "send_notification").
        payload: Action-specific data.
        id: Unique action identifier (auto-generated if not provided).
        dedup_key: Optional deduplication key.
        metadata: Optional key-value metadata.
        created_at: Timestamp when the action was created.
    """
    namespace: str
    tenant: str
    provider: str
    action_type: str
    payload: dict[str, Any]
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    dedup_key: Optional[str] = None
    metadata: Optional[dict[str, str]] = None
    created_at: datetime = field(default_factory=datetime.utcnow)

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        result = {
            "id": self.id,
            "namespace": self.namespace,
            "tenant": self.tenant,
            "provider": self.provider,
            "action_type": self.action_type,
            "payload": self.payload,
            "created_at": self.created_at.isoformat() + "Z",
        }
        if self.dedup_key:
            result["dedup_key"] = self.dedup_key
        if self.metadata:
            result["metadata"] = {"labels": self.metadata}
        return result


@dataclass
class ProviderResponse:
    """Response from a provider after executing an action."""
    status: str
    body: dict[str, Any]
    headers: dict[str, str] = field(default_factory=dict)


@dataclass
class ActionOutcome:
    """Outcome of dispatching an action.

    Attributes:
        outcome_type: One of "executed", "deduplicated", "suppressed",
                      "rerouted", "throttled", "failed", "dry_run",
                      "scheduled".
        response: Provider response (for executed/rerouted).
        rule: Rule name (for suppressed).
        original_provider: Original provider (for rerouted).
        new_provider: New provider (for rerouted).
        retry_after_secs: Seconds to wait (for throttled).
        error: Error details (for failed).
        verdict_details: Dry-run details including verdict, matched_rule,
                         and would_be_provider (for dry_run).
        action_id: Scheduled action identifier (for scheduled).
        scheduled_for: RFC 3339 timestamp for scheduled execution (for scheduled).
    """
    outcome_type: str
    response: Optional[ProviderResponse] = None
    rule: Optional[str] = None
    original_provider: Optional[str] = None
    new_provider: Optional[str] = None
    retry_after_secs: Optional[float] = None
    error: Optional[dict[str, Any]] = None
    verdict_details: Optional[dict[str, Any]] = None
    action_id: Optional[str] = None
    scheduled_for: Optional[str] = None
    tenant: Optional[str] = None
    limit: Optional[int] = None
    used: Optional[int] = None
    overage_behavior: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ActionOutcome":
        """Parse from API response."""
        if "Executed" in data:
            resp_data = data["Executed"]
            return cls(
                outcome_type="executed",
                response=ProviderResponse(
                    status=resp_data.get("status", "success"),
                    body=resp_data.get("body", {}),
                    headers=resp_data.get("headers", {}),
                ),
            )
        elif data == "Deduplicated" or "Deduplicated" in data:
            return cls(outcome_type="deduplicated")
        elif "Suppressed" in data:
            return cls(outcome_type="suppressed", rule=data["Suppressed"].get("rule"))
        elif "Rerouted" in data:
            rerouted = data["Rerouted"]
            resp_data = rerouted.get("response", {})
            return cls(
                outcome_type="rerouted",
                original_provider=rerouted.get("original_provider"),
                new_provider=rerouted.get("new_provider"),
                response=ProviderResponse(
                    status=resp_data.get("status", "success"),
                    body=resp_data.get("body", {}),
                    headers=resp_data.get("headers", {}),
                ),
            )
        elif "Throttled" in data:
            retry_after = data["Throttled"].get("retry_after", {})
            secs = retry_after.get("secs", 0) + retry_after.get("nanos", 0) / 1e9
            return cls(outcome_type="throttled", retry_after_secs=secs)
        elif "Failed" in data:
            return cls(outcome_type="failed", error=data["Failed"])
        elif "DryRun" in data:
            dry_run = data["DryRun"]
            return cls(
                outcome_type="dry_run",
                verdict_details={
                    "verdict": dry_run.get("verdict"),
                    "matched_rule": dry_run.get("matched_rule"),
                    "would_be_provider": dry_run.get("would_be_provider"),
                },
            )
        elif "Scheduled" in data:
            scheduled = data["Scheduled"]
            return cls(
                outcome_type="scheduled",
                action_id=scheduled.get("action_id"),
                scheduled_for=scheduled.get("scheduled_for"),
            )
        elif "QuotaExceeded" in data:
            quota = data["QuotaExceeded"]
            return cls(
                outcome_type="quota_exceeded",
                tenant=quota.get("tenant"),
                limit=quota.get("limit"),
                used=quota.get("used"),
                overage_behavior=quota.get("overage_behavior"),
            )
        else:
            return cls(outcome_type="unknown")

    def is_executed(self) -> bool:
        return self.outcome_type == "executed"

    def is_deduplicated(self) -> bool:
        return self.outcome_type == "deduplicated"

    def is_suppressed(self) -> bool:
        return self.outcome_type == "suppressed"

    def is_rerouted(self) -> bool:
        return self.outcome_type == "rerouted"

    def is_throttled(self) -> bool:
        return self.outcome_type == "throttled"

    def is_failed(self) -> bool:
        return self.outcome_type == "failed"

    def is_dry_run(self) -> bool:
        return self.outcome_type == "dry_run"

    def is_scheduled(self) -> bool:
        return self.outcome_type == "scheduled"

    def is_quota_exceeded(self) -> bool:
        return self.outcome_type == "quota_exceeded"


@dataclass
class ErrorResponse:
    """Error response from the API."""
    code: str
    message: str
    retryable: bool = False


@dataclass
class BatchResult:
    """Result from a batch dispatch operation."""
    success: bool
    outcome: Optional[ActionOutcome] = None
    error: Optional[ErrorResponse] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "BatchResult":
        """Parse from API response."""
        if "error" in data:
            err = data["error"]
            return cls(
                success=False,
                error=ErrorResponse(
                    code=err.get("code", "UNKNOWN"),
                    message=err.get("message", "Unknown error"),
                    retryable=err.get("retryable", False),
                ),
            )
        else:
            return cls(success=True, outcome=ActionOutcome.from_dict(data))


@dataclass
class RuleInfo:
    """Information about a loaded rule."""
    name: str
    priority: int
    enabled: bool
    description: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "RuleInfo":
        return cls(
            name=data["name"],
            priority=data["priority"],
            enabled=data["enabled"],
            description=data.get("description"),
        )


@dataclass
class ReloadResult:
    """Result of reloading rules."""
    loaded: int
    errors: list[str]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ReloadResult":
        return cls(loaded=data["loaded"], errors=data.get("errors", []))


@dataclass
class EvaluateRulesRequest:
    """Request to evaluate rules against a test action without dispatching.

    Attributes:
        namespace: Logical grouping for the action.
        tenant: Tenant identifier for multi-tenancy.
        provider: Target provider name.
        action_type: Type of action.
        payload: Action-specific data.
        metadata: Optional key-value metadata.
        include_disabled: Whether to include disabled rules in evaluation.
        evaluate_all: Whether to evaluate all rules instead of stopping at first match.
        evaluate_at: Optional ISO 8601 timestamp to simulate evaluation at a specific time.
        mock_state: Optional mock state entries for evaluation.
    """
    namespace: str
    tenant: str
    provider: str
    action_type: str
    payload: Dict[str, Any]
    metadata: Optional[Dict[str, str]] = None
    include_disabled: bool = False
    evaluate_all: bool = False
    evaluate_at: Optional[str] = None
    mock_state: Optional[Dict[str, str]] = None


@dataclass
class SemanticMatchDetail:
    """Details about a semantic match evaluation.

    Attributes:
        extracted_text: The text that was extracted and compared.
        topic: The topic the text was compared against.
        similarity: The computed similarity score.
        threshold: The threshold that was configured on the rule.
    """
    extracted_text: str
    topic: str
    similarity: float
    threshold: float

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "SemanticMatchDetail":
        return cls(
            extracted_text=data["extracted_text"],
            topic=data["topic"],
            similarity=data["similarity"],
            threshold=data["threshold"],
        )


@dataclass
class TraceContext:
    """Contextual information captured during rule evaluation.

    Attributes:
        time: The time map that was used during evaluation.
        environment_keys: Environment keys accessed during evaluation (values
            omitted for security).
        accessed_state_keys: State keys actually accessed during evaluation.
        effective_timezone: The effective timezone used for time-based conditions.
    """
    time: Dict[str, Any]
    environment_keys: List[str]
    accessed_state_keys: List[str] = field(default_factory=list)
    effective_timezone: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "TraceContext":
        return cls(
            time=data.get("time", {}),
            environment_keys=data.get("environment_keys", []),
            accessed_state_keys=data.get("accessed_state_keys", []),
            effective_timezone=data.get("effective_timezone"),
        )


@dataclass
class RuleTraceEntry:
    """Trace entry for a single rule evaluation.

    Attributes:
        rule_name: Name of the rule.
        priority: Rule priority.
        enabled: Whether the rule is enabled.
        condition_display: Human-readable display of the rule condition.
        result: Evaluation result (matched, not_matched, skipped, error).
        evaluation_duration_us: Time spent evaluating this rule in microseconds.
        action: The rule action (e.g., Deny, Allow, Reroute).
        source: The rule source (e.g., Yaml, Cel).
        description: Optional rule description.
        skip_reason: Reason the rule was skipped (if skipped).
        error: Error message (if evaluation errored).
        semantic_details: Details about semantic match evaluation, if the rule
            uses a semantic match condition.
        modify_patch: JSON merge patch for Modify rules in evaluate_all mode.
        modified_payload_preview: Cumulative payload after applying this rule's
            patch (only for Modify rules in evaluate_all mode).
    """
    rule_name: str
    priority: int
    enabled: bool
    condition_display: str
    result: str
    evaluation_duration_us: int
    action: str
    source: str
    description: Optional[str] = None
    skip_reason: Optional[str] = None
    error: Optional[str] = None
    semantic_details: Optional[SemanticMatchDetail] = None
    modify_patch: Optional[Dict[str, Any]] = None
    modified_payload_preview: Optional[Dict[str, Any]] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "RuleTraceEntry":
        semantic_raw = data.get("semantic_details")
        return cls(
            rule_name=data["rule_name"],
            priority=data["priority"],
            enabled=data["enabled"],
            condition_display=data["condition_display"],
            result=data["result"],
            evaluation_duration_us=data["evaluation_duration_us"],
            action=data["action"],
            source=data["source"],
            description=data.get("description"),
            skip_reason=data.get("skip_reason"),
            error=data.get("error"),
            semantic_details=(
                SemanticMatchDetail.from_dict(semantic_raw)
                if semantic_raw is not None
                else None
            ),
            modify_patch=data.get("modify_patch"),
            modified_payload_preview=data.get("modified_payload_preview"),
        )


@dataclass
class EvaluateRulesResponse:
    """Response from the rule evaluation playground.

    Attributes:
        verdict: The overall verdict (e.g., allow, deny).
        total_rules_evaluated: Number of rules that were evaluated.
        total_rules_skipped: Number of rules that were skipped.
        evaluation_duration_us: Total evaluation time in microseconds.
        trace: Per-rule trace entries showing evaluation details.
        context: Evaluation context including time, environment, and state info.
        matched_rule: Name of the matched rule (if any).
        has_errors: Whether any rule evaluation produced an error.
        modified_payload: The payload after rule modifications (if any).
    """
    verdict: str
    total_rules_evaluated: int
    total_rules_skipped: int
    evaluation_duration_us: int
    trace: List[RuleTraceEntry]
    context: TraceContext
    matched_rule: Optional[str] = None
    has_errors: bool = False
    modified_payload: Optional[Dict[str, Any]] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "EvaluateRulesResponse":
        return cls(
            verdict=data["verdict"],
            matched_rule=data.get("matched_rule"),
            has_errors=data.get("has_errors", False),
            total_rules_evaluated=data["total_rules_evaluated"],
            total_rules_skipped=data["total_rules_skipped"],
            evaluation_duration_us=data["evaluation_duration_us"],
            trace=[RuleTraceEntry.from_dict(t) for t in data["trace"]],
            context=TraceContext.from_dict(data["context"]),
            modified_payload=data.get("modified_payload"),
        )


@dataclass
class AuditQuery:
    """Query parameters for audit search."""
    namespace: Optional[str] = None
    tenant: Optional[str] = None
    provider: Optional[str] = None
    action_type: Optional[str] = None
    outcome: Optional[str] = None
    limit: Optional[int] = None
    offset: Optional[int] = None

    def to_params(self) -> dict[str, Any]:
        """Convert to query parameters."""
        params = {}
        if self.namespace:
            params["namespace"] = self.namespace
        if self.tenant:
            params["tenant"] = self.tenant
        if self.provider:
            params["provider"] = self.provider
        if self.action_type:
            params["action_type"] = self.action_type
        if self.outcome:
            params["outcome"] = self.outcome
        if self.limit is not None:
            params["limit"] = self.limit
        if self.offset is not None:
            params["offset"] = self.offset
        return params


@dataclass
class AuditRecord:
    """An audit record."""
    id: str
    action_id: str
    namespace: str
    tenant: str
    provider: str
    action_type: str
    verdict: str
    outcome: str
    matched_rule: Optional[str]
    duration_ms: int
    dispatched_at: str
    record_hash: Optional[str] = None
    previous_hash: Optional[str] = None
    sequence_number: Optional[int] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "AuditRecord":
        return cls(
            id=data["id"],
            action_id=data["action_id"],
            namespace=data["namespace"],
            tenant=data["tenant"],
            provider=data["provider"],
            action_type=data["action_type"],
            verdict=data["verdict"],
            outcome=data["outcome"],
            matched_rule=data.get("matched_rule"),
            duration_ms=data["duration_ms"],
            dispatched_at=data["dispatched_at"],
            record_hash=data.get("record_hash"),
            previous_hash=data.get("previous_hash"),
            sequence_number=data.get("sequence_number"),
        )


@dataclass
class AuditPage:
    """Paginated audit results."""
    records: list[AuditRecord]
    total: int
    limit: int
    offset: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "AuditPage":
        return cls(
            records=[AuditRecord.from_dict(r) for r in data["records"]],
            total=data["total"],
            limit=data["limit"],
            offset=data["offset"],
        )


# =============================================================================
# Event Types (State Machine Lifecycle)
# =============================================================================


@dataclass
class EventQuery:
    """Query parameters for listing events."""
    namespace: str
    tenant: str
    status: Optional[str] = None
    limit: Optional[int] = None

    def to_params(self) -> dict[str, Any]:
        """Convert to query parameters."""
        params: dict[str, Any] = {
            "namespace": self.namespace,
            "tenant": self.tenant,
        }
        if self.status:
            params["status"] = self.status
        if self.limit is not None:
            params["limit"] = self.limit
        return params


@dataclass
class EventState:
    """Current state of an event."""
    fingerprint: str
    state: str
    action_type: Optional[str] = None
    updated_at: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "EventState":
        return cls(
            fingerprint=data["fingerprint"],
            state=data["state"],
            action_type=data.get("action_type"),
            updated_at=data.get("updated_at"),
        )


@dataclass
class EventListResponse:
    """Response from listing events."""
    events: list[EventState]
    count: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "EventListResponse":
        return cls(
            events=[EventState.from_dict(e) for e in data["events"]],
            count=data["count"],
        )


@dataclass
class TransitionResponse:
    """Response from transitioning an event."""
    fingerprint: str
    previous_state: str
    new_state: str
    notify: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "TransitionResponse":
        return cls(
            fingerprint=data["fingerprint"],
            previous_state=data["previous_state"],
            new_state=data["new_state"],
            notify=data["notify"],
        )


# =============================================================================
# Group Types (Event Batching)
# =============================================================================


@dataclass
class GroupSummary:
    """Summary of an event group."""
    group_id: str
    group_key: str
    event_count: int
    state: str
    notify_at: Optional[str] = None
    created_at: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "GroupSummary":
        return cls(
            group_id=data["group_id"],
            group_key=data["group_key"],
            event_count=data["event_count"],
            state=data["state"],
            notify_at=data.get("notify_at"),
            created_at=data.get("created_at"),
        )


@dataclass
class GroupListResponse:
    """Response from listing groups."""
    groups: list[GroupSummary]
    total: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "GroupListResponse":
        return cls(
            groups=[GroupSummary.from_dict(g) for g in data["groups"]],
            total=data["total"],
        )


@dataclass
class GroupDetail:
    """Detailed information about a group."""
    group: GroupSummary
    events: list[str]
    labels: dict[str, str]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "GroupDetail":
        return cls(
            group=GroupSummary.from_dict(data["group"]),
            events=data.get("events", []),
            labels=data.get("labels", {}),
        )


@dataclass
class FlushGroupResponse:
    """Response from flushing a group."""
    group_id: str
    event_count: int
    notified: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FlushGroupResponse":
        return cls(
            group_id=data["group_id"],
            event_count=data["event_count"],
            notified=data["notified"],
        )


# =============================================================================
# Approval Types (Human-in-the-Loop)
# =============================================================================


@dataclass
class ApprovalActionResponse:
    """Response from approving or rejecting an action."""
    id: str
    status: str
    outcome: Optional[dict[str, Any]] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ApprovalActionResponse":
        return cls(
            id=data["id"],
            status=data["status"],
            outcome=data.get("outcome"),
        )


@dataclass
class ApprovalStatus:
    """Public-facing approval status (no payload exposed)."""
    token: str
    status: str
    rule: str
    created_at: str
    expires_at: str
    decided_at: Optional[str] = None
    message: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ApprovalStatus":
        return cls(
            token=data["token"],
            status=data["status"],
            rule=data["rule"],
            created_at=data["created_at"],
            expires_at=data["expires_at"],
            decided_at=data.get("decided_at"),
            message=data.get("message"),
        )


@dataclass
class ApprovalListResponse:
    """Response from listing pending approvals."""
    approvals: list[ApprovalStatus]
    count: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ApprovalListResponse":
        return cls(
            approvals=[ApprovalStatus.from_dict(a) for a in data["approvals"]],
            count=data["count"],
        )


# =============================================================================
# Webhook Helpers
# =============================================================================


@dataclass
class WebhookPayload:
    """Payload for webhook actions.

    Use this to build the payload for an Action targeted at the webhook provider.

    Attributes:
        url: Target URL for the webhook request.
        method: HTTP method (default: "POST").
        headers: Additional HTTP headers to include.
        body: The JSON body to send to the webhook endpoint.
    """
    url: str
    body: dict[str, Any]
    method: str = "POST"
    headers: Optional[dict[str, str]] = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to payload dictionary for an Action."""
        result: dict[str, Any] = {
            "url": self.url,
            "method": self.method,
            "body": self.body,
        }
        if self.headers:
            result["headers"] = self.headers
        return result


def create_webhook_action(
    namespace: str,
    tenant: str,
    url: str,
    body: dict[str, Any],
    *,
    action_type: str = "webhook",
    method: str = "POST",
    headers: Optional[dict[str, str]] = None,
    dedup_key: Optional[str] = None,
    metadata: Optional[dict[str, str]] = None,
) -> Action:
    """Create an Action targeting the webhook provider.

    This is a convenience function that constructs a properly formatted Action
    for the webhook provider, wrapping the URL, method, headers, and body into
    the payload.

    Args:
        namespace: Logical grouping for the action.
        tenant: Tenant identifier for multi-tenancy.
        url: Target URL for the webhook request.
        body: The JSON body to send to the webhook endpoint.
        action_type: Action type (default: "webhook").
        method: HTTP method (default: "POST").
        headers: Additional HTTP headers to include.
        dedup_key: Optional deduplication key.
        metadata: Optional key-value metadata.

    Returns:
        An Action configured for the webhook provider.

    Example::

        action = create_webhook_action(
            namespace="notifications",
            tenant="tenant-1",
            url="https://hooks.example.com/alert",
            body={"message": "Server is down", "severity": "critical"},
            headers={"X-Custom-Header": "value"},
        )
    """
    webhook = WebhookPayload(url=url, body=body, method=method, headers=headers)
    return Action(
        namespace=namespace,
        tenant=tenant,
        provider="webhook",
        action_type=action_type,
        payload=webhook.to_dict(),
        dedup_key=dedup_key,
        metadata=metadata,
    )


@dataclass
class ReplayResult:
    """Result of replaying a single action."""
    original_action_id: str
    new_action_id: str
    success: bool
    error: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict) -> "ReplayResult":
        return cls(
            original_action_id=data["original_action_id"],
            new_action_id=data["new_action_id"],
            success=data["success"],
            error=data.get("error"),
        )


@dataclass
class ReplaySummary:
    """Summary of a bulk replay operation."""
    replayed: int
    failed: int
    skipped: int
    results: list[ReplayResult]

    @classmethod
    def from_dict(cls, data: dict) -> "ReplaySummary":
        return cls(
            replayed=data["replayed"],
            failed=data["failed"],
            skipped=data["skipped"],
            results=[ReplayResult.from_dict(r) for r in data["results"]],
        )


@dataclass
class ReplayQuery:
    """Query parameters for bulk audit replay."""
    namespace: Optional[str] = None
    tenant: Optional[str] = None
    provider: Optional[str] = None
    action_type: Optional[str] = None
    outcome: Optional[str] = None
    verdict: Optional[str] = None
    matched_rule: Optional[str] = None
    from_time: Optional[str] = None
    to_time: Optional[str] = None
    limit: Optional[int] = None

    def to_params(self) -> dict:
        params = {}
        if self.namespace is not None:
            params["namespace"] = self.namespace
        if self.tenant is not None:
            params["tenant"] = self.tenant
        if self.provider is not None:
            params["provider"] = self.provider
        if self.action_type is not None:
            params["action_type"] = self.action_type
        if self.outcome is not None:
            params["outcome"] = self.outcome
        if self.verdict is not None:
            params["verdict"] = self.verdict
        if self.matched_rule is not None:
            params["matched_rule"] = self.matched_rule
        if self.from_time is not None:
            params["from"] = self.from_time
        if self.to_time is not None:
            params["to"] = self.to_time
        if self.limit is not None:
            params["limit"] = self.limit
        return params


# =============================================================================
# Recurring Action Types
# =============================================================================


@dataclass
class CreateRecurringAction:
    """Request to create a recurring action."""
    namespace: str
    tenant: str
    provider: str
    action_type: str
    payload: dict[str, Any]
    cron_expression: str
    name: Optional[str] = None
    metadata: Optional[dict[str, str]] = None
    timezone: Optional[str] = None
    end_date: Optional[str] = None
    max_executions: Optional[int] = None
    description: Optional[str] = None
    dedup_key: Optional[str] = None
    labels: Optional[dict[str, str]] = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        result: dict[str, Any] = {
            "namespace": self.namespace,
            "tenant": self.tenant,
            "provider": self.provider,
            "action_type": self.action_type,
            "payload": self.payload,
            "cron_expression": self.cron_expression,
        }
        if self.name is not None:
            result["name"] = self.name
        if self.metadata is not None:
            result["metadata"] = self.metadata
        if self.timezone is not None:
            result["timezone"] = self.timezone
        if self.end_date is not None:
            result["end_date"] = self.end_date
        if self.max_executions is not None:
            result["max_executions"] = self.max_executions
        if self.description is not None:
            result["description"] = self.description
        if self.dedup_key is not None:
            result["dedup_key"] = self.dedup_key
        if self.labels is not None:
            result["labels"] = self.labels
        return result


@dataclass
class CreateRecurringResponse:
    """Response from creating a recurring action."""
    id: str
    status: str
    name: Optional[str] = None
    next_execution_at: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "CreateRecurringResponse":
        return cls(
            id=data["id"],
            status=data["status"],
            name=data.get("name"),
            next_execution_at=data.get("next_execution_at"),
        )


@dataclass
class RecurringFilter:
    """Query parameters for listing recurring actions."""
    namespace: Optional[str] = None
    tenant: Optional[str] = None
    status: Optional[str] = None
    limit: Optional[int] = None
    offset: Optional[int] = None

    def to_params(self) -> dict[str, Any]:
        params: dict[str, Any] = {}
        if self.namespace is not None:
            params["namespace"] = self.namespace
        if self.tenant is not None:
            params["tenant"] = self.tenant
        if self.status is not None:
            params["status"] = self.status
        if self.limit is not None:
            params["limit"] = self.limit
        if self.offset is not None:
            params["offset"] = self.offset
        return params


@dataclass
class RecurringSummary:
    """Summary of a recurring action in list responses."""
    id: str
    namespace: str
    tenant: str
    cron_expr: str
    timezone: str
    enabled: bool
    provider: str
    action_type: str
    execution_count: int
    created_at: str
    next_execution_at: Optional[str] = None
    description: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "RecurringSummary":
        return cls(
            id=data["id"],
            namespace=data["namespace"],
            tenant=data["tenant"],
            cron_expr=data["cron_expr"],
            timezone=data["timezone"],
            enabled=data["enabled"],
            provider=data["provider"],
            action_type=data["action_type"],
            execution_count=data["execution_count"],
            created_at=data["created_at"],
            next_execution_at=data.get("next_execution_at"),
            description=data.get("description"),
        )


@dataclass
class ListRecurringResponse:
    """Response from listing recurring actions."""
    recurring_actions: list[RecurringSummary]
    count: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ListRecurringResponse":
        return cls(
            recurring_actions=[
                RecurringSummary.from_dict(r) for r in data["recurring_actions"]
            ],
            count=data["count"],
        )


@dataclass
class RecurringDetail:
    """Detailed information about a recurring action."""
    id: str
    namespace: str
    tenant: str
    cron_expr: str
    timezone: str
    enabled: bool
    provider: str
    action_type: str
    payload: dict[str, Any]
    metadata: dict[str, str]
    execution_count: int
    created_at: str
    updated_at: str
    labels: dict[str, str]
    next_execution_at: Optional[str] = None
    last_executed_at: Optional[str] = None
    ends_at: Optional[str] = None
    description: Optional[str] = None
    dedup_key: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "RecurringDetail":
        return cls(
            id=data["id"],
            namespace=data["namespace"],
            tenant=data["tenant"],
            cron_expr=data["cron_expr"],
            timezone=data["timezone"],
            enabled=data["enabled"],
            provider=data["provider"],
            action_type=data["action_type"],
            payload=data.get("payload", {}),
            metadata=data.get("metadata", {}),
            execution_count=data["execution_count"],
            created_at=data["created_at"],
            updated_at=data["updated_at"],
            labels=data.get("labels", {}),
            next_execution_at=data.get("next_execution_at"),
            last_executed_at=data.get("last_executed_at"),
            ends_at=data.get("ends_at"),
            description=data.get("description"),
            dedup_key=data.get("dedup_key"),
        )


@dataclass
class UpdateRecurringAction:
    """Request to update a recurring action."""
    namespace: str
    tenant: str
    name: Optional[str] = None
    payload: Optional[dict[str, Any]] = None
    metadata: Optional[dict[str, str]] = None
    cron_expression: Optional[str] = None
    timezone: Optional[str] = None
    end_date: Optional[str] = None
    max_executions: Optional[int] = None
    description: Optional[str] = None
    dedup_key: Optional[str] = None
    labels: Optional[dict[str, str]] = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        result: dict[str, Any] = {
            "namespace": self.namespace,
            "tenant": self.tenant,
        }
        if self.name is not None:
            result["name"] = self.name
        if self.payload is not None:
            result["payload"] = self.payload
        if self.metadata is not None:
            result["metadata"] = self.metadata
        if self.cron_expression is not None:
            result["cron_expression"] = self.cron_expression
        if self.timezone is not None:
            result["timezone"] = self.timezone
        if self.end_date is not None:
            result["end_date"] = self.end_date
        if self.max_executions is not None:
            result["max_executions"] = self.max_executions
        if self.description is not None:
            result["description"] = self.description
        if self.dedup_key is not None:
            result["dedup_key"] = self.dedup_key
        if self.labels is not None:
            result["labels"] = self.labels
        return result


# =============================================================================
# Quota Types
# =============================================================================


@dataclass
class CreateQuotaRequest:
    """Request to create a quota policy."""
    namespace: str
    tenant: str
    max_actions: int
    window: str
    overage_behavior: str
    description: Optional[str] = None
    labels: Optional[dict[str, str]] = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        result: dict[str, Any] = {
            "namespace": self.namespace,
            "tenant": self.tenant,
            "max_actions": self.max_actions,
            "window": self.window,
            "overage_behavior": self.overage_behavior,
        }
        if self.description is not None:
            result["description"] = self.description
        if self.labels is not None:
            result["labels"] = self.labels
        return result


@dataclass
class UpdateQuotaRequest:
    """Request to update a quota policy."""
    namespace: str
    tenant: str
    max_actions: Optional[int] = None
    window: Optional[str] = None
    overage_behavior: Optional[str] = None
    description: Optional[str] = None
    enabled: Optional[bool] = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        result: dict[str, Any] = {
            "namespace": self.namespace,
            "tenant": self.tenant,
        }
        if self.max_actions is not None:
            result["max_actions"] = self.max_actions
        if self.window is not None:
            result["window"] = self.window
        if self.overage_behavior is not None:
            result["overage_behavior"] = self.overage_behavior
        if self.description is not None:
            result["description"] = self.description
        if self.enabled is not None:
            result["enabled"] = self.enabled
        return result


@dataclass
class QuotaPolicy:
    """A quota policy."""
    id: str
    namespace: str
    tenant: str
    max_actions: int
    window: str
    overage_behavior: str
    enabled: bool
    created_at: str
    updated_at: str
    description: Optional[str] = None
    labels: Optional[dict[str, str]] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "QuotaPolicy":
        return cls(
            id=data["id"],
            namespace=data["namespace"],
            tenant=data["tenant"],
            max_actions=data["max_actions"],
            window=data["window"],
            overage_behavior=data["overage_behavior"],
            enabled=data["enabled"],
            created_at=data["created_at"],
            updated_at=data["updated_at"],
            description=data.get("description"),
            labels=data.get("labels"),
        )


@dataclass
class ListQuotasResponse:
    """Response from listing quota policies."""
    quotas: list[QuotaPolicy]
    count: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ListQuotasResponse":
        return cls(
            quotas=[QuotaPolicy.from_dict(q) for q in data["quotas"]],
            count=data["count"],
        )


@dataclass
class QuotaUsage:
    """Current usage statistics for a quota."""
    tenant: str
    namespace: str
    used: int
    limit: int
    remaining: int
    window: str
    resets_at: str
    overage_behavior: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "QuotaUsage":
        return cls(
            tenant=data["tenant"],
            namespace=data["namespace"],
            used=data["used"],
            limit=data["limit"],
            remaining=data["remaining"],
            window=data["window"],
            resets_at=data["resets_at"],
            overage_behavior=data["overage_behavior"],
        )


# =============================================================================
# Retention Policy Types
# =============================================================================


@dataclass
class CreateRetentionRequest:
    """Request to create a retention policy."""
    namespace: str
    tenant: str
    audit_ttl_seconds: int
    state_ttl_seconds: int
    event_ttl_seconds: int
    compliance_hold: bool = False
    description: Optional[str] = None
    labels: Optional[dict[str, str]] = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        result: dict[str, Any] = {
            "namespace": self.namespace,
            "tenant": self.tenant,
            "audit_ttl_seconds": self.audit_ttl_seconds,
            "state_ttl_seconds": self.state_ttl_seconds,
            "event_ttl_seconds": self.event_ttl_seconds,
            "compliance_hold": self.compliance_hold,
        }
        if self.description is not None:
            result["description"] = self.description
        if self.labels is not None:
            result["labels"] = self.labels
        return result


@dataclass
class UpdateRetentionRequest:
    """Request to update a retention policy."""
    enabled: Optional[bool] = None
    audit_ttl_seconds: Optional[int] = None
    state_ttl_seconds: Optional[int] = None
    event_ttl_seconds: Optional[int] = None
    compliance_hold: Optional[bool] = None
    description: Optional[str] = None
    labels: Optional[dict[str, str]] = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        result: dict[str, Any] = {}
        if self.enabled is not None:
            result["enabled"] = self.enabled
        if self.audit_ttl_seconds is not None:
            result["audit_ttl_seconds"] = self.audit_ttl_seconds
        if self.state_ttl_seconds is not None:
            result["state_ttl_seconds"] = self.state_ttl_seconds
        if self.event_ttl_seconds is not None:
            result["event_ttl_seconds"] = self.event_ttl_seconds
        if self.compliance_hold is not None:
            result["compliance_hold"] = self.compliance_hold
        if self.description is not None:
            result["description"] = self.description
        if self.labels is not None:
            result["labels"] = self.labels
        return result


@dataclass
class RetentionPolicy:
    """A retention policy."""
    id: str
    namespace: str
    tenant: str
    enabled: bool
    audit_ttl_seconds: int
    state_ttl_seconds: int
    event_ttl_seconds: int
    compliance_hold: bool
    created_at: str
    updated_at: str
    description: Optional[str] = None
    labels: Optional[dict[str, str]] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "RetentionPolicy":
        return cls(
            id=data["id"],
            namespace=data["namespace"],
            tenant=data["tenant"],
            enabled=data["enabled"],
            audit_ttl_seconds=data["audit_ttl_seconds"],
            state_ttl_seconds=data["state_ttl_seconds"],
            event_ttl_seconds=data["event_ttl_seconds"],
            compliance_hold=data["compliance_hold"],
            created_at=data["created_at"],
            updated_at=data["updated_at"],
            description=data.get("description"),
            labels=data.get("labels"),
        )


@dataclass
class ListRetentionResponse:
    """Response from listing retention policies."""
    policies: list[RetentionPolicy]
    count: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ListRetentionResponse":
        return cls(
            policies=[RetentionPolicy.from_dict(p) for p in data["policies"]],
            count=data["count"],
        )


# =============================================================================
# Chain Types
# =============================================================================


@dataclass
class ChainSummary:
    """Summary of a chain execution.

    Attributes:
        chain_id: Unique chain execution ID.
        chain_name: Name of the chain configuration.
        status: Current status (running, completed, failed, cancelled, timed_out).
        current_step: Current step index (0-based).
        total_steps: Total number of steps.
        started_at: When the chain started.
        updated_at: When the chain was last updated.
        parent_chain_id: Parent chain ID if this is a sub-chain.
    """
    chain_id: str
    chain_name: str
    status: str
    current_step: int
    total_steps: int
    started_at: str
    updated_at: str
    parent_chain_id: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ChainSummary":
        return cls(
            chain_id=data["chain_id"],
            chain_name=data["chain_name"],
            status=data["status"],
            current_step=data["current_step"],
            total_steps=data["total_steps"],
            started_at=data["started_at"],
            updated_at=data["updated_at"],
            parent_chain_id=data.get("parent_chain_id"),
        )


@dataclass
class ListChainsResponse:
    """Response from listing chain executions."""
    chains: list[ChainSummary]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ListChainsResponse":
        return cls(
            chains=[ChainSummary.from_dict(c) for c in data["chains"]],
        )


@dataclass
class ChainStepStatus:
    """Detailed status of a single chain step.

    Attributes:
        name: Step name.
        provider: Provider used for this step.
        status: Step status (pending, completed, failed, skipped).
        response_body: Response body from the provider (if completed).
        error: Error message (if failed).
        completed_at: When this step completed.
        sub_chain: Name of the sub-chain this step triggers, if any.
        child_chain_id: ID of the child chain instance spawned by this step, if any.
    """
    name: str
    provider: str
    status: str
    response_body: Optional[Any] = None
    error: Optional[str] = None
    completed_at: Optional[str] = None
    sub_chain: Optional[str] = None
    child_chain_id: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ChainStepStatus":
        return cls(
            name=data["name"],
            provider=data["provider"],
            status=data["status"],
            response_body=data.get("response_body"),
            error=data.get("error"),
            completed_at=data.get("completed_at"),
            sub_chain=data.get("sub_chain"),
            child_chain_id=data.get("child_chain_id"),
        )


@dataclass
class ChainDetailResponse:
    """Full detail response for a chain execution.

    Attributes:
        chain_id: Unique chain execution ID.
        chain_name: Name of the chain configuration.
        status: Current status.
        current_step: Current step index (0-based).
        total_steps: Total number of steps.
        steps: Per-step status details.
        started_at: When the chain started.
        updated_at: When the chain was last updated.
        expires_at: When the chain will time out.
        cancel_reason: Reason for cancellation (if cancelled).
        cancelled_by: Who cancelled the chain (if cancelled).
        execution_path: Ordered list of step names that were executed.
        parent_chain_id: Parent chain ID if this is a sub-chain.
        child_chain_ids: IDs of child chains spawned by sub-chain steps.
    """
    chain_id: str
    chain_name: str
    status: str
    current_step: int
    total_steps: int
    steps: list[ChainStepStatus]
    started_at: str
    updated_at: str
    expires_at: Optional[str] = None
    cancel_reason: Optional[str] = None
    cancelled_by: Optional[str] = None
    execution_path: list[str] = field(default_factory=list)
    parent_chain_id: Optional[str] = None
    child_chain_ids: list[str] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ChainDetailResponse":
        return cls(
            chain_id=data["chain_id"],
            chain_name=data["chain_name"],
            status=data["status"],
            current_step=data["current_step"],
            total_steps=data["total_steps"],
            steps=[ChainStepStatus.from_dict(s) for s in data.get("steps", [])],
            started_at=data["started_at"],
            updated_at=data["updated_at"],
            expires_at=data.get("expires_at"),
            cancel_reason=data.get("cancel_reason"),
            cancelled_by=data.get("cancelled_by"),
            execution_path=data.get("execution_path", []),
            parent_chain_id=data.get("parent_chain_id"),
            child_chain_ids=data.get("child_chain_ids", []),
        )


# =============================================================================
# DAG Types (Chain Visualization)
# =============================================================================


@dataclass
class DagNode:
    """A node in the chain DAG.

    Attributes:
        name: Node name (step name or sub-chain name).
        node_type: Node type ("step" or "sub_chain").
        provider: Provider for this step, if applicable.
        action_type: Action type for this step, if applicable.
        sub_chain_name: Name of the sub-chain, if this is a sub-chain node.
        status: Current status of this node (for instance DAGs).
        child_chain_id: ID of the child chain instance (for instance DAGs).
        children: Nested DAG for sub-chain expansion.
    """
    name: str
    node_type: str
    provider: Optional[str] = None
    action_type: Optional[str] = None
    sub_chain_name: Optional[str] = None
    status: Optional[str] = None
    child_chain_id: Optional[str] = None
    children: Optional["DagResponse"] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "DagNode":
        children_data = data.get("children")
        return cls(
            name=data["name"],
            node_type=data["node_type"],
            provider=data.get("provider"),
            action_type=data.get("action_type"),
            sub_chain_name=data.get("sub_chain_name"),
            status=data.get("status"),
            child_chain_id=data.get("child_chain_id"),
            children=(
                DagResponse.from_dict(children_data)
                if children_data is not None
                else None
            ),
        )


@dataclass
class DagEdge:
    """An edge in the chain DAG.

    Attributes:
        source: Source node name.
        target: Target node name.
        label: Edge label (e.g., branch condition).
        on_execution_path: Whether this edge is on the execution path.
    """
    source: str
    target: str
    label: Optional[str] = None
    on_execution_path: bool = False

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "DagEdge":
        return cls(
            source=data["source"],
            target=data["target"],
            label=data.get("label"),
            on_execution_path=data.get("on_execution_path", False),
        )


@dataclass
class DagResponse:
    """DAG representation of a chain (config or instance).

    Attributes:
        chain_name: Chain configuration name.
        chain_id: Chain instance ID (only for instance DAGs).
        status: Chain status (only for instance DAGs).
        nodes: Nodes in the DAG.
        edges: Edges connecting the nodes.
        execution_path: Ordered list of step names on the execution path.
    """
    chain_name: str
    nodes: List[DagNode]
    edges: List[DagEdge]
    chain_id: Optional[str] = None
    status: Optional[str] = None
    execution_path: List[str] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "DagResponse":
        return cls(
            chain_name=data["chain_name"],
            chain_id=data.get("chain_id"),
            status=data.get("status"),
            nodes=[DagNode.from_dict(n) for n in data.get("nodes", [])],
            edges=[DagEdge.from_dict(e) for e in data.get("edges", [])],
            execution_path=data.get("execution_path", []),
        )


# =============================================================================
# DLQ Types (Dead-Letter Queue)
# =============================================================================


@dataclass
class DlqStatsResponse:
    """Response from the DLQ stats endpoint.

    Attributes:
        enabled: Whether the DLQ is enabled.
        count: Number of entries in the DLQ.
    """
    enabled: bool
    count: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "DlqStatsResponse":
        return cls(
            enabled=data["enabled"],
            count=data["count"],
        )


@dataclass
class DlqEntry:
    """A single dead-letter queue entry.

    Attributes:
        action_id: The failed action's unique identifier.
        namespace: Namespace the action belongs to.
        tenant: Tenant that owns the action.
        provider: Target provider for the action.
        action_type: Action type discriminator.
        error: Human-readable description of the final error.
        attempts: Number of execution attempts made.
        timestamp: Unix timestamp (seconds) when the entry was created.
    """
    action_id: str
    namespace: str
    tenant: str
    provider: str
    action_type: str
    error: str
    attempts: int
    timestamp: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "DlqEntry":
        return cls(
            action_id=data["action_id"],
            namespace=data["namespace"],
            tenant=data["tenant"],
            provider=data["provider"],
            action_type=data["action_type"],
            error=data["error"],
            attempts=data["attempts"],
            timestamp=data["timestamp"],
        )


@dataclass
class DlqDrainResponse:
    """Response from the DLQ drain endpoint.

    Attributes:
        entries: Entries drained from the DLQ.
        count: Number of entries drained.
    """
    entries: list[DlqEntry]
    count: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "DlqDrainResponse":
        return cls(
            entries=[DlqEntry.from_dict(e) for e in data["entries"]],
            count=data["count"],
        )


# =============================================================================
# SSE Event Types
# =============================================================================


@dataclass
class SseEvent:
    """A parsed Server-Sent Event.

    Attributes:
        event: The event type (e.g., "action_dispatched", "chain_completed").
        id: The event ID (if present).
        data: The parsed JSON data payload.
    """
    event: Optional[str] = None
    id: Optional[str] = None
    data: Optional[Any] = None


def _parse_sse_stream(lines: Iterator[str]) -> Iterator[SseEvent]:
    """Parse a text/event-stream into SseEvent objects.

    This is a simple line-by-line SSE parser that yields events as they
    arrive. It handles the ``event:``, ``id:``, and ``data:`` fields.
    Blank lines delimit events. Comment lines (starting with ``:``) are
    ignored.

    Args:
        lines: An iterator of lines from the SSE stream (without trailing newlines).

    Yields:
        Parsed SseEvent objects.
    """
    event_type: Optional[str] = None
    event_id: Optional[str] = None
    data_parts: list[str] = []

    for line in lines:
        if line.startswith(":"):
            # Comment line, skip.
            continue
        if line == "":
            # Blank line: dispatch event if we have data.
            if data_parts:
                raw_data = "\n".join(data_parts)
                try:
                    parsed = json.loads(raw_data)
                except (json.JSONDecodeError, ValueError):
                    parsed = raw_data
                yield SseEvent(event=event_type, id=event_id, data=parsed)
            # Reset for next event.
            event_type = None
            event_id = None
            data_parts = []
            continue
        if line.startswith("event:"):
            event_type = line[len("event:"):].strip()
        elif line.startswith("id:"):
            event_id = line[len("id:"):].strip()
        elif line.startswith("data:"):
            data_parts.append(line[len("data:"):].strip())
        # Other fields are ignored per the SSE spec.


# =============================================================================
# Provider Health Types
# =============================================================================


@dataclass
class ProviderHealthStatus:
    """Health and metrics for a single provider.

    Attributes:
        provider: Provider name.
        healthy: Whether the provider is healthy (circuit breaker closed).
        health_check_error: Error message from last health check (if any).
        circuit_breaker_state: Current circuit breaker state (closed, open, half_open).
        total_requests: Total number of requests to this provider.
        successes: Number of successful requests.
        failures: Number of failed requests.
        success_rate: Success rate as percentage (0-100).
        avg_latency_ms: Average request latency in milliseconds.
        p50_latency_ms: 50th percentile latency in milliseconds.
        p95_latency_ms: 95th percentile latency in milliseconds.
        p99_latency_ms: 99th percentile latency in milliseconds.
        last_request_at: Timestamp of last request (milliseconds since epoch).
        last_error: Last error message (if any).
    """
    provider: str
    healthy: bool
    circuit_breaker_state: str
    total_requests: int
    successes: int
    failures: int
    success_rate: float
    avg_latency_ms: float
    p50_latency_ms: float
    p95_latency_ms: float
    p99_latency_ms: float
    health_check_error: Optional[str] = None
    last_request_at: Optional[int] = None
    last_error: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ProviderHealthStatus":
        return cls(
            provider=data["provider"],
            healthy=data["healthy"],
            health_check_error=data.get("health_check_error"),
            circuit_breaker_state=data["circuit_breaker_state"],
            total_requests=data["total_requests"],
            successes=data["successes"],
            failures=data["failures"],
            success_rate=data["success_rate"],
            avg_latency_ms=data["avg_latency_ms"],
            p50_latency_ms=data["p50_latency_ms"],
            p95_latency_ms=data["p95_latency_ms"],
            p99_latency_ms=data["p99_latency_ms"],
            last_request_at=data.get("last_request_at"),
            last_error=data.get("last_error"),
        )


@dataclass
class ListProviderHealthResponse:
    """Response from listing provider health."""
    providers: list[ProviderHealthStatus]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ListProviderHealthResponse":
        return cls(
            providers=[ProviderHealthStatus.from_dict(p) for p in data["providers"]],
        )


# =============================================================================
# Provider Payload Helpers
# =============================================================================


def twilio_sms_payload(
    to: str,
    body: str,
    *,
    from_number: Optional[str] = None,
    media_url: Optional[str] = None,
) -> dict[str, Any]:
    """Build a payload for the Twilio SMS provider.

    Args:
        to: Destination phone number (E.164 format).
        body: Message body text.
        from_number: Override the default sender phone number.
        media_url: URL of media to attach (MMS).

    Returns:
        Payload dictionary suitable for an Action targeting the Twilio provider.
    """
    payload: dict[str, Any] = {"to": to, "body": body}
    if from_number is not None:
        payload["from"] = from_number
    if media_url is not None:
        payload["media_url"] = media_url
    return payload


def teams_message_payload(
    text: str,
    *,
    title: Optional[str] = None,
    theme_color: Optional[str] = None,
    summary: Optional[str] = None,
) -> dict[str, Any]:
    """Build a payload for the Microsoft Teams provider (MessageCard).

    Args:
        text: Message body text (supports basic markdown).
        title: Card title.
        theme_color: Hex color string (e.g., "FF0000").
        summary: Summary text for notifications.

    Returns:
        Payload dictionary suitable for an Action targeting the Teams provider.
    """
    payload: dict[str, Any] = {"text": text}
    if title is not None:
        payload["title"] = title
    if theme_color is not None:
        payload["theme_color"] = theme_color
    if summary is not None:
        payload["summary"] = summary
    return payload


def teams_adaptive_card_payload(card: dict[str, Any]) -> dict[str, Any]:
    """Build a payload for the Microsoft Teams provider (Adaptive Card).

    Args:
        card: The Adaptive Card JSON object.

    Returns:
        Payload dictionary suitable for an Action targeting the Teams provider.
    """
    return {"adaptive_card": card}


# =============================================================================
# WASM Plugin Types
# =============================================================================


@dataclass
class WasmPluginConfig:
    """Configuration for a WASM plugin.

    Attributes:
        memory_limit_bytes: Maximum memory in bytes the plugin can use.
        timeout_ms: Maximum execution time in milliseconds.
        allowed_host_functions: List of host functions the plugin may call.
    """
    memory_limit_bytes: Optional[int] = None
    timeout_ms: Optional[int] = None
    allowed_host_functions: Optional[List[str]] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "WasmPluginConfig":
        return cls(
            memory_limit_bytes=data.get("memory_limit_bytes"),
            timeout_ms=data.get("timeout_ms"),
            allowed_host_functions=data.get("allowed_host_functions"),
        )

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        result: dict[str, Any] = {}
        if self.memory_limit_bytes is not None:
            result["memory_limit_bytes"] = self.memory_limit_bytes
        if self.timeout_ms is not None:
            result["timeout_ms"] = self.timeout_ms
        if self.allowed_host_functions is not None:
            result["allowed_host_functions"] = self.allowed_host_functions
        return result


@dataclass
class WasmPlugin:
    """A registered WASM plugin.

    Attributes:
        name: Plugin name (unique identifier).
        status: Plugin status (e.g., "active", "disabled").
        enabled: Whether the plugin is enabled.
        created_at: When the plugin was registered.
        updated_at: When the plugin was last updated.
        invocation_count: Number of times the plugin has been invoked.
        description: Optional human-readable description.
        config: Plugin resource configuration.
    """
    name: str
    status: str
    enabled: bool
    created_at: str
    updated_at: str
    invocation_count: int = 0
    description: Optional[str] = None
    config: Optional[WasmPluginConfig] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "WasmPlugin":
        config_data = data.get("config")
        return cls(
            name=data["name"],
            status=data["status"],
            enabled=data.get("enabled", True),
            created_at=data["created_at"],
            updated_at=data["updated_at"],
            invocation_count=data.get("invocation_count", 0),
            description=data.get("description"),
            config=(
                WasmPluginConfig.from_dict(config_data)
                if config_data is not None
                else None
            ),
        )


@dataclass
class RegisterPluginRequest:
    """Request to register a new WASM plugin.

    Attributes:
        name: Plugin name (unique identifier).
        description: Optional human-readable description.
        wasm_bytes: Base64-encoded WASM module bytes.
        wasm_path: Path to the WASM file (server-side).
        config: Plugin resource configuration.
    """
    name: str
    description: Optional[str] = None
    wasm_bytes: Optional[str] = None
    wasm_path: Optional[str] = None
    config: Optional[WasmPluginConfig] = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        result: dict[str, Any] = {"name": self.name}
        if self.description is not None:
            result["description"] = self.description
        if self.wasm_bytes is not None:
            result["wasm_bytes"] = self.wasm_bytes
        if self.wasm_path is not None:
            result["wasm_path"] = self.wasm_path
        if self.config is not None:
            result["config"] = self.config.to_dict()
        return result


@dataclass
class ListPluginsResponse:
    """Response from listing WASM plugins."""
    plugins: list[WasmPlugin]
    count: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ListPluginsResponse":
        return cls(
            plugins=[WasmPlugin.from_dict(p) for p in data["plugins"]],
            count=data["count"],
        )


@dataclass
class PluginInvocationRequest:
    """Request to test-invoke a WASM plugin.

    Attributes:
        input: JSON input to pass to the plugin.
        function: The function to invoke (default: "evaluate").
    """
    input: Dict[str, Any]
    function: Optional[str] = None

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        result: dict[str, Any] = {"input": self.input}
        if self.function is not None:
            result["function"] = self.function
        return result


@dataclass
class PluginInvocationResponse:
    """Response from test-invoking a WASM plugin.

    Attributes:
        verdict: Whether the plugin evaluation returned true or false.
        message: Optional message from the plugin.
        metadata: Optional structured metadata from the plugin.
        duration_ms: Execution time in milliseconds.
    """
    verdict: bool
    message: Optional[str] = None
    metadata: Optional[Dict[str, Any]] = None
    duration_ms: Optional[float] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "PluginInvocationResponse":
        return cls(
            verdict=data["verdict"],
            message=data.get("message"),
            metadata=data.get("metadata"),
            duration_ms=data.get("duration_ms"),
        )


# =============================================================================
# Compliance Types (SOC2/HIPAA Audit Mode)
# =============================================================================


@dataclass
class ComplianceStatus:
    """Current compliance configuration status.

    Attributes:
        mode: The active compliance mode ("none", "soc2", or "hipaa").
        sync_audit_writes: Whether audit writes block the dispatch pipeline.
        immutable_audit: Whether audit records are immutable (deletes rejected).
        hash_chain: Whether SHA-256 hash chaining is enabled for audit records.
    """
    mode: str
    sync_audit_writes: bool
    immutable_audit: bool
    hash_chain: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ComplianceStatus":
        return cls(
            mode=data["mode"],
            sync_audit_writes=data["sync_audit_writes"],
            immutable_audit=data["immutable_audit"],
            hash_chain=data["hash_chain"],
        )


@dataclass
class HashChainVerification:
    """Result of verifying the integrity of an audit hash chain.

    Attributes:
        valid: Whether the hash chain is intact (no broken links).
        records_checked: Total number of records verified.
        first_broken_at: ID of the first record where the chain broke, if any.
        first_record_id: ID of the first record in the verified range.
        last_record_id: ID of the last record in the verified range.
    """
    valid: bool
    records_checked: int
    first_broken_at: Optional[str] = None
    first_record_id: Optional[str] = None
    last_record_id: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "HashChainVerification":
        return cls(
            valid=data["valid"],
            records_checked=data["records_checked"],
            first_broken_at=data.get("first_broken_at"),
            first_record_id=data.get("first_record_id"),
            last_record_id=data.get("last_record_id"),
        )


@dataclass
class VerifyHashChainRequest:
    """Request body for hash chain verification.

    Attributes:
        namespace: Namespace to verify.
        tenant: Tenant to verify.
        from_time: Optional start of the time range (ISO 8601).
        to_time: Optional end of the time range (ISO 8601).
    """
    namespace: str
    tenant: str
    from_time: Optional[str] = None
    to_time: Optional[str] = None

    def to_dict(self) -> dict[str, Any]:
        body: dict[str, Any] = {
            "namespace": self.namespace,
            "tenant": self.tenant,
        }
        if self.from_time is not None:
            body["from"] = self.from_time
        if self.to_time is not None:
            body["to"] = self.to_time
        return body


def discord_message_payload(
    *,
    content: Optional[str] = None,
    embeds: Optional[List[dict[str, Any]]] = None,
    username: Optional[str] = None,
    avatar_url: Optional[str] = None,
    tts: Optional[bool] = None,
) -> dict[str, Any]:
    """Build a payload for the Discord webhook provider.

    At least one of ``content`` or ``embeds`` must be provided.

    Args:
        content: Plain-text message content.
        embeds: List of Discord embed objects.
        username: Override the webhook's default username.
        avatar_url: Override the webhook's default avatar.
        tts: Whether the message should be read aloud.

    Returns:
        Payload dictionary suitable for an Action targeting the Discord provider.
    """
    payload: dict[str, Any] = {}
    if content is not None:
        payload["content"] = content
    if embeds is not None:
        payload["embeds"] = embeds
    if username is not None:
        payload["username"] = username
    if avatar_url is not None:
        payload["avatar_url"] = avatar_url
    if tts is not None:
        payload["tts"] = tts
    return payload


# =============================================================================
# AWS Provider Payload Helpers
# =============================================================================


def sns_publish_payload(
    message: str,
    *,
    subject: Optional[str] = None,
    topic_arn: Optional[str] = None,
    message_group_id: Optional[str] = None,
    message_dedup_id: Optional[str] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS SNS provider.

    Args:
        message: Message body to publish.
        subject: Subject for email-protocol subscriptions.
        topic_arn: Override the topic ARN configured on the provider.
        message_group_id: Message group ID (for FIFO topics).
        message_dedup_id: Message deduplication ID (for FIFO topics).

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-sns`` provider.
    """
    payload: dict[str, Any] = {"message": message}
    if subject is not None:
        payload["subject"] = subject
    if topic_arn is not None:
        payload["topic_arn"] = topic_arn
    if message_group_id is not None:
        payload["message_group_id"] = message_group_id
    if message_dedup_id is not None:
        payload["message_dedup_id"] = message_dedup_id
    return payload


def lambda_invoke_payload(
    payload_data: Any = None,
    *,
    function_name: Optional[str] = None,
    invocation_type: Optional[str] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS Lambda provider.

    Args:
        payload_data: JSON-serializable data to pass to the Lambda function.
        function_name: Override the function name configured on the provider.
        invocation_type: ``"RequestResponse"``, ``"Event"``, or ``"DryRun"``.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-lambda`` provider.
    """
    payload: dict[str, Any] = {}
    if payload_data is not None:
        payload["payload"] = payload_data
    if function_name is not None:
        payload["function_name"] = function_name
    if invocation_type is not None:
        payload["invocation_type"] = invocation_type
    return payload


def eventbridge_put_event_payload(
    source: str,
    detail_type: str,
    detail: Any,
    *,
    event_bus_name: Optional[str] = None,
    resources: Optional[List[str]] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS ``EventBridge`` provider.

    Args:
        source: Event source (e.g., ``"com.myapp.orders"``).
        detail_type: Event detail type (e.g., ``"OrderCreated"``).
        detail: Event detail as a JSON-serializable value.
        event_bus_name: Override the event bus name configured on the provider.
        resources: List of resource ARNs associated with the event.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-eventbridge`` provider.
    """
    payload: dict[str, Any] = {
        "source": source,
        "detail_type": detail_type,
        "detail": detail,
    }
    if event_bus_name is not None:
        payload["event_bus_name"] = event_bus_name
    if resources is not None:
        payload["resources"] = resources
    return payload


def sqs_send_message_payload(
    message_body: str,
    *,
    queue_url: Optional[str] = None,
    delay_seconds: Optional[int] = None,
    message_group_id: Optional[str] = None,
    message_dedup_id: Optional[str] = None,
    message_attributes: Optional[dict[str, str]] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS SQS provider.

    Args:
        message_body: Message body text.
        queue_url: Override the queue URL configured on the provider.
        delay_seconds: Delivery delay in seconds (0-900).
        message_group_id: Message group ID (for FIFO queues).
        message_dedup_id: Message deduplication ID (for FIFO queues).
        message_attributes: Message attributes as key-value pairs.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-sqs`` provider.
    """
    payload: dict[str, Any] = {"message_body": message_body}
    if queue_url is not None:
        payload["queue_url"] = queue_url
    if delay_seconds is not None:
        payload["delay_seconds"] = delay_seconds
    if message_group_id is not None:
        payload["message_group_id"] = message_group_id
    if message_dedup_id is not None:
        payload["message_dedup_id"] = message_dedup_id
    if message_attributes is not None:
        payload["message_attributes"] = message_attributes
    return payload


def s3_put_object_payload(
    key: str,
    *,
    bucket: Optional[str] = None,
    body: Optional[str] = None,
    body_base64: Optional[str] = None,
    content_type: Optional[str] = None,
    metadata: Optional[dict[str, str]] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS S3 put-object action.

    Args:
        key: S3 object key.
        bucket: Override the bucket name configured on the provider.
        body: Object body as a UTF-8 string.
        body_base64: Object body as base64-encoded bytes.
        content_type: Content type (e.g., ``"application/json"``).
        metadata: Object metadata as key-value pairs.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-s3`` provider
        with action type ``put_object``.
    """
    payload: dict[str, Any] = {"key": key}
    if bucket is not None:
        payload["bucket"] = bucket
    if body is not None:
        payload["body"] = body
    if body_base64 is not None:
        payload["body_base64"] = body_base64
    if content_type is not None:
        payload["content_type"] = content_type
    if metadata is not None:
        payload["metadata"] = metadata
    return payload


def s3_get_object_payload(
    key: str,
    *,
    bucket: Optional[str] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS S3 get-object action.

    Args:
        key: S3 object key.
        bucket: Override the bucket name configured on the provider.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-s3`` provider
        with action type ``get_object``.
    """
    payload: dict[str, Any] = {"key": key}
    if bucket is not None:
        payload["bucket"] = bucket
    return payload


def s3_delete_object_payload(
    key: str,
    *,
    bucket: Optional[str] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS S3 delete-object action.

    Args:
        key: S3 object key.
        bucket: Override the bucket name configured on the provider.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-s3`` provider
        with action type ``delete_object``.
    """
    payload: dict[str, Any] = {"key": key}
    if bucket is not None:
        payload["bucket"] = bucket
    return payload


# =============================================================================
# AWS EC2 Provider Payload Helpers
# =============================================================================


def ec2_start_instances_payload(
    instance_ids: List[str],
) -> dict[str, Any]:
    """Build a payload for the AWS EC2 start-instances action.

    Args:
        instance_ids: List of EC2 instance IDs to start.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-ec2`` provider
        with action type ``start_instances``.
    """
    return {"instance_ids": instance_ids}


def ec2_stop_instances_payload(
    instance_ids: List[str],
    *,
    hibernate: Optional[bool] = None,
    force: Optional[bool] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS EC2 stop-instances action.

    Args:
        instance_ids: List of EC2 instance IDs to stop.
        hibernate: Whether to hibernate the instances instead of stopping.
        force: Whether to force stop the instances.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-ec2`` provider
        with action type ``stop_instances``.
    """
    payload: dict[str, Any] = {"instance_ids": instance_ids}
    if hibernate is not None:
        payload["hibernate"] = hibernate
    if force is not None:
        payload["force"] = force
    return payload


def ec2_reboot_instances_payload(
    instance_ids: List[str],
) -> dict[str, Any]:
    """Build a payload for the AWS EC2 reboot-instances action.

    Args:
        instance_ids: List of EC2 instance IDs to reboot.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-ec2`` provider
        with action type ``reboot_instances``.
    """
    return {"instance_ids": instance_ids}


def ec2_terminate_instances_payload(
    instance_ids: List[str],
) -> dict[str, Any]:
    """Build a payload for the AWS EC2 terminate-instances action.

    Args:
        instance_ids: List of EC2 instance IDs to terminate.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-ec2`` provider
        with action type ``terminate_instances``.
    """
    return {"instance_ids": instance_ids}


def ec2_hibernate_instances_payload(
    instance_ids: List[str],
) -> dict[str, Any]:
    """Build a payload for the AWS EC2 hibernate-instances action.

    Args:
        instance_ids: List of EC2 instance IDs to hibernate.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-ec2`` provider
        with action type ``hibernate_instances``.
    """
    return {"instance_ids": instance_ids}


def ec2_run_instances_payload(
    image_id: str,
    instance_type: str,
    *,
    min_count: Optional[int] = None,
    max_count: Optional[int] = None,
    key_name: Optional[str] = None,
    security_group_ids: Optional[List[str]] = None,
    subnet_id: Optional[str] = None,
    user_data: Optional[str] = None,
    tags: Optional[dict[str, str]] = None,
    iam_instance_profile: Optional[str] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS EC2 run-instances action.

    Args:
        image_id: AMI ID to launch.
        instance_type: EC2 instance type (e.g., ``"t3.micro"``).
        min_count: Minimum number of instances to launch.
        max_count: Maximum number of instances to launch.
        key_name: Name of the key pair for SSH access.
        security_group_ids: List of security group IDs.
        subnet_id: VPC subnet ID to launch into.
        user_data: Base64-encoded user data script.
        tags: Tags to apply to the launched instances.
        iam_instance_profile: IAM instance profile name or ARN.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-ec2`` provider
        with action type ``run_instances``.
    """
    payload: dict[str, Any] = {
        "image_id": image_id,
        "instance_type": instance_type,
    }
    if min_count is not None:
        payload["min_count"] = min_count
    if max_count is not None:
        payload["max_count"] = max_count
    if key_name is not None:
        payload["key_name"] = key_name
    if security_group_ids is not None:
        payload["security_group_ids"] = security_group_ids
    if subnet_id is not None:
        payload["subnet_id"] = subnet_id
    if user_data is not None:
        payload["user_data"] = user_data
    if tags is not None:
        payload["tags"] = tags
    if iam_instance_profile is not None:
        payload["iam_instance_profile"] = iam_instance_profile
    return payload


def ec2_attach_volume_payload(
    volume_id: str,
    instance_id: str,
    device: str,
) -> dict[str, Any]:
    """Build a payload for the AWS EC2 attach-volume action.

    Args:
        volume_id: EBS volume ID to attach.
        instance_id: EC2 instance ID to attach the volume to.
        device: Device name (e.g., ``"/dev/sdf"``).

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-ec2`` provider
        with action type ``attach_volume``.
    """
    return {
        "volume_id": volume_id,
        "instance_id": instance_id,
        "device": device,
    }


def ec2_detach_volume_payload(
    volume_id: str,
    *,
    instance_id: Optional[str] = None,
    device: Optional[str] = None,
    force: Optional[bool] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS EC2 detach-volume action.

    Args:
        volume_id: EBS volume ID to detach.
        instance_id: EC2 instance ID to detach the volume from.
        device: Device name.
        force: Whether to force detach the volume.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-ec2`` provider
        with action type ``detach_volume``.
    """
    payload: dict[str, Any] = {"volume_id": volume_id}
    if instance_id is not None:
        payload["instance_id"] = instance_id
    if device is not None:
        payload["device"] = device
    if force is not None:
        payload["force"] = force
    return payload


def ec2_describe_instances_payload(
    *,
    instance_ids: Optional[List[str]] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS EC2 describe-instances action.

    Args:
        instance_ids: Optional list of EC2 instance IDs to describe.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-ec2`` provider
        with action type ``describe_instances``.
    """
    payload: dict[str, Any] = {}
    if instance_ids is not None:
        payload["instance_ids"] = instance_ids
    return payload


# =============================================================================
# AWS Auto Scaling Provider Payload Helpers
# =============================================================================


def autoscaling_describe_groups_payload(
    *,
    group_names: Optional[List[str]] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS Auto Scaling describe-groups action.

    Args:
        group_names: Optional list of Auto Scaling group names to describe.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-autoscaling``
        provider with action type ``describe_auto_scaling_groups``.
    """
    payload: dict[str, Any] = {}
    if group_names is not None:
        payload["auto_scaling_group_names"] = group_names
    return payload


def autoscaling_set_desired_capacity_payload(
    group_name: str,
    desired_capacity: int,
    *,
    honor_cooldown: Optional[bool] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS Auto Scaling set-desired-capacity action.

    Args:
        group_name: Auto Scaling group name.
        desired_capacity: Desired number of instances.
        honor_cooldown: Whether to honor the cooldown period.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-autoscaling``
        provider with action type ``set_desired_capacity``.
    """
    payload: dict[str, Any] = {
        "auto_scaling_group_name": group_name,
        "desired_capacity": desired_capacity,
    }
    if honor_cooldown is not None:
        payload["honor_cooldown"] = honor_cooldown
    return payload


def autoscaling_update_group_payload(
    group_name: str,
    *,
    min_size: Optional[int] = None,
    max_size: Optional[int] = None,
    desired_capacity: Optional[int] = None,
    default_cooldown: Optional[int] = None,
    health_check_type: Optional[str] = None,
    health_check_grace_period: Optional[int] = None,
) -> dict[str, Any]:
    """Build a payload for the AWS Auto Scaling update-group action.

    Args:
        group_name: Auto Scaling group name.
        min_size: Minimum group size.
        max_size: Maximum group size.
        desired_capacity: Desired number of instances.
        default_cooldown: Default cooldown period in seconds.
        health_check_type: Health check type (e.g., ``"EC2"``, ``"ELB"``).
        health_check_grace_period: Health check grace period in seconds.

    Returns:
        Payload dictionary suitable for an Action targeting the ``aws-autoscaling``
        provider with action type ``update_auto_scaling_group``.
    """
    payload: dict[str, Any] = {
        "auto_scaling_group_name": group_name,
    }
    if min_size is not None:
        payload["min_size"] = min_size
    if max_size is not None:
        payload["max_size"] = max_size
    if desired_capacity is not None:
        payload["desired_capacity"] = desired_capacity
    if default_cooldown is not None:
        payload["default_cooldown"] = default_cooldown
    if health_check_type is not None:
        payload["health_check_type"] = health_check_type
    if health_check_grace_period is not None:
        payload["health_check_grace_period"] = health_check_grace_period
    return payload
