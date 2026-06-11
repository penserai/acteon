"""Task-queue worker for the Python ActeonClient.

:class:`Worker` wraps the lease-based queue surface
(``queues.py``) in a poll → dispatch → settle loop:

- Plain task handlers are registered per ``action_type`` via
  :meth:`Worker.register` and receive the task payload. The return
  value completes the task; an exception fails it.
- Workflow functions are registered via
  :meth:`Worker.register_workflow`. Continuation tasks arrive with
  the reserved action type ``__workflow__`` and are routed by
  ``payload["workflow"]``. The payload is *slim* — it carries only
  the execution reference — so the worker fetches the execution's
  input and recorded checkpoints from the server, builds a
  :class:`~acteon_client.workflows.WorkflowContext`, and settles the
  task with a *directive* (see ``workflows.py`` for the execution
  model). Legacy fat payloads (pre-slim servers embed ``input`` and
  ``checkpoints`` in the task) are still honored without a fetch.

Failure convention
------------------

A plain exception from a handler fails the task with
``retryable=True`` — transient breakage (network, upstream 5xx) is
the common case, and the server's ``max_attempts`` bounds the blast
radius. Raise :class:`~acteon_client.errors.NonRetryableError` to
fail permanently; :class:`~acteon_client.errors.RetryableError` is
accepted for explicitness but behaves like any other exception.

The worker matches the sync :class:`~acteon_client.client.ActeonClient`
paradigm: handlers run on a thread pool (``max_concurrent`` wide),
and a background thread heartbeats each in-flight task at half the
lease interval so long-running handlers don't lose their lease.
``async def`` handlers are accepted — the worker runs the returned
coroutine to completion on the handler's thread via ``asyncio.run``.
"""

from __future__ import annotations

import asyncio
import inspect
import logging
import threading
import uuid
from concurrent.futures import Future, ThreadPoolExecutor
from typing import Any, Callable, Optional

from .errors import ActeonError, NonRetryableError
from .queues import WorkerTask, _QueuesClientMixin
from .workflows import WorkflowContext, _Suspend

logger = logging.getLogger("acteon_client.worker")

#: Reserved action type the server uses for workflow continuation tasks.
WORKFLOW_ACTION_TYPE = "__workflow__"


def _run_maybe_async(result: Any) -> Any:
    """Drive an ``async def`` handler's coroutine to completion.

    Sync handlers return their result directly; async handlers return
    a coroutine, which is run on the calling (pool) thread.
    """
    if inspect.iscoroutine(result):
        return asyncio.run(result)
    return result


class _LeaseHeartbeat:
    """Background thread that heartbeats one leased task.

    Fires every ``lease_seconds / 2`` until stopped, extending the
    lease by ``lease_seconds`` each time. A failed heartbeat (lease
    lost, server unreachable) stops the loop — the settle call will
    surface the real error.
    """

    def __init__(self, client: _QueuesClientMixin, task: WorkerTask,
                 namespace: str, tenant: str, lease_seconds: int):
        self._client = client
        self._task = task
        self._namespace = namespace
        self._tenant = tenant
        self._lease_seconds = lease_seconds
        self._stopped = threading.Event()
        self._thread = threading.Thread(
            target=self._loop,
            name=f"acteon-heartbeat-{task.task_id}",
            daemon=True,
        )

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stopped.set()
        self._thread.join()

    def _loop(self) -> None:
        interval = self._lease_seconds / 2
        while not self._stopped.wait(interval):
            try:
                self._client.heartbeat_task(
                    self._task.task_id,
                    self._namespace,
                    self._tenant,
                    self._task.lease_token or "",
                    extend_seconds=self._lease_seconds,
                )
            except ActeonError as e:
                logger.warning(
                    "heartbeat failed for task %s: %s", self._task.task_id, e
                )
                return


class Worker:
    """Polls one queue and dispatches tasks to registered handlers.

    Example:
        >>> client = ActeonClient("http://localhost:8080")
        >>> worker = Worker(client, "jobs", "tenant-1", queue="emails")
        >>> worker.register("send_email", lambda payload: do_send(payload))
        >>> worker.register_workflow("onboarding", onboarding_flow)
        >>> worker.run()  # blocks until worker.stop()
    """

    def __init__(
        self,
        client: _QueuesClientMixin,
        namespace: str,
        tenant: str,
        queue: str,
        *,
        worker_id: Optional[str] = None,
        poll_interval: float = 1.0,
        lease_seconds: int = 60,
        max_concurrent: int = 4,
    ):
        """Create a worker.

        Args:
            client: A sync :class:`~acteon_client.client.ActeonClient`.
            namespace: The task namespace.
            tenant: The task tenant.
            queue: The queue to poll.
            worker_id: Stable worker identity sent on poll; a random
                one is generated when omitted.
            poll_interval: Seconds to wait between empty polls.
            lease_seconds: Lease duration requested on poll; in-flight
                tasks are heartbeat-extended at half this interval.
            max_concurrent: Maximum tasks processed concurrently.
        """
        self._client = client
        self._namespace = namespace
        self._tenant = tenant
        self._queue = queue
        self._worker_id = worker_id or f"worker-{uuid.uuid4().hex[:12]}"
        self._poll_interval = poll_interval
        self._lease_seconds = lease_seconds
        self._max_concurrent = max_concurrent
        self._handlers: dict[str, Callable[[Any], Any]] = {}
        self._workflows: dict[str, Callable[[WorkflowContext, Any], Any]] = {}
        self._stop_event = threading.Event()

    @property
    def worker_id(self) -> str:
        """The worker identity sent on every poll."""
        return self._worker_id

    def register(self, action_type: str, handler: Callable[[Any], Any]) -> None:
        """Register a plain task handler for ``action_type``.

        The handler receives the task payload; its return value
        (JSON-serializable) completes the task. A plain exception
        fails the task with ``retryable=True``; raise
        :class:`~acteon_client.errors.NonRetryableError` to fail
        permanently. ``async def`` handlers are supported.

        Args:
            action_type: The task routing key. ``__workflow__`` is
                reserved for workflow continuations.
            handler: Callable invoked as ``handler(payload)``.

        Raises:
            ValueError: If ``action_type`` is the reserved workflow type.
        """
        if action_type == WORKFLOW_ACTION_TYPE:
            raise ValueError(
                f"{WORKFLOW_ACTION_TYPE} is reserved; use register_workflow()"
            )
        self._handlers[action_type] = handler

    def register_workflow(
        self, name: str, fn: Callable[[WorkflowContext, Any], Any]
    ) -> None:
        """Register a workflow function under ``name``.

        Invoked as ``fn(ctx, input)`` on every continuation of the
        execution; use the :class:`~acteon_client.workflows.WorkflowContext`
        primitives (``ctx.step``, ``ctx.sleep``, ...) so completed
        work replays from checkpoints instead of re-executing.

        Args:
            name: The workflow name used in ``start_workflow``.
            fn: The workflow function.
        """
        self._workflows[name] = fn

    # =========================================================================
    # Run loop
    # =========================================================================

    def run(self) -> None:
        """Poll and process tasks until :meth:`stop` is called.

        Blocks the calling thread. Tasks are dispatched onto a pool
        ``max_concurrent`` wide; on shutdown the loop drains in-flight
        tasks before returning. Poll failures are logged and retried
        after ``poll_interval``.
        """
        self._stop_event.clear()
        inflight: set[Future] = set()
        with ThreadPoolExecutor(
            max_workers=self._max_concurrent,
            thread_name_prefix=f"acteon-{self._worker_id}",
        ) as pool:
            while not self._stop_event.is_set():
                inflight = {f for f in inflight if not f.done()}
                capacity = self._max_concurrent - len(inflight)
                tasks: list[WorkerTask] = []
                if capacity > 0:
                    try:
                        tasks = self._client.poll_tasks(
                            self._queue,
                            self._namespace,
                            self._tenant,
                            max_tasks=capacity,
                            lease_seconds=self._lease_seconds,
                            worker_id=self._worker_id,
                        )
                    except ActeonError as e:
                        logger.warning("poll failed: %s", e)
                for task in tasks:
                    inflight.add(pool.submit(self._process_task, task))
                if not tasks:
                    self._stop_event.wait(self._poll_interval)

    def run_once(self, *, max_tasks: Optional[int] = None) -> int:
        """Poll once and process every returned task on this thread.

        Intended for tests and cron-style invocations.

        Args:
            max_tasks: Maximum tasks to lease (defaults to
                ``max_concurrent``).

        Returns:
            The number of tasks processed.
        """
        tasks = self._client.poll_tasks(
            self._queue,
            self._namespace,
            self._tenant,
            max_tasks=max_tasks if max_tasks is not None else self._max_concurrent,
            lease_seconds=self._lease_seconds,
            worker_id=self._worker_id,
        )
        for task in tasks:
            self._process_task(task)
        return len(tasks)

    def stop(self) -> None:
        """Signal :meth:`run` to exit.

        Safe to call from any thread (e.g. a signal handler). The run
        loop stops polling immediately and returns once in-flight
        tasks have settled.
        """
        self._stop_event.set()

    # =========================================================================
    # Dispatch
    # =========================================================================

    def _process_task(self, task: WorkerTask) -> None:
        """Dispatch one leased task and settle it.

        Settlement failures (lease expired, server unreachable) are
        logged rather than raised — the lease will lapse and the
        server re-delivers retryable work.
        """
        heartbeat = _LeaseHeartbeat(
            self._client, task, self._namespace, self._tenant, self._lease_seconds
        )
        heartbeat.start()
        try:
            if task.action_type == WORKFLOW_ACTION_TYPE:
                self._process_workflow_task(task)
            else:
                self._process_plain_task(task)
        except ActeonError as e:
            logger.warning("failed to settle task %s: %s", task.task_id, e)
        finally:
            heartbeat.stop()

    def _process_plain_task(self, task: WorkerTask) -> None:
        handler = self._handlers.get(task.action_type)
        if handler is None:
            # Another worker on the queue may know this action type;
            # release the task for re-delivery rather than burying it.
            self._fail(task, f"no handler registered for {task.action_type!r}", True)
            return
        try:
            result = _run_maybe_async(handler(task.payload))
        except NonRetryableError as e:
            self._fail(task, str(e), False)
        except Exception as e:
            # Default convention: plain exceptions are retryable.
            self._fail(task, str(e), True)
        else:
            self._complete(task, result)

    def _process_workflow_task(self, task: WorkerTask) -> None:
        payload = task.payload or {}
        name = payload.get("workflow", "")
        fn = self._workflows.get(name)
        if fn is None:
            # Another worker on the queue may host this workflow;
            # release the continuation for re-delivery.
            self._fail(task, f"no workflow registered for {name!r}", True)
            return
        execution_id = payload.get("execution_id", "")
        if "checkpoints" in payload:
            # Legacy fat payload (pre-slim server): state is embedded.
            input = payload.get("input")
            checkpoints = {
                c["name"]: c.get("data") for c in payload.get("checkpoints") or []
            }
        else:
            # Slim payload: resolve the execution's input and recorded
            # checkpoints from the server.
            try:
                execution = self._client.get_workflow_execution(
                    execution_id, self._namespace, self._tenant
                )
            except Exception as e:
                # Transient fetch failure: release for re-delivery.
                self._fail(
                    task,
                    f"failed to load workflow execution {execution_id}: {e}",
                    True,
                )
                return
            if execution is None:
                # The execution record is gone (deleted or expired);
                # re-delivery cannot help.
                self._fail(
                    task, f"workflow execution not found: {execution_id}", False
                )
                return
            input = execution.input
            checkpoints = {c.name: c.data for c in execution.checkpoints}
        ctx = WorkflowContext(
            self._client,
            self._namespace,
            self._tenant,
            execution_id=execution_id,
            input=input,
            checkpoints=checkpoints,
        )
        try:
            result = _run_maybe_async(fn(ctx, ctx.input))
        except _Suspend as s:
            directive = s.directive
        except Exception as e:
            directive = {"directive": "fail", "error": str(e)}
        else:
            directive = {"directive": "complete", "result": result}
        self._complete(task, directive)

    def _complete(self, task: WorkerTask, result: Any) -> None:
        self._client.complete_task(
            task.task_id,
            self._namespace,
            self._tenant,
            task.lease_token or "",
            result,
        )

    def _fail(self, task: WorkerTask, error: str, retryable: bool) -> None:
        self._client.fail_task(
            task.task_id,
            self._namespace,
            self._tenant,
            task.lease_token or "",
            error,
            retryable,
        )
