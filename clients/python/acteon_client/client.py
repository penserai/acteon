"""HTTP client for the Acteon action gateway."""

from typing import Optional
import httpx

from .errors import ActeonError, ConnectionError, HttpError, ApiError
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

    def dispatch(self, action: Action) -> ActionOutcome:
        """Dispatch a single action.

        Args:
            action: The action to dispatch.

        Returns:
            The outcome of the action.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        response = self._request("POST", "/v1/dispatch", json=action.to_dict())

        if response.status_code == 200:
            return ActionOutcome.from_dict(response.json())
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    def dispatch_batch(self, actions: list[Action]) -> list[BatchResult]:
        """Dispatch multiple actions in a single request.

        Args:
            actions: List of actions to dispatch.

        Returns:
            List of results, one per action.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns a batch-level error.
        """
        response = self._request(
            "POST",
            "/v1/dispatch/batch",
            json=[a.to_dict() for a in actions],
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

    async def dispatch(self, action: Action) -> ActionOutcome:
        response = await self._request("POST", "/v1/dispatch", json=action.to_dict())
        if response.status_code == 200:
            return ActionOutcome.from_dict(response.json())
        else:
            data = response.json()
            raise ApiError(
                code=data.get("code", "UNKNOWN"),
                message=data.get("message", "Unknown error"),
                retryable=data.get("retryable", False),
            )

    async def dispatch_batch(self, actions: list[Action]) -> list[BatchResult]:
        response = await self._request(
            "POST",
            "/v1/dispatch/batch",
            json=[a.to_dict() for a in actions],
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
