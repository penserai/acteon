"""Checkpoint-based workflow surface for the Python ActeonClient.

Two layers live here:

1. ``_WorkflowsClientMixin`` / ``_AsyncWorkflowsClientMixin`` — the
   workflow REST surface (start / get / list / signal / cancel /
   checkpoints / children / history), following the same mixin shape
   as ``bus.py`` and ``queues.py``.
2. :class:`WorkflowContext` — the authoring API handed to workflow
   functions run by :class:`~acteon_client.worker.Worker`.

Execution model (checkpoint-based, NOT replay-determinism)
----------------------------------------------------------

A workflow function ``fn(ctx, input)`` is re-run *from the top* on
every continuation task. The context replays recorded checkpoints by
name: a ``ctx.step(...)`` whose checkpoint already exists returns the
stored data instantly instead of re-executing. When the function
reaches a suspension point (``ctx.sleep``, ``ctx.wait_for_signal``)
with no recorded checkpoint, the context raises the internal
:class:`_Suspend` control-flow exception carrying a *directive*; the
worker settles the continuation task with that directive and the
server resumes the execution later with the checkpoint recorded.

Because the function re-runs from the top, checkpoint names must be
stable across runs. Each context primitive derives its key from a
per-name occurrence counter within the current run (``step:{name}#{k}``,
``sleep#{k}``, ``signal:{name}#{k}``, ``child:{workflow}#{k}`` with
``k`` starting at 0), so straight-line code — including loops — gets
the same keys on every re-run as long as the code path up to the
suspension point is deterministic.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Callable, Optional
from urllib.parse import quote

from .errors import ApiError, HttpError

if TYPE_CHECKING:
    import httpx


def _seg(s: str) -> str:
    """Percent-encode a path segment opaquely (no ``/`` passthrough).

    Mirrors ``_seg`` in ``bus.py``: an execution id or signal name
    that happens to contain a slash must be escaped, not silently
    split into additional path components.
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
            or f"workflow error (status {resp.status_code})"
        )
        raise ApiError(
            code=data.get("code", "WORKFLOW"),
            message=message,
            retryable=data.get("retryable", resp.status_code >= 500),
        )
    except ValueError:
        # Not JSON — fall back to a raw HTTP error with the body text.
        raise HttpError(resp.status_code, resp.text or "workflow error")


# ============================================================================
# Models
# ============================================================================


@dataclass
class WorkflowCheckpoint:
    """A single recorded checkpoint on a workflow execution."""

    seq: int
    name: str
    data: Any
    recorded_at: Optional[str] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "WorkflowCheckpoint":
        return cls(
            seq=d["seq"],
            name=d["name"],
            data=d.get("data"),
            recorded_at=d.get("recorded_at"),
        )


@dataclass
class WorkflowExecution:
    """A workflow execution and its recorded progress.

    ``status`` is one of ``running``, ``waiting_timer``,
    ``waiting_signal``, ``completed``, ``failed``, or ``cancelled``.
    """

    execution_id: str
    workflow: str
    queue: str
    status: str
    input: Any
    checkpoints: list[WorkflowCheckpoint]
    search_attributes: dict[str, Any]
    created_at: str
    updated_at: str
    result: Any = None
    error: Optional[str] = None
    awaiting: Any = None
    parent_id: Optional[str] = None
    children: Optional[list[str]] = None

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "WorkflowExecution":
        return cls(
            execution_id=d["execution_id"],
            workflow=d["workflow"],
            queue=d["queue"],
            status=d["status"],
            input=d.get("input"),
            checkpoints=[
                WorkflowCheckpoint.from_dict(c) for c in d.get("checkpoints", [])
            ],
            search_attributes=d.get("search_attributes", {}) or {},
            created_at=d["created_at"],
            updated_at=d["updated_at"],
            result=d.get("result"),
            error=d.get("error"),
            awaiting=d.get("awaiting"),
            parent_id=d.get("parent_id"),
            children=d.get("children"),
        )


@dataclass
class ExecutionHistory:
    """The recorded event history of a workflow execution."""

    execution_id: str
    events: list[dict[str, Any]] = field(default_factory=list)

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "ExecutionHistory":
        return cls(
            execution_id=d["execution_id"],
            events=d.get("events", []) or [],
        )


# ============================================================================
# Sync mixin
# ============================================================================


class _WorkflowsClientMixin:
    """Mixin providing the workflow REST surface."""

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

    def start_workflow(
        self,
        namespace: str,
        tenant: str,
        workflow: str,
        queue: str,
        input: Any,
        *,
        search_attributes: Optional[dict[str, Any]] = None,
    ) -> WorkflowExecution:
        """Start a new workflow execution.

        Args:
            namespace: The execution namespace.
            tenant: The execution tenant.
            workflow: The registered workflow name.
            queue: The task queue continuation tasks are delivered on.
            input: Arbitrary JSON input handed to the workflow function.
            search_attributes: Optional indexed attributes for listing.

        Returns:
            The created execution (status ``running``).

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns a validation error.
        """
        body: dict[str, Any] = {
            "namespace": namespace,
            "tenant": tenant,
            "workflow": workflow,
            "queue": queue,
            "input": input,
        }
        if search_attributes is not None:
            body["search_attributes"] = search_attributes
        resp = self._request("POST", "/v1/workflows/start", json=body)
        _raise_for_status(resp)
        return WorkflowExecution.from_dict(resp.json())

    def list_workflow_executions(
        self,
        namespace: str,
        tenant: str,
        *,
        workflow: Optional[str] = None,
        status: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> list[WorkflowExecution]:
        """List workflow executions, optionally filtered.

        Args:
            namespace: The execution namespace.
            tenant: The execution tenant.
            workflow: Optional workflow-name filter.
            status: Optional status filter (``running``,
                ``waiting_timer``, ``waiting_signal``, ``completed``,
                ``failed``, ``cancelled``).
            limit: Optional maximum number of results.

        Returns:
            The matching executions.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        params: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if workflow is not None:
            params["workflow"] = workflow
        if status is not None:
            params["status"] = status
        if limit is not None:
            params["limit"] = limit
        resp = self._request("GET", "/v1/workflows/executions", params=params)
        _raise_for_status(resp)
        return [
            WorkflowExecution.from_dict(e) for e in resp.json().get("executions", [])
        ]

    def get_workflow_execution(
        self, execution_id: str, namespace: str, tenant: str
    ) -> Optional[WorkflowExecution]:
        """Get a single workflow execution by ID.

        Returns:
            The execution, or None if not found.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error (other than 404).
        """
        resp = self._request(
            "GET",
            f"/v1/workflows/executions/{_seg(execution_id)}",
            params={"namespace": namespace, "tenant": tenant},
        )
        if resp.status_code == 404:
            return None
        _raise_for_status(resp)
        return WorkflowExecution.from_dict(resp.json())

    def signal_workflow(
        self,
        execution_id: str,
        name: str,
        namespace: str,
        tenant: str,
        *,
        payload: Any = None,
    ) -> None:
        """Deliver a named signal to a workflow execution.

        If the execution is waiting on the signal, the server records
        the corresponding checkpoint with ``payload`` and schedules a
        continuation task.

        Args:
            execution_id: The execution ID.
            name: The signal name.
            namespace: The execution namespace.
            tenant: The execution tenant.
            payload: Optional JSON payload delivered to the workflow.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        body: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if payload is not None:
            body["payload"] = payload
        resp = self._request(
            "POST",
            f"/v1/workflows/executions/{_seg(execution_id)}/signal/{_seg(name)}",
            json=body,
        )
        _raise_for_status(resp)

    def cancel_workflow(
        self,
        execution_id: str,
        namespace: str,
        tenant: str,
        *,
        reason: Optional[str] = None,
    ) -> None:
        """Cancel a workflow execution.

        Args:
            execution_id: The execution ID.
            namespace: The execution namespace.
            tenant: The execution tenant.
            reason: Optional human-readable cancellation reason.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        body: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if reason is not None:
            body["reason"] = reason
        resp = self._request(
            "POST",
            f"/v1/workflows/executions/{_seg(execution_id)}/cancel",
            json=body,
        )
        _raise_for_status(resp)

    def record_workflow_checkpoint(
        self,
        execution_id: str,
        namespace: str,
        tenant: str,
        name: str,
        data: Any,
    ) -> WorkflowCheckpoint:
        """Record a named checkpoint on a workflow execution.

        Idempotent by name: if a checkpoint with the same name was
        already recorded, the server returns the *original* data —
        callers must use the returned data, not what they sent, so
        re-runs that race a previous recording stay consistent.

        Args:
            execution_id: The execution ID.
            namespace: The execution namespace.
            tenant: The execution tenant.
            name: The checkpoint name (unique within the execution).
            data: Arbitrary JSON data to store.

        Returns:
            The recorded (or previously recorded) checkpoint.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        resp = self._request(
            "POST",
            f"/v1/workflows/executions/{_seg(execution_id)}/checkpoints",
            json={
                "namespace": namespace,
                "tenant": tenant,
                "name": name,
                "data": data,
            },
        )
        _raise_for_status(resp)
        return WorkflowCheckpoint.from_dict(resp.json())

    def start_child_workflow(
        self,
        execution_id: str,
        namespace: str,
        tenant: str,
        checkpoint: str,
        workflow: str,
        input: Any,
        *,
        queue: Optional[str] = None,
        parent_close_policy: Optional[str] = None,
    ) -> str:
        """Start a child workflow execution under a parent.

        Idempotent by ``checkpoint``: replays return the child started
        by the first call instead of spawning a duplicate.

        Args:
            execution_id: The parent execution ID.
            namespace: The execution namespace.
            tenant: The execution tenant.
            checkpoint: The checkpoint name recording the spawn.
            workflow: The child workflow name.
            input: Arbitrary JSON input for the child.
            queue: Optional child task queue (defaults to the parent's).
            parent_close_policy: ``abandon`` or ``cancel``.

        Returns:
            The child execution ID.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        body: dict[str, Any] = {
            "namespace": namespace,
            "tenant": tenant,
            "checkpoint": checkpoint,
            "workflow": workflow,
            "input": input,
        }
        if queue is not None:
            body["queue"] = queue
        if parent_close_policy is not None:
            body["parent_close_policy"] = parent_close_policy
        resp = self._request(
            "POST",
            f"/v1/workflows/executions/{_seg(execution_id)}/children",
            json=body,
        )
        _raise_for_status(resp)
        return resp.json()["child_execution_id"]

    def get_execution_history(
        self, execution_id: str, namespace: str, tenant: str
    ) -> ExecutionHistory:
        """Get the recorded event history of a workflow execution.

        Args:
            execution_id: The execution ID.
            namespace: The execution namespace.
            tenant: The execution tenant.

        Returns:
            The execution history.

        Raises:
            ConnectionError: If unable to connect to the server.
            ApiError: If the server returns an error.
        """
        resp = self._request(
            "GET",
            f"/v1/executions/{_seg(execution_id)}/history",
            params={"namespace": namespace, "tenant": tenant},
        )
        _raise_for_status(resp)
        return ExecutionHistory.from_dict(resp.json())


# ============================================================================
# Async mixin
#
# Mounted onto `AsyncActeonClient`. Mirrors the sync mixin exactly;
# the two share zero implementation for the same reason ``bus.py``
# documents — blocking and non-blocking call sites are syntactically
# distinct in Python.
# ============================================================================


class _AsyncWorkflowsClientMixin:
    """Async mixin providing the workflow REST surface."""

    if TYPE_CHECKING:
        async def _request(  # noqa: D401
            self,
            method: str,
            path: str,
            *,
            json: Optional[dict] = None,
            params: Optional[dict] = None,
        ) -> "httpx.Response": ...

    async def start_workflow(
        self,
        namespace: str,
        tenant: str,
        workflow: str,
        queue: str,
        input: Any,
        *,
        search_attributes: Optional[dict[str, Any]] = None,
    ) -> WorkflowExecution:
        """Start a new workflow execution. See the sync mixin."""
        body: dict[str, Any] = {
            "namespace": namespace,
            "tenant": tenant,
            "workflow": workflow,
            "queue": queue,
            "input": input,
        }
        if search_attributes is not None:
            body["search_attributes"] = search_attributes
        resp = await self._request("POST", "/v1/workflows/start", json=body)
        _raise_for_status(resp)
        return WorkflowExecution.from_dict(resp.json())

    async def list_workflow_executions(
        self,
        namespace: str,
        tenant: str,
        *,
        workflow: Optional[str] = None,
        status: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> list[WorkflowExecution]:
        """List workflow executions. See the sync mixin."""
        params: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if workflow is not None:
            params["workflow"] = workflow
        if status is not None:
            params["status"] = status
        if limit is not None:
            params["limit"] = limit
        resp = await self._request("GET", "/v1/workflows/executions", params=params)
        _raise_for_status(resp)
        return [
            WorkflowExecution.from_dict(e) for e in resp.json().get("executions", [])
        ]

    async def get_workflow_execution(
        self, execution_id: str, namespace: str, tenant: str
    ) -> Optional[WorkflowExecution]:
        """Get a single execution by ID, or None on 404. See the sync mixin."""
        resp = await self._request(
            "GET",
            f"/v1/workflows/executions/{_seg(execution_id)}",
            params={"namespace": namespace, "tenant": tenant},
        )
        if resp.status_code == 404:
            return None
        _raise_for_status(resp)
        return WorkflowExecution.from_dict(resp.json())

    async def signal_workflow(
        self,
        execution_id: str,
        name: str,
        namespace: str,
        tenant: str,
        *,
        payload: Any = None,
    ) -> None:
        """Deliver a named signal to an execution. See the sync mixin."""
        body: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if payload is not None:
            body["payload"] = payload
        resp = await self._request(
            "POST",
            f"/v1/workflows/executions/{_seg(execution_id)}/signal/{_seg(name)}",
            json=body,
        )
        _raise_for_status(resp)

    async def cancel_workflow(
        self,
        execution_id: str,
        namespace: str,
        tenant: str,
        *,
        reason: Optional[str] = None,
    ) -> None:
        """Cancel a workflow execution. See the sync mixin."""
        body: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if reason is not None:
            body["reason"] = reason
        resp = await self._request(
            "POST",
            f"/v1/workflows/executions/{_seg(execution_id)}/cancel",
            json=body,
        )
        _raise_for_status(resp)

    async def record_workflow_checkpoint(
        self,
        execution_id: str,
        namespace: str,
        tenant: str,
        name: str,
        data: Any,
    ) -> WorkflowCheckpoint:
        """Record a named checkpoint (idempotent by name). See the sync mixin."""
        resp = await self._request(
            "POST",
            f"/v1/workflows/executions/{_seg(execution_id)}/checkpoints",
            json={
                "namespace": namespace,
                "tenant": tenant,
                "name": name,
                "data": data,
            },
        )
        _raise_for_status(resp)
        return WorkflowCheckpoint.from_dict(resp.json())

    async def start_child_workflow(
        self,
        execution_id: str,
        namespace: str,
        tenant: str,
        checkpoint: str,
        workflow: str,
        input: Any,
        *,
        queue: Optional[str] = None,
        parent_close_policy: Optional[str] = None,
    ) -> str:
        """Start a child execution (idempotent by checkpoint). See the sync mixin."""
        body: dict[str, Any] = {
            "namespace": namespace,
            "tenant": tenant,
            "checkpoint": checkpoint,
            "workflow": workflow,
            "input": input,
        }
        if queue is not None:
            body["queue"] = queue
        if parent_close_policy is not None:
            body["parent_close_policy"] = parent_close_policy
        resp = await self._request(
            "POST",
            f"/v1/workflows/executions/{_seg(execution_id)}/children",
            json=body,
        )
        _raise_for_status(resp)
        return resp.json()["child_execution_id"]

    async def get_execution_history(
        self, execution_id: str, namespace: str, tenant: str
    ) -> ExecutionHistory:
        """Get the recorded event history. See the sync mixin."""
        resp = await self._request(
            "GET",
            f"/v1/executions/{_seg(execution_id)}/history",
            params={"namespace": namespace, "tenant": tenant},
        )
        _raise_for_status(resp)
        return ExecutionHistory.from_dict(resp.json())


# ============================================================================
# Authoring layer: suspension + context
# ============================================================================


class _Suspend(BaseException):
    """Internal control-flow exception that unwinds a workflow function
    at a suspension point.

    Carries the *directive* the worker settles the continuation task
    with (``sleep`` or ``await_signal``). Derives from
    ``BaseException`` so a workflow function's broad ``except
    Exception`` blocks don't swallow it by accident.
    """

    def __init__(self, directive: dict[str, Any]):
        self.directive = directive
        super().__init__(f"workflow suspended: {directive.get('directive')}")


class WorkflowContext:
    """The authoring API handed to workflow functions as ``ctx``.

    Built by the worker from a ``__workflow__`` continuation task.
    Checkpoint keys are derived from per-name occurrence counters
    within the current run (``k`` starts at 0), so the same code path
    yields the same keys on every re-run — see the module docstring
    for the execution model.
    """

    def __init__(
        self,
        client: Any,
        namespace: str,
        tenant: str,
        execution_id: str,
        input: Any,
        checkpoints: dict[str, Any],
    ):
        """Create a workflow context.

        Args:
            client: An :class:`~acteon_client.client.ActeonClient`
                (anything exposing ``record_workflow_checkpoint`` and
                ``start_child_workflow``).
            namespace: The execution namespace.
            tenant: The execution tenant.
            execution_id: The workflow execution ID.
            input: The workflow input.
            checkpoints: Recorded checkpoint data keyed by name.
        """
        self._client = client
        self._namespace = namespace
        self._tenant = tenant
        self._execution_id = execution_id
        self._input = input
        self._checkpoints = checkpoints
        # Per-prefix occurrence counters for stable checkpoint keys.
        self._counters: dict[str, int] = {}

    @property
    def execution_id(self) -> str:
        """The workflow execution ID."""
        return self._execution_id

    @property
    def input(self) -> Any:
        """The workflow input."""
        return self._input

    def _next_key(self, prefix: str) -> str:
        """Derive the next checkpoint key for ``prefix``.

        Keys are ``{prefix}#{k}`` with ``k`` the 0-based occurrence
        count of ``prefix`` so far in this run.
        """
        k = self._counters.get(prefix, 0)
        self._counters[prefix] = k + 1
        return f"{prefix}#{k}"

    def step(self, name: str, fn: Callable[[], Any]) -> Any:
        """Run ``fn`` exactly once across re-runs, checkpointing its result.

        On replay (checkpoint already recorded) the stored result is
        returned without calling ``fn``. Otherwise ``fn`` is executed,
        its result recorded as a checkpoint, and the *server's* stored
        data returned — the checkpoint endpoint is idempotent by name,
        so a concurrent recording wins consistently.

        Args:
            name: A stable step name. Repeated names get distinct
                keys via the occurrence counter.
            fn: A zero-argument callable whose result is JSON-serializable.

        Returns:
            The step result (recorded or replayed).
        """
        key = self._next_key(f"step:{name}")
        if key in self._checkpoints:
            return self._checkpoints[key]
        result = fn()
        checkpoint = self._client.record_workflow_checkpoint(
            self._execution_id, self._namespace, self._tenant, key, result
        )
        self._checkpoints[key] = checkpoint.data
        return checkpoint.data

    def sleep(self, seconds: float) -> None:
        """Suspend the workflow for ``seconds`` (durable timer).

        On replay (timer already fired) returns immediately;
        otherwise suspends the run with a ``sleep`` directive.
        """
        key = self._next_key("sleep")
        if key in self._checkpoints:
            return
        raise _Suspend(
            {"directive": "sleep", "checkpoint": key, "seconds": seconds}
        )

    def wait_for_signal(
        self, name: str, timeout_seconds: Optional[float] = None
    ) -> Any:
        """Suspend until the named signal arrives (or times out).

        On replay returns the recorded signal payload, or ``None`` if
        the wait timed out (the server records ``{"timed_out": true}``
        on timeout). Otherwise suspends the run with an
        ``await_signal`` directive.

        Args:
            name: The signal name.
            timeout_seconds: Optional wait timeout.

        Returns:
            The signal payload, or None on timeout.
        """
        key = self._next_key(f"signal:{name}")
        if key in self._checkpoints:
            data = self._checkpoints[key]
            if data == {"timed_out": True}:
                return None
            return data
        directive: dict[str, Any] = {
            "directive": "await_signal",
            "checkpoint": key,
            "name": name,
        }
        if timeout_seconds is not None:
            directive["timeout_seconds"] = timeout_seconds
        raise _Suspend(directive)

    def start_child(
        self,
        workflow: str,
        input: Any,
        *,
        queue: Optional[str] = None,
        parent_close_policy: str = "abandon",
    ) -> str:
        """Start a child workflow execution exactly once across re-runs.

        On replay returns the previously started child's ID. The
        children endpoint is idempotent by checkpoint, so a re-run
        that races the recording still gets the same child.

        Args:
            workflow: The child workflow name.
            input: Arbitrary JSON input for the child.
            queue: Optional child task queue (defaults to the parent's).
            parent_close_policy: ``abandon`` (default) or ``cancel``.

        Returns:
            The child execution ID.
        """
        key = self._next_key(f"child:{workflow}")
        if key in self._checkpoints:
            return self._checkpoints[key]["child_id"]
        child_id = self._client.start_child_workflow(
            self._execution_id,
            self._namespace,
            self._tenant,
            key,
            workflow,
            input,
            queue=queue,
            parent_close_policy=parent_close_policy,
        )
        self._checkpoints[key] = {"child_id": child_id}
        return child_id

    def wait_for_child(
        self, child_id: str, timeout_seconds: Optional[float] = None
    ) -> Any:
        """Suspend until the child execution closes (or the wait times out).

        The server delivers child completion as the well-known signal
        ``__child:{child_id}`` carrying a ``{"status", "result"?/
        "error"?}`` payload.

        Args:
            child_id: The child execution ID from :meth:`start_child`.
            timeout_seconds: Optional wait timeout.

        Returns:
            The child close payload, or None on timeout.
        """
        return self.wait_for_signal(f"__child:{child_id}", timeout_seconds)
