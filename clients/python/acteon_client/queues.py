"""Task-queue surface for the Python ActeonClient.

Two mixins — ``_QueuesClientMixin`` for sync and
``_AsyncQueuesClientMixin`` for async — bolt the durable task-queue
REST surface onto :class:`~acteon_client.client.ActeonClient` and
:class:`~acteon_client.client.AsyncActeonClient`, following the same
shape as ``bus.py`` / ``a2a.py``.

The queue model is lease-based: a worker polls a queue, receives
tasks with a ``lease_token``, and must settle each task (complete or
fail) — or keep the lease alive via heartbeat — before
``lease_expires_at``. :class:`~acteon_client.worker.Worker` wraps
this loop; the raw methods here are for callers who want direct
control.

The mixins assume the host class exposes ``self._request(method,
path, json=, params=)`` returning an ``httpx.Response``. The base
clients already provide this.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Optional
from urllib.parse import quote

from .errors import ApiError, HttpError

if TYPE_CHECKING:
    import httpx


def _seg(s: str) -> str:
    """Percent-encode a path segment opaquely (no ``/`` passthrough).

    Mirrors ``_seg`` in ``bus.py``: a queue name or task id that
    happens to contain a slash must be escaped, not silently split
    into additional path components.
    """
    return quote(s, safe="")


def _raise_for_status(resp: "httpx.Response") -> None:
    """Translate a non-2xx response into either ``ApiError`` (with the
    server's structured error envelope) or ``HttpError`` (raw body).
    """
    if 200 <= resp.status_code < 300:
        return
    try:
        data = resp.json()
        message = (
            data.get("error")
            or data.get("message")
            or f"queue error (status {resp.status_code})"
        )
        raise ApiError(
            code=data.get("code", "QUEUE"),
            message=message,
            retryable=data.get("retryable", resp.status_code >= 500),
        )
    except ValueError:
        # Not JSON — fall back to a raw HTTP error with the body text.
        raise HttpError(resp.status_code, resp.text or "queue error")


# ============================================================================
# Models
# ============================================================================


@dataclass
class WorkerTask:
    """A single durable task on a queue.

    ``status`` is one of ``pending``, ``leased``, ``completed``,
    ``failed``, or ``cancelled``. ``lease_token`` is only present
    while the task is leased to a worker; settling calls (heartbeat,
    complete, fail) must echo it back.
    """

    task_id: str
    queue: str
    action_type: str
    payload: Any
    status: str
    attempt: int
    max_attempts: int
    created_at: str
    updated_at: str
    lease_token: Optional[str] = None
    lease_expires_at: Optional[str] = None
    result: Any = None
    error: Optional[str] = None
    chain_id: Optional[str] = None
    workflow_execution_id: Optional[str] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "WorkerTask":
        return cls(
            task_id=d["task_id"],
            queue=d["queue"],
            action_type=d["action_type"],
            payload=d.get("payload"),
            status=d["status"],
            attempt=d["attempt"],
            max_attempts=d["max_attempts"],
            created_at=d["created_at"],
            updated_at=d["updated_at"],
            lease_token=d.get("lease_token"),
            lease_expires_at=d.get("lease_expires_at"),
            result=d.get("result"),
            error=d.get("error"),
            chain_id=d.get("chain_id"),
            workflow_execution_id=d.get("workflow_execution_id"),
        )


# ============================================================================
# Sync mixin
# ============================================================================


class _QueuesClientMixin:
    """Mixin providing the task-queue REST surface."""

    # The mixin doesn't define its own __init__; these attributes are
    # set by the concrete ``ActeonClient`` it gets mixed into. Stub
    # the types so ``mypy`` (and humans) understand the contract.
    if TYPE_CHECKING:
        def _request(  # noqa: D401
            self,
            method: str,
            path: str,
            *,
            json: Optional[dict] = None,
            params: Optional[dict] = None,
        ) -> "httpx.Response": ...

    def enqueue_task(
        self,
        queue: str,
        namespace: str,
        tenant: str,
        action_type: str,
        payload: Any,
        *,
        max_attempts: Optional[int] = None,
    ) -> WorkerTask:
        """Enqueue a new task on ``queue``.

        Args:
            queue: The queue name.
            namespace: The task namespace.
            tenant: The task tenant.
            action_type: Routing key workers use to pick a handler.
            payload: Arbitrary JSON payload handed to the handler.
            max_attempts: Optional cap on delivery attempts.

        Returns:
            The created task (status ``pending``).

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns a validation error.
        """
        body: dict[str, Any] = {
            "namespace": namespace,
            "tenant": tenant,
            "action_type": action_type,
            "payload": payload,
        }
        if max_attempts is not None:
            body["max_attempts"] = max_attempts
        resp = self._request("POST", f"/v1/queues/{_seg(queue)}/tasks", json=body)
        _raise_for_status(resp)
        return WorkerTask.from_dict(resp.json())

    def poll_tasks(
        self,
        queue: str,
        namespace: str,
        tenant: str,
        *,
        max_tasks: Optional[int] = None,
        lease_seconds: Optional[int] = None,
        worker_id: Optional[str] = None,
    ) -> list[WorkerTask]:
        """Poll ``queue`` for available tasks, leasing each one returned.

        Args:
            queue: The queue name.
            namespace: The task namespace.
            tenant: The task tenant.
            max_tasks: Optional maximum number of tasks to lease.
            lease_seconds: Optional lease duration; the lease must be
                heartbeat-extended or settled before it expires.
            worker_id: Optional stable worker identity for observability.

        Returns:
            The leased tasks (possibly empty).

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        body: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if max_tasks is not None:
            body["max_tasks"] = max_tasks
        if lease_seconds is not None:
            body["lease_seconds"] = lease_seconds
        if worker_id is not None:
            body["worker_id"] = worker_id
        resp = self._request("POST", f"/v1/queues/{_seg(queue)}/poll", json=body)
        _raise_for_status(resp)
        return [WorkerTask.from_dict(t) for t in resp.json().get("tasks", [])]

    def heartbeat_task(
        self,
        task_id: str,
        namespace: str,
        tenant: str,
        lease_token: str,
        *,
        extend_seconds: Optional[int] = None,
    ) -> WorkerTask:
        """Extend the lease on a task that is still being worked.

        Args:
            task_id: The task ID.
            namespace: The task namespace.
            tenant: The task tenant.
            lease_token: The lease token returned by poll.
            extend_seconds: Optional new lease duration from now.

        Returns:
            The updated task.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the lease is no longer valid.
        """
        body: dict[str, Any] = {
            "namespace": namespace,
            "tenant": tenant,
            "lease_token": lease_token,
        }
        if extend_seconds is not None:
            body["extend_seconds"] = extend_seconds
        resp = self._request(
            "POST", f"/v1/queues/tasks/{_seg(task_id)}/heartbeat", json=body
        )
        _raise_for_status(resp)
        return WorkerTask.from_dict(resp.json())

    def complete_task(
        self,
        task_id: str,
        namespace: str,
        tenant: str,
        lease_token: str,
        result: Any,
    ) -> WorkerTask:
        """Settle a leased task as completed.

        Args:
            task_id: The task ID.
            namespace: The task namespace.
            tenant: The task tenant.
            lease_token: The lease token returned by poll.
            result: Arbitrary JSON result stored on the task.

        Returns:
            The completed task.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the lease is no longer valid.
        """
        resp = self._request(
            "POST",
            f"/v1/queues/tasks/{_seg(task_id)}/complete",
            json={
                "namespace": namespace,
                "tenant": tenant,
                "lease_token": lease_token,
                "result": result,
            },
        )
        _raise_for_status(resp)
        return WorkerTask.from_dict(resp.json())

    def fail_task(
        self,
        task_id: str,
        namespace: str,
        tenant: str,
        lease_token: str,
        error: str,
        retryable: bool,
    ) -> WorkerTask:
        """Settle a leased task as failed.

        Args:
            task_id: The task ID.
            namespace: The task namespace.
            tenant: The task tenant.
            lease_token: The lease token returned by poll.
            error: Human-readable failure description.
            retryable: Whether the server should re-deliver the task
                (subject to ``max_attempts``).

        Returns:
            The updated task.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the lease is no longer valid.
        """
        resp = self._request(
            "POST",
            f"/v1/queues/tasks/{_seg(task_id)}/fail",
            json={
                "namespace": namespace,
                "tenant": tenant,
                "lease_token": lease_token,
                "error": error,
                "retryable": retryable,
            },
        )
        _raise_for_status(resp)
        return WorkerTask.from_dict(resp.json())

    def get_task(
        self, task_id: str, namespace: str, tenant: str
    ) -> Optional[WorkerTask]:
        """Get a single task by ID.

        Returns:
            The task, or None if not found.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error (other than 404).
        """
        resp = self._request(
            "GET",
            f"/v1/queues/tasks/{_seg(task_id)}",
            params={"namespace": namespace, "tenant": tenant},
        )
        if resp.status_code == 404:
            return None
        _raise_for_status(resp)
        return WorkerTask.from_dict(resp.json())

    def list_tasks(
        self,
        queue: str,
        namespace: str,
        tenant: str,
        *,
        status: Optional[str] = None,
    ) -> list[WorkerTask]:
        """List tasks on ``queue``, optionally filtered by status.

        Args:
            queue: The queue name.
            namespace: The task namespace.
            tenant: The task tenant.
            status: Optional status filter (``pending``, ``leased``,
                ``completed``, ``failed``, ``cancelled``).

        Returns:
            The matching tasks.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        params: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if status is not None:
            params["status"] = status
        resp = self._request(
            "GET", f"/v1/queues/{_seg(queue)}/tasks", params=params
        )
        _raise_for_status(resp)
        return [WorkerTask.from_dict(t) for t in resp.json().get("tasks", [])]


# ============================================================================
# Async mixin
#
# Mounted onto `AsyncActeonClient`. Mirrors the sync mixin exactly;
# the two share zero implementation for the same reason ``bus.py``
# documents — blocking and non-blocking call sites are syntactically
# distinct in Python.
# ============================================================================


class _AsyncQueuesClientMixin:
    """Async mixin providing the task-queue REST surface."""

    if TYPE_CHECKING:
        async def _request(  # noqa: D401
            self,
            method: str,
            path: str,
            *,
            json: Optional[dict] = None,
            params: Optional[dict] = None,
        ) -> "httpx.Response": ...

    async def enqueue_task(
        self,
        queue: str,
        namespace: str,
        tenant: str,
        action_type: str,
        payload: Any,
        *,
        max_attempts: Optional[int] = None,
    ) -> WorkerTask:
        """Enqueue a new task on ``queue``. See the sync mixin."""
        body: dict[str, Any] = {
            "namespace": namespace,
            "tenant": tenant,
            "action_type": action_type,
            "payload": payload,
        }
        if max_attempts is not None:
            body["max_attempts"] = max_attempts
        resp = await self._request("POST", f"/v1/queues/{_seg(queue)}/tasks", json=body)
        _raise_for_status(resp)
        return WorkerTask.from_dict(resp.json())

    async def poll_tasks(
        self,
        queue: str,
        namespace: str,
        tenant: str,
        *,
        max_tasks: Optional[int] = None,
        lease_seconds: Optional[int] = None,
        worker_id: Optional[str] = None,
    ) -> list[WorkerTask]:
        """Poll ``queue`` for available tasks. See the sync mixin."""
        body: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if max_tasks is not None:
            body["max_tasks"] = max_tasks
        if lease_seconds is not None:
            body["lease_seconds"] = lease_seconds
        if worker_id is not None:
            body["worker_id"] = worker_id
        resp = await self._request("POST", f"/v1/queues/{_seg(queue)}/poll", json=body)
        _raise_for_status(resp)
        return [WorkerTask.from_dict(t) for t in resp.json().get("tasks", [])]

    async def heartbeat_task(
        self,
        task_id: str,
        namespace: str,
        tenant: str,
        lease_token: str,
        *,
        extend_seconds: Optional[int] = None,
    ) -> WorkerTask:
        """Extend the lease on a task. See the sync mixin."""
        body: dict[str, Any] = {
            "namespace": namespace,
            "tenant": tenant,
            "lease_token": lease_token,
        }
        if extend_seconds is not None:
            body["extend_seconds"] = extend_seconds
        resp = await self._request(
            "POST", f"/v1/queues/tasks/{_seg(task_id)}/heartbeat", json=body
        )
        _raise_for_status(resp)
        return WorkerTask.from_dict(resp.json())

    async def complete_task(
        self,
        task_id: str,
        namespace: str,
        tenant: str,
        lease_token: str,
        result: Any,
    ) -> WorkerTask:
        """Settle a leased task as completed. See the sync mixin."""
        resp = await self._request(
            "POST",
            f"/v1/queues/tasks/{_seg(task_id)}/complete",
            json={
                "namespace": namespace,
                "tenant": tenant,
                "lease_token": lease_token,
                "result": result,
            },
        )
        _raise_for_status(resp)
        return WorkerTask.from_dict(resp.json())

    async def fail_task(
        self,
        task_id: str,
        namespace: str,
        tenant: str,
        lease_token: str,
        error: str,
        retryable: bool,
    ) -> WorkerTask:
        """Settle a leased task as failed. See the sync mixin."""
        resp = await self._request(
            "POST",
            f"/v1/queues/tasks/{_seg(task_id)}/fail",
            json={
                "namespace": namespace,
                "tenant": tenant,
                "lease_token": lease_token,
                "error": error,
                "retryable": retryable,
            },
        )
        _raise_for_status(resp)
        return WorkerTask.from_dict(resp.json())

    async def get_task(
        self, task_id: str, namespace: str, tenant: str
    ) -> Optional[WorkerTask]:
        """Get a single task by ID, or None on 404. See the sync mixin."""
        resp = await self._request(
            "GET",
            f"/v1/queues/tasks/{_seg(task_id)}",
            params={"namespace": namespace, "tenant": tenant},
        )
        if resp.status_code == 404:
            return None
        _raise_for_status(resp)
        return WorkerTask.from_dict(resp.json())

    async def list_tasks(
        self,
        queue: str,
        namespace: str,
        tenant: str,
        *,
        status: Optional[str] = None,
    ) -> list[WorkerTask]:
        """List tasks on ``queue``. See the sync mixin."""
        params: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if status is not None:
            params["status"] = status
        resp = await self._request(
            "GET", f"/v1/queues/{_seg(queue)}/tasks", params=params
        )
        _raise_for_status(resp)
        return [WorkerTask.from_dict(t) for t in resp.json().get("tasks", [])]
