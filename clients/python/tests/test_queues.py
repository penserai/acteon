"""Task-queue Python surface — URL/body capture + model parsing.

Live HTTP tests would need a running Acteon server with task queues
enabled; instead these tests pin the wire surface of ``queues.py``:
each mixin method hits the documented path with the documented body,
and ``WorkerTask`` round-trips every wire field the server sends.
The mocking approach mirrors ``test_a2a.py`` — a stub host class
records every ``_request`` call and returns a canned body so the
mixin's parsing code runs end-to-end.
"""

import unittest
from typing import Any, Optional

from acteon_client import WorkerTask
from acteon_client.errors import ApiError
from acteon_client.queues import _QueuesClientMixin


def _task_body(**overrides: Any) -> dict:
    """A minimal valid Task JSON body, overridable per test."""
    body: dict[str, Any] = {
        "task_id": "t-1",
        "queue": "emails",
        "action_type": "send_email",
        "payload": {"to": "a@b.c"},
        "status": "leased",
        "attempt": 1,
        "max_attempts": 3,
        "lease_token": "lease-1",
        "lease_expires_at": "2026-06-10T00:01:00Z",
        "created_at": "2026-06-10T00:00:00Z",
        "updated_at": "2026-06-10T00:00:00Z",
    }
    body.update(overrides)
    return body


class _Captured:
    """One captured ``_request`` call."""

    def __init__(
        self,
        method: str,
        path: str,
        json: Optional[dict],
        params: Optional[dict],
    ):
        self.method = method
        self.path = path
        self.json = json
        self.params = params


class _FakeResponse:
    """Minimal ``httpx.Response`` stand-in covering only the bits the
    queues mixin uses (``status_code``, ``json()``, ``text``)."""

    def __init__(self, status_code: int = 200, body: Any = None):
        self.status_code = status_code
        self._body = body if body is not None else {}
        self.text = ""

    def json(self):
        return self._body


class _StubClient(_QueuesClientMixin):
    """Mixin host that records every ``_request`` call without
    hitting the network. Returns a canned body so the mixin's
    parsing code runs end-to-end.
    """

    def __init__(self, body: Any = None, status_code: int = 200):
        self.calls: list[_Captured] = []
        self._body = body if body is not None else {}
        self._status_code = status_code

    def _request(
        self,
        method: str,
        path: str,
        *,
        json: Optional[dict] = None,
        params: Optional[dict] = None,
    ):
        self.calls.append(_Captured(method, path, json, params))
        return _FakeResponse(status_code=self._status_code, body=self._body)


class TestWorkerTaskModel(unittest.TestCase):
    def test_from_dict_complete(self):
        task = WorkerTask.from_dict(
            _task_body(
                result={"ok": True},
                error="late",
                chain_id="ch-1",
                workflow_execution_id="wf-1",
            )
        )
        self.assertEqual(task.task_id, "t-1")
        self.assertEqual(task.queue, "emails")
        self.assertEqual(task.action_type, "send_email")
        self.assertEqual(task.payload, {"to": "a@b.c"})
        self.assertEqual(task.status, "leased")
        self.assertEqual(task.attempt, 1)
        self.assertEqual(task.max_attempts, 3)
        self.assertEqual(task.lease_token, "lease-1")
        self.assertEqual(task.lease_expires_at, "2026-06-10T00:01:00Z")
        self.assertEqual(task.result, {"ok": True})
        self.assertEqual(task.error, "late")
        self.assertEqual(task.chain_id, "ch-1")
        self.assertEqual(task.workflow_execution_id, "wf-1")

    def test_from_dict_minimal(self):
        body = _task_body(status="pending")
        del body["lease_token"]
        del body["lease_expires_at"]
        task = WorkerTask.from_dict(body)
        self.assertEqual(task.status, "pending")
        self.assertIsNone(task.lease_token)
        self.assertIsNone(task.lease_expires_at)
        self.assertIsNone(task.result)
        self.assertIsNone(task.error)
        self.assertIsNone(task.chain_id)
        self.assertIsNone(task.workflow_execution_id)


class TestQueueEndpoints(unittest.TestCase):
    def test_enqueue_task(self):
        c = _StubClient(body=_task_body(status="pending"))
        task = c.enqueue_task(
            "emails", "ns", "te", "send_email", {"to": "a@b.c"}, max_attempts=5
        )
        call = c.calls[0]
        self.assertEqual(call.method, "POST")
        self.assertEqual(call.path, "/v1/queues/emails/tasks")
        self.assertEqual(
            call.json,
            {
                "namespace": "ns",
                "tenant": "te",
                "action_type": "send_email",
                "payload": {"to": "a@b.c"},
                "max_attempts": 5,
            },
        )
        self.assertEqual(task.task_id, "t-1")

    def test_enqueue_task_omits_absent_max_attempts(self):
        c = _StubClient(body=_task_body(status="pending"))
        c.enqueue_task("emails", "ns", "te", "send_email", {})
        self.assertNotIn("max_attempts", c.calls[0].json)

    def test_poll_tasks(self):
        c = _StubClient(body={"tasks": [_task_body()]})
        tasks = c.poll_tasks(
            "emails", "ns", "te",
            max_tasks=2, lease_seconds=30, worker_id="w-1",
        )
        call = c.calls[0]
        self.assertEqual(call.method, "POST")
        self.assertEqual(call.path, "/v1/queues/emails/poll")
        self.assertEqual(
            call.json,
            {
                "namespace": "ns",
                "tenant": "te",
                "max_tasks": 2,
                "lease_seconds": 30,
                "worker_id": "w-1",
            },
        )
        self.assertEqual(len(tasks), 1)
        self.assertEqual(tasks[0].lease_token, "lease-1")

    def test_poll_tasks_minimal_body(self):
        c = _StubClient(body={"tasks": []})
        tasks = c.poll_tasks("emails", "ns", "te")
        self.assertEqual(c.calls[0].json, {"namespace": "ns", "tenant": "te"})
        self.assertEqual(tasks, [])

    def test_heartbeat_task(self):
        c = _StubClient(body=_task_body())
        c.heartbeat_task("t-1", "ns", "te", "lease-1", extend_seconds=60)
        call = c.calls[0]
        self.assertEqual(call.method, "POST")
        self.assertEqual(call.path, "/v1/queues/tasks/t-1/heartbeat")
        self.assertEqual(
            call.json,
            {
                "namespace": "ns",
                "tenant": "te",
                "lease_token": "lease-1",
                "extend_seconds": 60,
            },
        )

    def test_complete_task(self):
        c = _StubClient(body=_task_body(status="completed", result={"ok": True}))
        task = c.complete_task("t-1", "ns", "te", "lease-1", {"ok": True})
        call = c.calls[0]
        self.assertEqual(call.method, "POST")
        self.assertEqual(call.path, "/v1/queues/tasks/t-1/complete")
        self.assertEqual(
            call.json,
            {
                "namespace": "ns",
                "tenant": "te",
                "lease_token": "lease-1",
                "result": {"ok": True},
            },
        )
        self.assertEqual(task.status, "completed")

    def test_fail_task(self):
        c = _StubClient(body=_task_body(status="failed", error="boom"))
        task = c.fail_task("t-1", "ns", "te", "lease-1", "boom", True)
        call = c.calls[0]
        self.assertEqual(call.method, "POST")
        self.assertEqual(call.path, "/v1/queues/tasks/t-1/fail")
        self.assertEqual(
            call.json,
            {
                "namespace": "ns",
                "tenant": "te",
                "lease_token": "lease-1",
                "error": "boom",
                "retryable": True,
            },
        )
        self.assertEqual(task.status, "failed")

    def test_get_task(self):
        c = _StubClient(body=_task_body())
        task = c.get_task("t-1", "ns", "te")
        call = c.calls[0]
        self.assertEqual(call.method, "GET")
        self.assertEqual(call.path, "/v1/queues/tasks/t-1")
        self.assertEqual(call.params, {"namespace": "ns", "tenant": "te"})
        self.assertEqual(task.task_id, "t-1")

    def test_get_task_404_returns_none(self):
        c = _StubClient(status_code=404)
        self.assertIsNone(c.get_task("missing", "ns", "te"))

    def test_list_tasks(self):
        c = _StubClient(body={"tasks": [_task_body()]})
        tasks = c.list_tasks("emails", "ns", "te", status="leased")
        call = c.calls[0]
        self.assertEqual(call.method, "GET")
        self.assertEqual(call.path, "/v1/queues/emails/tasks")
        self.assertEqual(
            call.params, {"namespace": "ns", "tenant": "te", "status": "leased"}
        )
        self.assertEqual(len(tasks), 1)

    def test_path_segments_are_escaped(self):
        c = _StubClient(body={"tasks": []})
        c.poll_tasks("a/b", "ns", "te")
        self.assertEqual(c.calls[0].path, "/v1/queues/a%2Fb/poll")

    def test_error_body_raises_api_error(self):
        c = _StubClient(
            status_code=409,
            body={"code": "LEASE_EXPIRED", "message": "lease expired"},
        )
        with self.assertRaises(ApiError) as cm:
            c.complete_task("t-1", "ns", "te", "stale", {})
        self.assertEqual(cm.exception.code, "LEASE_EXPIRED")


if __name__ == "__main__":
    unittest.main()
