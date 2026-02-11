"""Data models for the Acteon client."""

from dataclasses import dataclass, field
from typing import Any, Iterator, Optional
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
    """
    chain_id: str
    chain_name: str
    status: str
    current_step: int
    total_steps: int
    started_at: str
    updated_at: str

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
    """
    name: str
    provider: str
    status: str
    response_body: Optional[Any] = None
    error: Optional[str] = None
    completed_at: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ChainStepStatus":
        return cls(
            name=data["name"],
            provider=data["provider"],
            status=data["status"],
            response_body=data.get("response_body"),
            error=data.get("error"),
            completed_at=data.get("completed_at"),
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
