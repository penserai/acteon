"""Acteon Python Client - HTTP client for the Acteon action gateway."""

from .client import ActeonClient, AsyncActeonClient
from .errors import ActeonError, ConnectionError, ApiError, HttpError
from .models import (
    Action,
    ActionOutcome,
    BatchResult,
    RuleInfo,
    ReloadResult,
    AuditQuery,
    AuditPage,
    AuditRecord,
    EventQuery,
    EventState,
    EventListResponse,
    TransitionResponse,
    GroupSummary,
    GroupListResponse,
    GroupDetail,
    FlushGroupResponse,
)

__version__ = "0.1.0"
__all__ = [
    "ActeonClient",
    "AsyncActeonClient",
    "ActeonError",
    "ConnectionError",
    "ApiError",
    "HttpError",
    "Action",
    "ActionOutcome",
    "BatchResult",
    "RuleInfo",
    "ReloadResult",
    "AuditQuery",
    "AuditPage",
    "AuditRecord",
    "EventQuery",
    "EventState",
    "EventListResponse",
    "TransitionResponse",
    "GroupSummary",
    "GroupListResponse",
    "GroupDetail",
    "FlushGroupResponse",
]
