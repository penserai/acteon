"""Data models for the Acteon client."""

from dataclasses import dataclass, field
from typing import Any, Optional
from datetime import datetime
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
