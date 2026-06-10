"""Worker loop — poll → dispatch → settle, against a fake client.

The worker only talks to the client through the queue surface
(``poll_tasks`` / ``heartbeat_task`` / ``complete_task`` /
``fail_task``), so these tests substitute a fake client that records
every settle call — the same boundary the ``_StubClient`` pattern in
``test_a2a.py`` mocks, one layer up. Covered contracts:

- plain handler success completes with the handler result;
- handler exceptions fail retryable by default, ``NonRetryableError``
  opts out;
- ``__workflow__`` tasks route by ``payload["workflow"]`` and settle
  with a directive (complete / fail / sleep / await_signal);
- replayed checkpoints are not re-executed;
- ``run()`` processes tasks until ``stop()``;
- long-running handlers are heartbeat-extended.
"""

import threading
import time
import unittest
from typing import Any, Optional

from acteon_client import (
    NonRetryableError,
    RetryableError,
    Worker,
    WorkerTask,
)
from acteon_client.worker import WORKFLOW_ACTION_TYPE


def _task(action_type: str = "send_email", payload: Any = None, **overrides: Any) -> WorkerTask:
    fields: dict[str, Any] = {
        "task_id": "t-1",
        "queue": "emails",
        "action_type": action_type,
        "payload": payload if payload is not None else {"to": "a@b.c"},
        "status": "leased",
        "attempt": 1,
        "max_attempts": 3,
        "lease_token": "lease-1",
        "created_at": "2026-06-10T00:00:00Z",
        "updated_at": "2026-06-10T00:00:00Z",
    }
    fields.update(overrides)
    return WorkerTask(**fields)


def _workflow_task(
    workflow: str = "onboarding",
    input: Any = None,
    checkpoints: Optional[list] = None,
) -> WorkerTask:
    return _task(
        action_type=WORKFLOW_ACTION_TYPE,
        payload={
            "execution_id": "ex-1",
            "workflow": workflow,
            "input": input if input is not None else {"user": "u-1"},
            "checkpoints": checkpoints or [],
        },
    )


class _FakeClient:
    """Queue + workflow surface stand-in recording every settle call.

    ``poll_tasks`` pops one batch per call from ``batches`` (then
    returns empty), emulating a queue that drains.
    """

    def __init__(self, batches: Optional[list[list[WorkerTask]]] = None):
        self.batches = batches or []
        self.poll_calls: list[dict[str, Any]] = []
        self.heartbeats: list[str] = []
        self.completes: list[tuple[str, Any]] = []
        self.fails: list[tuple[str, str, bool]] = []
        self.checkpoint_calls: list[tuple[str, Any]] = []
        self._lock = threading.Lock()

    def poll_tasks(self, queue, namespace, tenant, *, max_tasks=None,
                   lease_seconds=None, worker_id=None):
        with self._lock:
            self.poll_calls.append(
                {
                    "queue": queue,
                    "namespace": namespace,
                    "tenant": tenant,
                    "max_tasks": max_tasks,
                    "lease_seconds": lease_seconds,
                    "worker_id": worker_id,
                }
            )
            if self.batches:
                return self.batches.pop(0)
            return []

    def heartbeat_task(self, task_id, namespace, tenant, lease_token, *,
                       extend_seconds=None):
        with self._lock:
            self.heartbeats.append(task_id)
        return _task(task_id=task_id)

    def complete_task(self, task_id, namespace, tenant, lease_token, result):
        with self._lock:
            self.completes.append((task_id, result))
        return _task(task_id=task_id, status="completed", result=result)

    def fail_task(self, task_id, namespace, tenant, lease_token, error, retryable):
        with self._lock:
            self.fails.append((task_id, error, retryable))
        return _task(task_id=task_id, status="failed", error=error)

    def record_workflow_checkpoint(self, execution_id, namespace, tenant, name, data):
        from acteon_client import WorkflowCheckpoint

        with self._lock:
            self.checkpoint_calls.append((name, data))
        return WorkflowCheckpoint(seq=len(self.checkpoint_calls), name=name, data=data)

    def start_child_workflow(self, *args, **kwargs):
        raise AssertionError("not used in these tests")


def _worker(client: _FakeClient, **overrides: Any) -> Worker:
    kwargs: dict[str, Any] = {
        "worker_id": "w-test",
        "poll_interval": 0.01,
        "lease_seconds": 60,
        "max_concurrent": 2,
    }
    kwargs.update(overrides)
    return Worker(client, "ns", "te", "emails", **kwargs)


class TestPlainHandlers(unittest.TestCase):
    def test_poll_handle_complete(self):
        client = _FakeClient(batches=[[_task()]])
        worker = _worker(client)
        worker.register("send_email", lambda payload: {"sent": payload["to"]})

        processed = worker.run_once()

        self.assertEqual(processed, 1)
        self.assertEqual(client.completes, [("t-1", {"sent": "a@b.c"})])
        self.assertEqual(client.fails, [])
        poll = client.poll_calls[0]
        self.assertEqual(poll["queue"], "emails")
        self.assertEqual(poll["namespace"], "ns")
        self.assertEqual(poll["tenant"], "te")
        self.assertEqual(poll["lease_seconds"], 60)
        self.assertEqual(poll["worker_id"], "w-test")

    def test_run_once_with_empty_queue(self):
        client = _FakeClient()
        worker = _worker(client)
        self.assertEqual(worker.run_once(), 0)

    def test_handler_exception_fails_retryable_by_default(self):
        client = _FakeClient(batches=[[_task()]])
        worker = _worker(client)

        def handler(payload):
            raise ValueError("smtp unreachable")

        worker.register("send_email", handler)
        worker.run_once()

        self.assertEqual(client.completes, [])
        self.assertEqual(client.fails, [("t-1", "smtp unreachable", True)])

    def test_retryable_error_fails_retryable(self):
        client = _FakeClient(batches=[[_task()]])
        worker = _worker(client)

        def handler(payload):
            raise RetryableError("rate limited")

        worker.register("send_email", handler)
        worker.run_once()

        self.assertEqual(len(client.fails), 1)
        self.assertTrue(client.fails[0][2])

    def test_non_retryable_error_fails_permanently(self):
        client = _FakeClient(batches=[[_task()]])
        worker = _worker(client)

        def handler(payload):
            raise NonRetryableError("malformed address")

        worker.register("send_email", handler)
        worker.run_once()

        self.assertEqual(len(client.fails), 1)
        self.assertFalse(client.fails[0][2])

    def test_unregistered_action_type_fails_retryable(self):
        client = _FakeClient(batches=[[_task(action_type="unknown_type")]])
        worker = _worker(client)
        worker.run_once()

        self.assertEqual(len(client.fails), 1)
        task_id, error, retryable = client.fails[0]
        self.assertIn("unknown_type", error)
        self.assertTrue(retryable)

    def test_async_handler_is_supported(self):
        client = _FakeClient(batches=[[_task()]])
        worker = _worker(client)

        async def handler(payload):
            return {"sent": True}

        worker.register("send_email", handler)
        worker.run_once()

        self.assertEqual(client.completes, [("t-1", {"sent": True})])

    def test_register_rejects_reserved_workflow_type(self):
        worker = _worker(_FakeClient())
        with self.assertRaises(ValueError):
            worker.register(WORKFLOW_ACTION_TYPE, lambda payload: None)


class TestWorkflowTasks(unittest.TestCase):
    def test_workflow_return_completes_with_directive(self):
        client = _FakeClient(batches=[[_workflow_task()]])
        worker = _worker(client)

        def onboarding(ctx, input):
            return {"welcomed": input["user"]}

        worker.register_workflow("onboarding", onboarding)
        worker.run_once()

        self.assertEqual(
            client.completes,
            [("t-1", {"directive": "complete", "result": {"welcomed": "u-1"}})],
        )

    def test_workflow_exception_completes_with_fail_directive(self):
        client = _FakeClient(batches=[[_workflow_task()]])
        worker = _worker(client)

        def onboarding(ctx, input):
            raise RuntimeError("provisioning broke")

        worker.register_workflow("onboarding", onboarding)
        worker.run_once()

        # A workflow failure is delivered as a directive via the
        # *complete* endpoint — the task itself succeeded at running
        # the workflow function.
        self.assertEqual(client.fails, [])
        self.assertEqual(
            client.completes,
            [("t-1", {"directive": "fail", "error": "provisioning broke"})],
        )

    def test_workflow_sleep_completes_with_sleep_directive(self):
        client = _FakeClient(batches=[[_workflow_task()]])
        worker = _worker(client)

        def onboarding(ctx, input):
            ctx.sleep(30)
            return "never reached"

        worker.register_workflow("onboarding", onboarding)
        worker.run_once()

        self.assertEqual(
            client.completes,
            [("t-1", {"directive": "sleep", "checkpoint": "sleep#0", "seconds": 30})],
        )

    def test_workflow_await_signal_completes_with_directive(self):
        client = _FakeClient(batches=[[_workflow_task()]])
        worker = _worker(client)

        def onboarding(ctx, input):
            return ctx.wait_for_signal("approved", timeout_seconds=300)

        worker.register_workflow("onboarding", onboarding)
        worker.run_once()

        self.assertEqual(
            client.completes,
            [
                (
                    "t-1",
                    {
                        "directive": "await_signal",
                        "checkpoint": "signal:approved#0",
                        "name": "approved",
                        "timeout_seconds": 300,
                    },
                )
            ],
        )

    def test_workflow_replay_skips_checkpointed_step(self):
        checkpoints = [
            {"seq": 1, "name": "step:provision#0", "data": {"account": "acct-9"}},
            {"seq": 2, "name": "sleep#0", "data": {}},
        ]
        client = _FakeClient(batches=[[_workflow_task(checkpoints=checkpoints)]])
        worker = _worker(client)
        executed = []

        def onboarding(ctx, input):
            account = ctx.step("provision", lambda: executed.append(1) or {"account": "new"})
            ctx.sleep(30)
            return account

        worker.register_workflow("onboarding", onboarding)
        worker.run_once()

        self.assertEqual(executed, [])  # checkpointed step not re-executed
        self.assertEqual(client.checkpoint_calls, [])  # nothing re-recorded
        self.assertEqual(
            client.completes,
            [("t-1", {"directive": "complete", "result": {"account": "acct-9"}})],
        )

    def test_workflow_first_run_records_step_then_suspends(self):
        client = _FakeClient(batches=[[_workflow_task()]])
        worker = _worker(client)

        def onboarding(ctx, input):
            ctx.step("provision", lambda: {"account": "acct-9"})
            ctx.sleep(30)
            return "done"

        worker.register_workflow("onboarding", onboarding)
        worker.run_once()

        self.assertEqual(
            client.checkpoint_calls, [("step:provision#0", {"account": "acct-9"})]
        )
        self.assertEqual(
            client.completes,
            [("t-1", {"directive": "sleep", "checkpoint": "sleep#0", "seconds": 30})],
        )

    def test_unregistered_workflow_fails_retryable(self):
        client = _FakeClient(batches=[[_workflow_task(workflow="unknown_flow")]])
        worker = _worker(client)
        worker.run_once()

        self.assertEqual(len(client.fails), 1)
        task_id, error, retryable = client.fails[0]
        self.assertIn("unknown_flow", error)
        self.assertTrue(retryable)


class TestRunLoop(unittest.TestCase):
    def test_run_processes_until_stopped(self):
        client = _FakeClient(batches=[[_task()]])
        worker = _worker(client)
        worker.register("send_email", lambda payload: "ok")

        thread = threading.Thread(target=worker.run, daemon=True)
        thread.start()
        deadline = time.monotonic() + 5.0
        while not client.completes and time.monotonic() < deadline:
            time.sleep(0.01)
        worker.stop()
        thread.join(timeout=5.0)

        self.assertFalse(thread.is_alive())
        self.assertEqual(client.completes, [("t-1", "ok")])

    def test_stop_before_run_exits_promptly(self):
        client = _FakeClient()
        worker = _worker(client)
        thread = threading.Thread(target=worker.run, daemon=True)
        thread.start()
        time.sleep(0.05)
        worker.stop()
        thread.join(timeout=5.0)
        self.assertFalse(thread.is_alive())

    def test_long_running_handler_is_heartbeated(self):
        client = _FakeClient(batches=[[_task()]])
        # lease 0.2s → heartbeat every 0.1s; the handler runs 0.35s,
        # so at least two heartbeats land before settlement.
        worker = _worker(client, lease_seconds=0.2)
        worker.register("send_email", lambda payload: time.sleep(0.35) or "ok")
        worker.run_once()

        self.assertGreaterEqual(len(client.heartbeats), 2)
        self.assertEqual(client.completes, [("t-1", "ok")])

    def test_fast_handler_is_not_heartbeated(self):
        client = _FakeClient(batches=[[_task()]])
        worker = _worker(client, lease_seconds=60)
        worker.register("send_email", lambda payload: "ok")
        worker.run_once()
        self.assertEqual(client.heartbeats, [])


if __name__ == "__main__":
    unittest.main()
