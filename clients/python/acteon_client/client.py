"""HTTP client for the Acteon action gateway."""

from collections.abc import AsyncIterator
from typing import Iterator, Optional
import httpx

from .errors import ActeonError, ConnectionError, HttpError, ApiError
from .models import (
    Action,
    ActionOutcome,
    BatchResult,
    RuleInfo,
    ReloadResult,
    EvaluateRulesRequest,
    EvaluateRulesResponse,
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
    ApprovalActionResponse,
    ApprovalStatus,
    ApprovalListResponse,
    ReplayResult,
    ReplaySummary,
    ReplayQuery,
    CreateRecurringAction,
    CreateRecurringResponse,
    RecurringFilter,
    RecurringSummary,
    ListRecurringResponse,
    RecurringDetail,
    UpdateRecurringAction,
    CreateQuotaRequest,
    UpdateQuotaRequest,
    QuotaPolicy,
    ListQuotasResponse,
    QuotaUsage,
    ChainSummary,
    ListChainsResponse,
    ChainDetailResponse,
    DlqStatsResponse,
    DlqDrainResponse,
    SseEvent,
    _parse_sse_stream,
)


class ActeonClient:
    """HTTP client for the Acteon action gateway.

    Example:
        >>> client = ActeonClient("http://localhost:8080")
        >>> if client.health():
        ...     action = Action(
        ...         namespace="notifications",
        ...         tenant="tenant-1",
        ...         provider="email",
        ...         action_type="send_notification",
        ...         payload={"to": "user@example.com", "subject": "Hello"},
        ...     )
        ...     outcome = client.dispatch(action)
        ...     print(f"Outcome: {outcome.outcome_type}")
    """

    def __init__(
        self,
        base_url: str,
        *,
        timeout: float = 30.0,
        api_key: Optional[str] = None,
    ):
        """Create a new Acteon client.

        Args:
            base_url: Base URL of the Acteon server (e.g., "http://localhost:8080").
            timeout: Request timeout in seconds.
            api_key: Optional API key for authentication.
        """
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self._client = httpx.Client(timeout=timeout)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()

    def close(self):
        """Close the HTTP client."""
        self._client.close()

    def _headers(self) -> dict[str, str]:
        """Get request headers."""
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        return headers

    def _request(
        self,
        method: str,
        path: str,
        *,
        json: Optional[dict] = None,
        params: Optional[dict] = None,
    ) -> httpx.Response:
        """Make an HTTP request."""
        url = f"{self.base_url}{path}"
        try:
            response = self._client.request(
                method,
                url,
                json=json,
                params=params,
                headers=self._headers(),
            )
            return response
        except httpx.ConnectError as e:
            raise ConnectionError(str(e)) from e
        except httpx.TimeoutException as e:
            raise ConnectionError(f"Request timed out: {e}") from e

    # =========================================================================
    # Health
    # =========================================================================

    def health(self) -> bool:
        """Check if the server is healthy.

        Returns:
            True if the server is healthy, False otherwise.
        """
        try:
            response = self._request("GET", "/health")
            return response.status_code == 200
        except ConnectionError:
            return False

    # =========================================================================
    # Action Dispatch
    # =========================================================================

    def dispatch(
        self, action: Action, *, dry_run: bool = False
    ) -> ActionOutcome:
        """Dispatch a single action.

        Args:
            action: The action to dispatch.
            dry_run: When True, evaluates rules without executing the action.

        Returns:
            The outcome of the action.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        params = {"dry_run": "true"} if dry_run else None
        response = self._request(
            "POST", "/v1/dispatch", json=action.to_dict(), params=params
        )

        if response.status_code == 200:
            return ActionOutcome.from_dict(response.json())
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    def dispatch_dry_run(self, action: Action) -> ActionOutcome:
        """Dispatch a single action in dry-run mode.

        Rules are evaluated but the action is not executed and no state is mutated.

        Args:
            action: The action to evaluate.

        Returns:
            A DryRun outcome describing what would happen.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        return self.dispatch(action, dry_run=True)

    def dispatch_batch(
        self, actions: list[Action], *, dry_run: bool = False
    ) -> list[BatchResult]:
        """Dispatch multiple actions in a single request.

        Args:
            actions: List of actions to dispatch.
            dry_run: When True, evaluates rules without executing any actions.

        Returns:
            List of results, one per action.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns a batch-level error.
        """
        params = {"dry_run": "true"} if dry_run else None
        response = self._request(
            "POST",
            "/v1/dispatch/batch",
            json=[a.to_dict() for a in actions],
            params=params,
        )

        if response.status_code == 200:
            return [BatchResult.from_dict(r) for r in response.json()]
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    def dispatch_batch_dry_run(self, actions: list[Action]) -> list[BatchResult]:
        """Dispatch multiple actions in dry-run mode.

        Rules are evaluated for each action but none are executed and no state is mutated.

        Args:
            actions: List of actions to evaluate.

        Returns:
            List of DryRun results, one per action.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns a batch-level error.
        """
        return self.dispatch_batch(actions, dry_run=True)

    # =========================================================================
    # Rules Management
    # =========================================================================

    def list_rules(self) -> list[RuleInfo]:
        """List all loaded rules.

        Returns:
            List of rule information.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        response = self._request("GET", "/v1/rules")

        if response.status_code == 200:
            return [RuleInfo.from_dict(r) for r in response.json()]
        else:
            raise HttpError(response.status_code, f"Failed to list rules")

    def reload_rules(self) -> ReloadResult:
        """Reload rules from the configured directory.

        Returns:
            Result indicating how many rules were loaded.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        response = self._request("POST", "/v1/rules/reload")

        if response.status_code == 200:
            return ReloadResult.from_dict(response.json())
        else:
            raise HttpError(response.status_code, f"Failed to reload rules")

    def set_rule_enabled(self, rule_name: str, enabled: bool) -> None:
        """Enable or disable a specific rule.

        Args:
            rule_name: Name of the rule to modify.
            enabled: Whether to enable or disable the rule.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        response = self._request(
            "PUT",
            f"/v1/rules/{rule_name}/enabled",
            json={"enabled": enabled},
        )

        if response.status_code != 200:
            raise HttpError(response.status_code, f"Failed to set rule enabled")

    def evaluate_rules(self, request: EvaluateRulesRequest) -> EvaluateRulesResponse:
        """Evaluate rules against a test action without dispatching.

        This is the Rule Playground endpoint. It evaluates all matching rules
        against the provided action parameters and returns a detailed trace
        of each rule evaluation.

        Args:
            request: The evaluation request with action parameters.

        Returns:
            Detailed evaluation response with verdict, trace, and context.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        body: dict = {
            "namespace": request.namespace,
            "tenant": request.tenant,
            "provider": request.provider,
            "action_type": request.action_type,
            "payload": request.payload,
        }
        if request.metadata:
            body["metadata"] = request.metadata
        if request.include_disabled:
            body["include_disabled"] = True
        if request.evaluate_all:
            body["evaluate_all"] = True
        if request.evaluate_at:
            body["evaluate_at"] = request.evaluate_at
        if request.mock_state:
            body["mock_state"] = request.mock_state

        response = self._request("POST", "/v1/rules/evaluate", json=body)

        if response.status_code == 200:
            return EvaluateRulesResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to evaluate rules")

    # =========================================================================
    # Audit Trail
    # =========================================================================

    def query_audit(self, query: Optional[AuditQuery] = None) -> AuditPage:
        """Query audit records.

        Args:
            query: Optional query parameters.

        Returns:
            Paginated audit results.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        params = query.to_params() if query else {}
        response = self._request("GET", "/v1/audit", params=params)

        if response.status_code == 200:
            return AuditPage.from_dict(response.json())
        else:
            raise HttpError(response.status_code, f"Failed to query audit")

    def get_audit_record(self, action_id: str) -> Optional[AuditRecord]:
        """Get a specific audit record by action ID.

        Args:
            action_id: The action ID to look up.

        Returns:
            The audit record, or None if not found.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error (other than 404).
        """
        response = self._request("GET", f"/v1/audit/{action_id}")

        if response.status_code == 200:
            return AuditRecord.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, f"Failed to get audit record")

    # =========================================================================
    # Audit Replay
    # =========================================================================

    def replay_action(self, action_id: str) -> ReplayResult:
        """Replay a single action from the audit trail by its action ID.

        Args:
            action_id: The action ID to replay.

        Returns:
            The replay result with new action ID.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the audit record is not found (404) or has no payload (422).
        """
        response = self._request("POST", f"/v1/audit/{action_id}/replay")

        if response.status_code == 200:
            return ReplayResult.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Audit record not found: {action_id}")
        elif response.status_code == 422:
            raise HttpError(422, "No stored payload available for replay")
        else:
            raise HttpError(response.status_code, "Failed to replay action")

    def replay_audit(self, query: Optional[ReplayQuery] = None) -> ReplaySummary:
        """Bulk replay actions from the audit trail matching the given query.

        Args:
            query: Optional query parameters to filter which records to replay.

        Returns:
            Summary of the replay operation.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        params = query.to_params() if query else {}
        response = self._request("POST", "/v1/audit/replay", params=params)

        if response.status_code == 200:
            return ReplaySummary.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to replay audit")

    # =========================================================================
    # Events (State Machine Lifecycle)
    # =========================================================================

    def list_events(self, query: EventQuery) -> EventListResponse:
        """List events filtered by namespace, tenant, and optionally status.

        Args:
            query: Query parameters for filtering events.

        Returns:
            List of events matching the query.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        response = self._request("GET", "/v1/events", params=query.to_params())

        if response.status_code == 200:
            return EventListResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list events")

    def get_event(
        self, fingerprint: str, namespace: str, tenant: str
    ) -> Optional[EventState]:
        """Get the current state of an event by fingerprint.

        Args:
            fingerprint: The event fingerprint.
            namespace: The event namespace.
            tenant: The event tenant.

        Returns:
            The event state, or None if not found.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error (other than 404).
        """
        response = self._request(
            "GET",
            f"/v1/events/{fingerprint}",
            params={"namespace": namespace, "tenant": tenant},
        )

        if response.status_code == 200:
            return EventState.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get event")

    def transition_event(
        self, fingerprint: str, to_state: str, namespace: str, tenant: str
    ) -> TransitionResponse:
        """Transition an event to a new state.

        Args:
            fingerprint: The event fingerprint.
            to_state: The target state to transition to.
            namespace: The event namespace.
            tenant: The event tenant.

        Returns:
            Details of the transition.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the event is not found (404).
            ApiError: If the server returns an error.
        """
        response = self._request(
            "PUT",
            f"/v1/events/{fingerprint}/transition",
            json={"to": to_state, "namespace": namespace, "tenant": tenant},
        )

        if response.status_code == 200:
            return TransitionResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Event not found: {fingerprint}")
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    # =========================================================================
    # Groups (Event Batching)
    # =========================================================================

    def list_groups(self) -> GroupListResponse:
        """List all active event groups.

        Returns:
            List of active groups.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        response = self._request("GET", "/v1/groups")

        if response.status_code == 200:
            return GroupListResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list groups")

    def get_group(self, group_key: str) -> Optional[GroupDetail]:
        """Get details of a specific group.

        Args:
            group_key: The group key.

        Returns:
            The group details, or None if not found.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error (other than 404).
        """
        response = self._request("GET", f"/v1/groups/{group_key}")

        if response.status_code == 200:
            return GroupDetail.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get group")

    def flush_group(self, group_key: str) -> FlushGroupResponse:
        """Force flush a group, triggering immediate notification.

        Args:
            group_key: The group key to flush.

        Returns:
            Details of the flushed group.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the group is not found (404).
            ApiError: If the server returns an error.
        """
        response = self._request("DELETE", f"/v1/groups/{group_key}")

        if response.status_code == 200:
            return FlushGroupResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Group not found: {group_key}")
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    # =========================================================================
    # Approvals (Human-in-the-Loop)
    # =========================================================================

    def approve(self, namespace: str, tenant: str, id: str, sig: str, expires_at: int, kid: Optional[str] = None) -> ApprovalActionResponse:
        """Approve a pending action by namespace, tenant, ID, and HMAC signature.

        Args:
            namespace: The approval namespace.
            tenant: The approval tenant.
            id: The approval ID.
            sig: The HMAC-SHA256 signature.
            expires_at: Expiration timestamp (unix seconds) bound into the signature.
            kid: Optional key ID identifying which HMAC key was used.

        Returns:
            The approval result with optional action outcome.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If approval not found (404) or already decided (410).
        """
        params: dict = {"sig": sig, "expires_at": expires_at}
        if kid is not None:
            params["kid"] = kid
        response = self._request(
            "POST",
            f"/v1/approvals/{namespace}/{tenant}/{id}/approve",
            params=params,
        )

        if response.status_code == 200:
            return ApprovalActionResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, "Approval not found or expired")
        elif response.status_code == 410:
            raise HttpError(410, "Approval already decided")
        else:
            raise HttpError(response.status_code, "Failed to approve")

    def reject(self, namespace: str, tenant: str, id: str, sig: str, expires_at: int, kid: Optional[str] = None) -> ApprovalActionResponse:
        """Reject a pending action by namespace, tenant, ID, and HMAC signature.

        Args:
            namespace: The approval namespace.
            tenant: The approval tenant.
            id: The approval ID.
            sig: The HMAC-SHA256 signature.
            expires_at: Expiration timestamp (unix seconds) bound into the signature.
            kid: Optional key ID identifying which HMAC key was used.

        Returns:
            The rejection result.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If approval not found (404) or already decided (410).
        """
        params: dict = {"sig": sig, "expires_at": expires_at}
        if kid is not None:
            params["kid"] = kid
        response = self._request(
            "POST",
            f"/v1/approvals/{namespace}/{tenant}/{id}/reject",
            params=params,
        )

        if response.status_code == 200:
            return ApprovalActionResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, "Approval not found or expired")
        elif response.status_code == 410:
            raise HttpError(410, "Approval already decided")
        else:
            raise HttpError(response.status_code, "Failed to reject")

    def get_approval(self, namespace: str, tenant: str, id: str, sig: str, expires_at: int, kid: Optional[str] = None) -> Optional[ApprovalStatus]:
        """Get the status of an approval by namespace, tenant, ID, and HMAC signature.

        Args:
            namespace: The approval namespace.
            tenant: The approval tenant.
            id: The approval ID.
            sig: The HMAC-SHA256 signature.
            expires_at: Expiration timestamp (unix seconds) bound into the signature.
            kid: Optional key ID identifying which HMAC key was used.

        Returns:
            The approval status, or None if not found.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error (other than 404).
        """
        params: dict = {"sig": sig, "expires_at": expires_at}
        if kid is not None:
            params["kid"] = kid
        response = self._request(
            "GET",
            f"/v1/approvals/{namespace}/{tenant}/{id}",
            params=params,
        )

        if response.status_code == 200:
            return ApprovalStatus.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get approval")

    def list_approvals(
        self, namespace: str, tenant: str
    ) -> ApprovalListResponse:
        """List pending approvals filtered by namespace and tenant.

        Args:
            namespace: The namespace to filter by.
            tenant: The tenant to filter by.

        Returns:
            List of pending approvals.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        response = self._request(
            "GET",
            "/v1/approvals",
            params={"namespace": namespace, "tenant": tenant},
        )

        if response.status_code == 200:
            return ApprovalListResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list approvals")


    # =========================================================================
    # Recurring Actions
    # =========================================================================

    def create_recurring(
        self, recurring: CreateRecurringAction
    ) -> CreateRecurringResponse:
        """Create a recurring action.

        Args:
            recurring: The recurring action definition.

        Returns:
            The created recurring action response with ID and next execution time.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns a validation error.
        """
        response = self._request("POST", "/v1/recurring", json=recurring.to_dict())

        if response.status_code == 201:
            return CreateRecurringResponse.from_dict(response.json())
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    def list_recurring(
        self, filter: Optional[RecurringFilter] = None
    ) -> ListRecurringResponse:
        """List recurring actions.

        Args:
            filter: Optional filter parameters.

        Returns:
            List of recurring action summaries.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        params = filter.to_params() if filter else {}
        response = self._request("GET", "/v1/recurring", params=params)

        if response.status_code == 200:
            return ListRecurringResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list recurring actions")

    def get_recurring(
        self, recurring_id: str, namespace: str, tenant: str
    ) -> Optional[RecurringDetail]:
        """Get details of a specific recurring action.

        Args:
            recurring_id: The recurring action ID.
            namespace: The namespace.
            tenant: The tenant.

        Returns:
            The recurring action details, or None if not found.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error (other than 404).
        """
        response = self._request(
            "GET",
            f"/v1/recurring/{recurring_id}",
            params={"namespace": namespace, "tenant": tenant},
        )

        if response.status_code == 200:
            return RecurringDetail.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get recurring action")

    def update_recurring(
        self, recurring_id: str, update: UpdateRecurringAction
    ) -> RecurringDetail:
        """Update a recurring action.

        Args:
            recurring_id: The recurring action ID.
            update: The update request with fields to change.

        Returns:
            The updated recurring action details.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the recurring action is not found (404).
            ApiError: If the server returns a validation error.
        """
        response = self._request(
            "PUT", f"/v1/recurring/{recurring_id}", json=update.to_dict()
        )

        if response.status_code == 200:
            return RecurringDetail.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Recurring action not found: {recurring_id}")
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    def delete_recurring(
        self, recurring_id: str, namespace: str, tenant: str
    ) -> None:
        """Delete a recurring action.

        Args:
            recurring_id: The recurring action ID.
            namespace: The namespace.
            tenant: The tenant.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the recurring action is not found (404).
        """
        response = self._request(
            "DELETE",
            f"/v1/recurring/{recurring_id}",
            params={"namespace": namespace, "tenant": tenant},
        )

        if response.status_code == 204:
            return
        elif response.status_code == 404:
            raise HttpError(404, f"Recurring action not found: {recurring_id}")
        else:
            raise HttpError(response.status_code, "Failed to delete recurring action")

    def pause_recurring(
        self, recurring_id: str, namespace: str, tenant: str
    ) -> RecurringDetail:
        """Pause a recurring action.

        Args:
            recurring_id: The recurring action ID.
            namespace: The namespace.
            tenant: The tenant.

        Returns:
            The updated recurring action details.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If not found (404) or already paused (409).
        """
        response = self._request(
            "POST",
            f"/v1/recurring/{recurring_id}/pause",
            json={"namespace": namespace, "tenant": tenant},
        )

        if response.status_code == 200:
            return RecurringDetail.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Recurring action not found: {recurring_id}")
        elif response.status_code == 409:
            raise HttpError(409, "Recurring action is already paused")
        else:
            raise HttpError(response.status_code, "Failed to pause recurring action")

    def resume_recurring(
        self, recurring_id: str, namespace: str, tenant: str
    ) -> RecurringDetail:
        """Resume a paused recurring action.

        Args:
            recurring_id: The recurring action ID.
            namespace: The namespace.
            tenant: The tenant.

        Returns:
            The updated recurring action details.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If not found (404) or already active (409).
        """
        response = self._request(
            "POST",
            f"/v1/recurring/{recurring_id}/resume",
            json={"namespace": namespace, "tenant": tenant},
        )

        if response.status_code == 200:
            return RecurringDetail.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Recurring action not found: {recurring_id}")
        elif response.status_code == 409:
            raise HttpError(409, "Recurring action is already active")
        else:
            raise HttpError(response.status_code, "Failed to resume recurring action")

    # =========================================================================
    # Quotas
    # =========================================================================

    def create_quota(self, req: "CreateQuotaRequest") -> "QuotaPolicy":
        """Create a quota policy.

        Args:
            req: The quota policy definition.

        Returns:
            The created quota policy.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns a validation error.
        """
        response = self._request("POST", "/v1/quotas", json=req.to_dict())

        if response.status_code == 201:
            return QuotaPolicy.from_dict(response.json())
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    def list_quotas(
        self,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
    ) -> "ListQuotasResponse":
        """List quota policies.

        Args:
            namespace: Optional namespace filter.
            tenant: Optional tenant filter.

        Returns:
            List of quota policies.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        params: dict = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        response = self._request("GET", "/v1/quotas", params=params)

        if response.status_code == 200:
            return ListQuotasResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list quotas")

    def get_quota(self, quota_id: str) -> Optional["QuotaPolicy"]:
        """Get a single quota policy by ID.

        Args:
            quota_id: The quota policy ID.

        Returns:
            The quota policy, or None if not found.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error (other than 404).
        """
        response = self._request("GET", f"/v1/quotas/{quota_id}")

        if response.status_code == 200:
            return QuotaPolicy.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get quota")

    def update_quota(
        self, quota_id: str, update: "UpdateQuotaRequest"
    ) -> "QuotaPolicy":
        """Update a quota policy.

        Args:
            quota_id: The quota policy ID.
            update: The update request with fields to change.

        Returns:
            The updated quota policy.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the quota is not found (404).
            ApiError: If the server returns a validation error.
        """
        response = self._request(
            "PUT", f"/v1/quotas/{quota_id}", json=update.to_dict()
        )

        if response.status_code == 200:
            return QuotaPolicy.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Quota not found: {quota_id}")
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    def delete_quota(
        self, quota_id: str, namespace: str, tenant: str
    ) -> None:
        """Delete a quota policy.

        Args:
            quota_id: The quota policy ID.
            namespace: The namespace.
            tenant: The tenant.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the quota is not found (404).
        """
        response = self._request(
            "DELETE",
            f"/v1/quotas/{quota_id}",
            params={"namespace": namespace, "tenant": tenant},
        )

        if response.status_code == 204:
            return
        elif response.status_code == 404:
            raise HttpError(404, f"Quota not found: {quota_id}")
        else:
            raise HttpError(response.status_code, "Failed to delete quota")

    def get_quota_usage(self, quota_id: str) -> "QuotaUsage":
        """Get current usage statistics for a quota policy.

        Args:
            quota_id: The quota policy ID.

        Returns:
            The current usage statistics.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the quota is not found (404).
        """
        response = self._request("GET", f"/v1/quotas/{quota_id}/usage")

        if response.status_code == 200:
            return QuotaUsage.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Quota not found: {quota_id}")
        else:
            raise HttpError(response.status_code, "Failed to get quota usage")

    # =========================================================================
    # Chains
    # =========================================================================

    def list_chains(
        self, namespace: str, tenant: str, *, status: Optional[str] = None
    ) -> ListChainsResponse:
        """List chain executions filtered by namespace, tenant, and optional status.

        Args:
            namespace: The namespace to filter by.
            tenant: The tenant to filter by.
            status: Optional status filter (running, completed, failed, cancelled, timed_out).

        Returns:
            List of chain execution summaries.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        params: dict = {"namespace": namespace, "tenant": tenant}
        if status is not None:
            params["status"] = status
        response = self._request("GET", "/v1/chains", params=params)

        if response.status_code == 200:
            return ListChainsResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list chains")

    def get_chain(
        self, chain_id: str, namespace: str, tenant: str
    ) -> Optional[ChainDetailResponse]:
        """Get full details of a chain execution.

        Args:
            chain_id: The chain execution ID.
            namespace: The namespace.
            tenant: The tenant.

        Returns:
            The chain detail response, or None if not found.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error (other than 404).
        """
        response = self._request(
            "GET",
            f"/v1/chains/{chain_id}",
            params={"namespace": namespace, "tenant": tenant},
        )

        if response.status_code == 200:
            return ChainDetailResponse.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get chain")

    def cancel_chain(
        self,
        chain_id: str,
        namespace: str,
        tenant: str,
        *,
        reason: Optional[str] = None,
        cancelled_by: Optional[str] = None,
    ) -> ChainDetailResponse:
        """Cancel a running chain execution.

        Args:
            chain_id: The chain execution ID.
            namespace: The namespace.
            tenant: The tenant.
            reason: Optional reason for cancellation.
            cancelled_by: Optional identifier of who cancelled the chain.

        Returns:
            The updated chain detail response.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the chain is not found (404) or already finished (409).
        """
        body: dict = {"namespace": namespace, "tenant": tenant}
        if reason is not None:
            body["reason"] = reason
        if cancelled_by is not None:
            body["cancelled_by"] = cancelled_by

        response = self._request(
            "POST", f"/v1/chains/{chain_id}/cancel", json=body
        )

        if response.status_code == 200:
            return ChainDetailResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Chain not found: {chain_id}")
        elif response.status_code == 409:
            raise HttpError(409, "Chain is not running")
        else:
            raise HttpError(response.status_code, "Failed to cancel chain")

    # =========================================================================
    # DLQ (Dead-Letter Queue)
    # =========================================================================

    def dlq_stats(self) -> DlqStatsResponse:
        """Get dead-letter queue statistics.

        Returns:
            DLQ statistics including enabled status and entry count.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        response = self._request("GET", "/v1/dlq/stats")

        if response.status_code == 200:
            return DlqStatsResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to get DLQ stats")

    def dlq_drain(self) -> DlqDrainResponse:
        """Drain all entries from the dead-letter queue.

        Removes and returns all entries from the DLQ for manual processing
        or resubmission.

        Returns:
            The drained entries and count.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the DLQ is not enabled (404) or the server returns an error.
        """
        response = self._request("POST", "/v1/dlq/drain")

        if response.status_code == 200:
            return DlqDrainResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, "Dead-letter queue is not enabled")
        else:
            raise HttpError(response.status_code, "Failed to drain DLQ")

    # =========================================================================
    # Subscribe (SSE)
    # =========================================================================

    def subscribe(
        self,
        entity_type: str,
        entity_id: str,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
        include_history: bool = True,
    ) -> Iterator[SseEvent]:
        """Subscribe to events for a specific entity via SSE.

        Opens a streaming connection to ``GET /v1/subscribe/{entity_type}/{entity_id}``
        and yields parsed SSE events as they arrive.

        Args:
            entity_type: One of "chain", "group", or "action".
            entity_id: The entity identifier to subscribe to.
            namespace: Namespace for tenant isolation (required for chain/group).
            tenant: Tenant for tenant isolation (required for chain/group).
            include_history: Emit catch-up events for current state (default: True).

        Yields:
            Parsed SseEvent objects.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        params: dict = {"include_history": str(include_history).lower()}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant

        url = f"{self.base_url}/v1/subscribe/{entity_type}/{entity_id}"
        headers = self._headers()
        headers["Accept"] = "text/event-stream"
        # Remove Content-Type for GET streaming requests.
        headers.pop("Content-Type", None)

        try:
            with self._client.stream(
                "GET", url, params=params, headers=headers
            ) as response:
                if response.status_code != 200:
                    response.read()
                    raise HttpError(response.status_code, "Failed to subscribe")
                yield from _parse_sse_stream(response.iter_lines())
        except httpx.ConnectError as e:
            raise ConnectionError(str(e)) from e
        except httpx.TimeoutException as e:
            raise ConnectionError(f"Request timed out: {e}") from e

    # =========================================================================
    # Stream (SSE)
    # =========================================================================

    def stream(
        self,
        *,
        namespace: Optional[str] = None,
        action_type: Optional[str] = None,
        outcome: Optional[str] = None,
        event_type: Optional[str] = None,
        chain_id: Optional[str] = None,
        group_id: Optional[str] = None,
        action_id: Optional[str] = None,
        last_event_id: Optional[str] = None,
    ) -> Iterator[SseEvent]:
        """Subscribe to the real-time event stream via SSE.

        Opens a streaming connection to ``GET /v1/stream`` and yields parsed
        SSE events as they arrive. All parameters are optional filters.

        Args:
            namespace: Filter events by namespace.
            action_type: Filter events by action type.
            outcome: Filter events by outcome category (e.g., executed, suppressed, failed).
            event_type: Filter events by stream event type (e.g., action_dispatched).
            chain_id: Filter events by chain ID.
            group_id: Filter events by group ID.
            action_id: Filter events by action ID.
            last_event_id: Reconnection token; replays missed events from this ID.

        Yields:
            Parsed SseEvent objects.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        params: dict = {}
        if namespace is not None:
            params["namespace"] = namespace
        if action_type is not None:
            params["action_type"] = action_type
        if outcome is not None:
            params["outcome"] = outcome
        if event_type is not None:
            params["event_type"] = event_type
        if chain_id is not None:
            params["chain_id"] = chain_id
        if group_id is not None:
            params["group_id"] = group_id
        if action_id is not None:
            params["action_id"] = action_id

        url = f"{self.base_url}/v1/stream"
        headers = self._headers()
        headers["Accept"] = "text/event-stream"
        # Remove Content-Type for GET streaming requests.
        headers.pop("Content-Type", None)
        if last_event_id is not None:
            headers["Last-Event-ID"] = last_event_id

        try:
            with self._client.stream(
                "GET", url, params=params, headers=headers
            ) as response:
                if response.status_code != 200:
                    response.read()
                    raise HttpError(response.status_code, "Failed to open stream")
                yield from _parse_sse_stream(response.iter_lines())
        except httpx.ConnectError as e:
            raise ConnectionError(str(e)) from e
        except httpx.TimeoutException as e:
            raise ConnectionError(f"Request timed out: {e}") from e


class AsyncActeonClient:
    """Async HTTP client for the Acteon action gateway.

    Example:
        >>> async with AsyncActeonClient("http://localhost:8080") as client:
        ...     if await client.health():
        ...         action = Action(...)
        ...         outcome = await client.dispatch(action)
    """

    def __init__(
        self,
        base_url: str,
        *,
        timeout: float = 30.0,
        api_key: Optional[str] = None,
    ):
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self._client = httpx.AsyncClient(timeout=timeout)

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        await self.close()

    async def close(self):
        await self._client.aclose()

    def _headers(self) -> dict[str, str]:
        headers = {"Content-Type": "application/json"}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        return headers

    async def _request(
        self,
        method: str,
        path: str,
        *,
        json: Optional[dict] = None,
        params: Optional[dict] = None,
    ) -> httpx.Response:
        url = f"{self.base_url}{path}"
        try:
            response = await self._client.request(
                method,
                url,
                json=json,
                params=params,
                headers=self._headers(),
            )
            return response
        except httpx.ConnectError as e:
            raise ConnectionError(str(e)) from e
        except httpx.TimeoutException as e:
            raise ConnectionError(f"Request timed out: {e}") from e

    async def health(self) -> bool:
        try:
            response = await self._request("GET", "/health")
            return response.status_code == 200
        except ConnectionError:
            return False

    async def dispatch(
        self, action: Action, *, dry_run: bool = False
    ) -> ActionOutcome:
        params = {"dry_run": "true"} if dry_run else None
        response = await self._request(
            "POST", "/v1/dispatch", json=action.to_dict(), params=params
        )
        if response.status_code == 200:
            return ActionOutcome.from_dict(response.json())
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    async def dispatch_dry_run(self, action: Action) -> ActionOutcome:
        return await self.dispatch(action, dry_run=True)

    async def dispatch_batch(
        self, actions: list[Action], *, dry_run: bool = False
    ) -> list[BatchResult]:
        params = {"dry_run": "true"} if dry_run else None
        response = await self._request(
            "POST",
            "/v1/dispatch/batch",
            json=[a.to_dict() for a in actions],
            params=params,
        )
        if response.status_code == 200:
            return [BatchResult.from_dict(r) for r in response.json()]
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    async def dispatch_batch_dry_run(
        self, actions: list[Action]
    ) -> list[BatchResult]:
        return await self.dispatch_batch(actions, dry_run=True)

    async def list_rules(self) -> list[RuleInfo]:
        response = await self._request("GET", "/v1/rules")
        if response.status_code == 200:
            return [RuleInfo.from_dict(r) for r in response.json()]
        else:
            raise HttpError(response.status_code, f"Failed to list rules")

    async def reload_rules(self) -> ReloadResult:
        response = await self._request("POST", "/v1/rules/reload")
        if response.status_code == 200:
            return ReloadResult.from_dict(response.json())
        else:
            raise HttpError(response.status_code, f"Failed to reload rules")

    async def set_rule_enabled(self, rule_name: str, enabled: bool) -> None:
        response = await self._request(
            "PUT",
            f"/v1/rules/{rule_name}/enabled",
            json={"enabled": enabled},
        )
        if response.status_code != 200:
            raise HttpError(response.status_code, f"Failed to set rule enabled")

    async def evaluate_rules(
        self, request: EvaluateRulesRequest
    ) -> EvaluateRulesResponse:
        """Evaluate rules against a test action without dispatching."""
        body: dict = {
            "namespace": request.namespace,
            "tenant": request.tenant,
            "provider": request.provider,
            "action_type": request.action_type,
            "payload": request.payload,
        }
        if request.metadata:
            body["metadata"] = request.metadata
        if request.include_disabled:
            body["include_disabled"] = True
        if request.evaluate_all:
            body["evaluate_all"] = True
        if request.evaluate_at:
            body["evaluate_at"] = request.evaluate_at
        if request.mock_state:
            body["mock_state"] = request.mock_state

        response = await self._request("POST", "/v1/rules/evaluate", json=body)
        if response.status_code == 200:
            return EvaluateRulesResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to evaluate rules")

    async def query_audit(self, query: Optional[AuditQuery] = None) -> AuditPage:
        params = query.to_params() if query else {}
        response = await self._request("GET", "/v1/audit", params=params)
        if response.status_code == 200:
            return AuditPage.from_dict(response.json())
        else:
            raise HttpError(response.status_code, f"Failed to query audit")

    async def get_audit_record(self, action_id: str) -> Optional[AuditRecord]:
        response = await self._request("GET", f"/v1/audit/{action_id}")
        if response.status_code == 200:
            return AuditRecord.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, f"Failed to get audit record")

    # =========================================================================
    # Audit Replay
    # =========================================================================

    async def replay_action(self, action_id: str) -> ReplayResult:
        """Replay a single action from the audit trail."""
        response = await self._request("POST", f"/v1/audit/{action_id}/replay")
        if response.status_code == 200:
            return ReplayResult.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Audit record not found: {action_id}")
        elif response.status_code == 422:
            raise HttpError(422, "No stored payload available for replay")
        else:
            raise HttpError(response.status_code, "Failed to replay action")

    async def replay_audit(self, query: Optional[ReplayQuery] = None) -> ReplaySummary:
        """Bulk replay actions from the audit trail."""
        params = query.to_params() if query else {}
        response = await self._request("POST", "/v1/audit/replay", params=params)
        if response.status_code == 200:
            return ReplaySummary.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to replay audit")

    # =========================================================================
    # Events (State Machine Lifecycle)
    # =========================================================================

    async def list_events(self, query: EventQuery) -> EventListResponse:
        response = await self._request("GET", "/v1/events", params=query.to_params())
        if response.status_code == 200:
            return EventListResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list events")

    async def get_event(
        self, fingerprint: str, namespace: str, tenant: str
    ) -> Optional[EventState]:
        response = await self._request(
            "GET",
            f"/v1/events/{fingerprint}",
            params={"namespace": namespace, "tenant": tenant},
        )
        if response.status_code == 200:
            return EventState.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get event")

    async def transition_event(
        self, fingerprint: str, to_state: str, namespace: str, tenant: str
    ) -> TransitionResponse:
        response = await self._request(
            "PUT",
            f"/v1/events/{fingerprint}/transition",
            json={"to": to_state, "namespace": namespace, "tenant": tenant},
        )
        if response.status_code == 200:
            return TransitionResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Event not found: {fingerprint}")
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    # =========================================================================
    # Groups (Event Batching)
    # =========================================================================

    async def list_groups(self) -> GroupListResponse:
        response = await self._request("GET", "/v1/groups")
        if response.status_code == 200:
            return GroupListResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list groups")

    async def get_group(self, group_key: str) -> Optional[GroupDetail]:
        response = await self._request("GET", f"/v1/groups/{group_key}")
        if response.status_code == 200:
            return GroupDetail.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get group")

    async def flush_group(self, group_key: str) -> FlushGroupResponse:
        response = await self._request("DELETE", f"/v1/groups/{group_key}")
        if response.status_code == 200:
            return FlushGroupResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Group not found: {group_key}")
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    # =========================================================================
    # Approvals (Human-in-the-Loop)
    # =========================================================================

    async def approve(self, namespace: str, tenant: str, id: str, sig: str, expires_at: int, kid: Optional[str] = None) -> ApprovalActionResponse:
        params: dict = {"sig": sig, "expires_at": expires_at}
        if kid is not None:
            params["kid"] = kid
        response = await self._request(
            "POST",
            f"/v1/approvals/{namespace}/{tenant}/{id}/approve",
            params=params,
        )
        if response.status_code == 200:
            return ApprovalActionResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, "Approval not found or expired")
        elif response.status_code == 410:
            raise HttpError(410, "Approval already decided")
        else:
            raise HttpError(response.status_code, "Failed to approve")

    async def reject(self, namespace: str, tenant: str, id: str, sig: str, expires_at: int, kid: Optional[str] = None) -> ApprovalActionResponse:
        params: dict = {"sig": sig, "expires_at": expires_at}
        if kid is not None:
            params["kid"] = kid
        response = await self._request(
            "POST",
            f"/v1/approvals/{namespace}/{tenant}/{id}/reject",
            params=params,
        )
        if response.status_code == 200:
            return ApprovalActionResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, "Approval not found or expired")
        elif response.status_code == 410:
            raise HttpError(410, "Approval already decided")
        else:
            raise HttpError(response.status_code, "Failed to reject")

    async def get_approval(self, namespace: str, tenant: str, id: str, sig: str, expires_at: int, kid: Optional[str] = None) -> Optional[ApprovalStatus]:
        params: dict = {"sig": sig, "expires_at": expires_at}
        if kid is not None:
            params["kid"] = kid
        response = await self._request(
            "GET",
            f"/v1/approvals/{namespace}/{tenant}/{id}",
            params=params,
        )
        if response.status_code == 200:
            return ApprovalStatus.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get approval")

    async def list_approvals(
        self, namespace: str, tenant: str
    ) -> ApprovalListResponse:
        response = await self._request(
            "GET",
            "/v1/approvals",
            params={"namespace": namespace, "tenant": tenant},
        )
        if response.status_code == 200:
            return ApprovalListResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list approvals")

    # =========================================================================
    # Recurring Actions
    # =========================================================================

    async def create_recurring(
        self, recurring: CreateRecurringAction
    ) -> CreateRecurringResponse:
        """Create a recurring action."""
        response = await self._request(
            "POST", "/v1/recurring", json=recurring.to_dict()
        )
        if response.status_code == 201:
            return CreateRecurringResponse.from_dict(response.json())
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    async def list_recurring(
        self, filter: Optional[RecurringFilter] = None
    ) -> ListRecurringResponse:
        """List recurring actions."""
        params = filter.to_params() if filter else {}
        response = await self._request("GET", "/v1/recurring", params=params)
        if response.status_code == 200:
            return ListRecurringResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list recurring actions")

    async def get_recurring(
        self, recurring_id: str, namespace: str, tenant: str
    ) -> Optional[RecurringDetail]:
        """Get details of a specific recurring action."""
        response = await self._request(
            "GET",
            f"/v1/recurring/{recurring_id}",
            params={"namespace": namespace, "tenant": tenant},
        )
        if response.status_code == 200:
            return RecurringDetail.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get recurring action")

    async def update_recurring(
        self, recurring_id: str, update: UpdateRecurringAction
    ) -> RecurringDetail:
        """Update a recurring action."""
        response = await self._request(
            "PUT", f"/v1/recurring/{recurring_id}", json=update.to_dict()
        )
        if response.status_code == 200:
            return RecurringDetail.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Recurring action not found: {recurring_id}")
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    async def delete_recurring(
        self, recurring_id: str, namespace: str, tenant: str
    ) -> None:
        """Delete a recurring action."""
        response = await self._request(
            "DELETE",
            f"/v1/recurring/{recurring_id}",
            params={"namespace": namespace, "tenant": tenant},
        )
        if response.status_code == 204:
            return
        elif response.status_code == 404:
            raise HttpError(404, f"Recurring action not found: {recurring_id}")
        else:
            raise HttpError(response.status_code, "Failed to delete recurring action")

    async def pause_recurring(
        self, recurring_id: str, namespace: str, tenant: str
    ) -> RecurringDetail:
        """Pause a recurring action."""
        response = await self._request(
            "POST",
            f"/v1/recurring/{recurring_id}/pause",
            json={"namespace": namespace, "tenant": tenant},
        )
        if response.status_code == 200:
            return RecurringDetail.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Recurring action not found: {recurring_id}")
        elif response.status_code == 409:
            raise HttpError(409, "Recurring action is already paused")
        else:
            raise HttpError(response.status_code, "Failed to pause recurring action")

    async def resume_recurring(
        self, recurring_id: str, namespace: str, tenant: str
    ) -> RecurringDetail:
        """Resume a paused recurring action."""
        response = await self._request(
            "POST",
            f"/v1/recurring/{recurring_id}/resume",
            json={"namespace": namespace, "tenant": tenant},
        )
        if response.status_code == 200:
            return RecurringDetail.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Recurring action not found: {recurring_id}")
        elif response.status_code == 409:
            raise HttpError(409, "Recurring action is already active")
        else:
            raise HttpError(response.status_code, "Failed to resume recurring action")

    # =========================================================================
    # Quotas
    # =========================================================================

    async def create_quota(self, req: "CreateQuotaRequest") -> "QuotaPolicy":
        """Create a quota policy."""
        response = await self._request("POST", "/v1/quotas", json=req.to_dict())
        if response.status_code == 201:
            return QuotaPolicy.from_dict(response.json())
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    async def list_quotas(
        self,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
    ) -> "ListQuotasResponse":
        """List quota policies."""
        params: dict = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        response = await self._request("GET", "/v1/quotas", params=params)
        if response.status_code == 200:
            return ListQuotasResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list quotas")

    async def get_quota(self, quota_id: str) -> Optional["QuotaPolicy"]:
        """Get a single quota policy by ID."""
        response = await self._request("GET", f"/v1/quotas/{quota_id}")
        if response.status_code == 200:
            return QuotaPolicy.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get quota")

    async def update_quota(
        self, quota_id: str, update: "UpdateQuotaRequest"
    ) -> "QuotaPolicy":
        """Update a quota policy."""
        response = await self._request(
            "PUT", f"/v1/quotas/{quota_id}", json=update.to_dict()
        )
        if response.status_code == 200:
            return QuotaPolicy.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Quota not found: {quota_id}")
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    async def delete_quota(
        self, quota_id: str, namespace: str, tenant: str
    ) -> None:
        """Delete a quota policy."""
        response = await self._request(
            "DELETE",
            f"/v1/quotas/{quota_id}",
            params={"namespace": namespace, "tenant": tenant},
        )
        if response.status_code == 204:
            return
        elif response.status_code == 404:
            raise HttpError(404, f"Quota not found: {quota_id}")
        else:
            raise HttpError(response.status_code, "Failed to delete quota")

    async def get_quota_usage(self, quota_id: str) -> "QuotaUsage":
        """Get current usage statistics for a quota policy."""
        response = await self._request("GET", f"/v1/quotas/{quota_id}/usage")
        if response.status_code == 200:
            return QuotaUsage.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Quota not found: {quota_id}")
        else:
            raise HttpError(response.status_code, "Failed to get quota usage")

    # =========================================================================
    # Chains
    # =========================================================================

    async def list_chains(
        self, namespace: str, tenant: str, *, status: Optional[str] = None
    ) -> ListChainsResponse:
        """List chain executions filtered by namespace, tenant, and optional status."""
        params: dict = {"namespace": namespace, "tenant": tenant}
        if status is not None:
            params["status"] = status
        response = await self._request("GET", "/v1/chains", params=params)
        if response.status_code == 200:
            return ListChainsResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to list chains")

    async def get_chain(
        self, chain_id: str, namespace: str, tenant: str
    ) -> Optional[ChainDetailResponse]:
        """Get full details of a chain execution."""
        response = await self._request(
            "GET",
            f"/v1/chains/{chain_id}",
            params={"namespace": namespace, "tenant": tenant},
        )
        if response.status_code == 200:
            return ChainDetailResponse.from_dict(response.json())
        elif response.status_code == 404:
            return None
        else:
            raise HttpError(response.status_code, "Failed to get chain")

    async def cancel_chain(
        self,
        chain_id: str,
        namespace: str,
        tenant: str,
        *,
        reason: Optional[str] = None,
        cancelled_by: Optional[str] = None,
    ) -> ChainDetailResponse:
        """Cancel a running chain execution."""
        body: dict = {"namespace": namespace, "tenant": tenant}
        if reason is not None:
            body["reason"] = reason
        if cancelled_by is not None:
            body["cancelled_by"] = cancelled_by
        response = await self._request(
            "POST", f"/v1/chains/{chain_id}/cancel", json=body
        )
        if response.status_code == 200:
            return ChainDetailResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, f"Chain not found: {chain_id}")
        elif response.status_code == 409:
            raise HttpError(409, "Chain is not running")
        else:
            raise HttpError(response.status_code, "Failed to cancel chain")

    # =========================================================================
    # DLQ (Dead-Letter Queue)
    # =========================================================================

    async def dlq_stats(self) -> DlqStatsResponse:
        """Get dead-letter queue statistics."""
        response = await self._request("GET", "/v1/dlq/stats")
        if response.status_code == 200:
            return DlqStatsResponse.from_dict(response.json())
        else:
            raise HttpError(response.status_code, "Failed to get DLQ stats")

    async def dlq_drain(self) -> DlqDrainResponse:
        """Drain all entries from the dead-letter queue."""
        response = await self._request("POST", "/v1/dlq/drain")
        if response.status_code == 200:
            return DlqDrainResponse.from_dict(response.json())
        elif response.status_code == 404:
            raise HttpError(404, "Dead-letter queue is not enabled")
        else:
            raise HttpError(response.status_code, "Failed to drain DLQ")

    # =========================================================================
    # Subscribe (SSE)
    # =========================================================================

    async def subscribe(
        self,
        entity_type: str,
        entity_id: str,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
        include_history: bool = True,
    ) -> AsyncIterator[SseEvent]:
        """Subscribe to events for a specific entity via SSE.

        Opens a streaming connection to ``GET /v1/subscribe/{entity_type}/{entity_id}``
        and yields parsed SSE events as they arrive.

        Args:
            entity_type: One of "chain", "group", or "action".
            entity_id: The entity identifier to subscribe to.
            namespace: Namespace for tenant isolation (required for chain/group).
            tenant: Tenant for tenant isolation (required for chain/group).
            include_history: Emit catch-up events for current state (default: True).

        Yields:
            Parsed SseEvent objects.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        params: dict = {"include_history": str(include_history).lower()}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant

        url = f"{self.base_url}/v1/subscribe/{entity_type}/{entity_id}"
        headers = self._headers()
        headers["Accept"] = "text/event-stream"
        headers.pop("Content-Type", None)

        try:
            async with self._client.stream(
                "GET", url, params=params, headers=headers
            ) as response:
                if response.status_code != 200:
                    await response.aread()
                    raise HttpError(response.status_code, "Failed to subscribe")
                async for event in _async_parse_sse_stream(response.aiter_lines()):
                    yield event
        except httpx.ConnectError as e:
            raise ConnectionError(str(e)) from e
        except httpx.TimeoutException as e:
            raise ConnectionError(f"Request timed out: {e}") from e

    # =========================================================================
    # Stream (SSE)
    # =========================================================================

    async def stream(
        self,
        *,
        namespace: Optional[str] = None,
        action_type: Optional[str] = None,
        outcome: Optional[str] = None,
        event_type: Optional[str] = None,
        chain_id: Optional[str] = None,
        group_id: Optional[str] = None,
        action_id: Optional[str] = None,
        last_event_id: Optional[str] = None,
    ) -> AsyncIterator[SseEvent]:
        """Subscribe to the real-time event stream via SSE.

        Opens a streaming connection to ``GET /v1/stream`` and yields parsed
        SSE events as they arrive. All parameters are optional filters.

        Args:
            namespace: Filter events by namespace.
            action_type: Filter events by action type.
            outcome: Filter events by outcome category.
            event_type: Filter events by stream event type.
            chain_id: Filter events by chain ID.
            group_id: Filter events by group ID.
            action_id: Filter events by action ID.
            last_event_id: Reconnection token; replays missed events from this ID.

        Yields:
            Parsed SseEvent objects.

        Raises:
            ConnectionError: If unable to connect to the server.
            HttpError: If the server returns an error.
        """
        params: dict = {}
        if namespace is not None:
            params["namespace"] = namespace
        if action_type is not None:
            params["action_type"] = action_type
        if outcome is not None:
            params["outcome"] = outcome
        if event_type is not None:
            params["event_type"] = event_type
        if chain_id is not None:
            params["chain_id"] = chain_id
        if group_id is not None:
            params["group_id"] = group_id
        if action_id is not None:
            params["action_id"] = action_id

        url = f"{self.base_url}/v1/stream"
        headers = self._headers()
        headers["Accept"] = "text/event-stream"
        headers.pop("Content-Type", None)
        if last_event_id is not None:
            headers["Last-Event-ID"] = last_event_id

        try:
            async with self._client.stream(
                "GET", url, params=params, headers=headers
            ) as response:
                if response.status_code != 200:
                    await response.aread()
                    raise HttpError(response.status_code, "Failed to open stream")
                async for event in _async_parse_sse_stream(response.aiter_lines()):
                    yield event
        except httpx.ConnectError as e:
            raise ConnectionError(str(e)) from e
        except httpx.TimeoutException as e:
            raise ConnectionError(f"Request timed out: {e}") from e


async def _async_parse_sse_stream(aiter_lines) -> AsyncIterator[SseEvent]:
    """Parse a text/event-stream from an async line iterator into SseEvent objects.

    This is the async equivalent of ``_parse_sse_stream``.

    Args:
        aiter_lines: An async iterator of lines from the SSE stream.

    Yields:
        Parsed SseEvent objects.
    """
    import json as _json

    event_type: Optional[str] = None
    event_id: Optional[str] = None
    data_parts: list[str] = []

    async for line in aiter_lines:
        if line.startswith(":"):
            continue
        if line == "":
            if data_parts:
                raw_data = "\n".join(data_parts)
                try:
                    parsed = _json.loads(raw_data)
                except (_json.JSONDecodeError, ValueError):
                    parsed = raw_data
                yield SseEvent(event=event_type, id=event_id, data=parsed)
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
