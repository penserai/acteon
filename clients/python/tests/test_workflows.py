"""Workflow Python surface — URL/body capture + context semantics.

Live HTTP tests would need a running Acteon server with workflows
enabled; instead these tests pin two contracts:

1. The mixin methods in ``workflows.py`` hit the documented paths
   with the documented bodies (stub ``_request`` capture, mirroring
   ``test_a2a.py``).
2. :class:`WorkflowContext` implements the checkpoint-based execution
   model — replayed steps don't re-execute, suspension points raise
   ``_Suspend`` with the correct directive, and occurrence counters
   give stable checkpoint names across re-runs.
"""

import unittest
from typing import Any, Optional

from acteon_client import (
    ExecutionHistory,
    WorkflowCheckpoint,
    WorkflowContext,
    WorkflowExecution,
)
from acteon_client.errors import ApiError
from acteon_client.workflows import _Suspend, _WorkflowsClientMixin


def _execution_body(**overrides: Any) -> dict:
    """A minimal valid Execution JSON body, overridable per test."""
    body: dict[str, Any] = {
        "execution_id": "ex-1",
        "workflow": "onboarding",
        "queue": "wf-queue",
        "status": "running",
        "input": {"user": "u-1"},
        "checkpoints": [],
        "search_attributes": {},
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
    workflows mixin uses (``status_code``, ``json()``, ``text``)."""

    def __init__(self, status_code: int = 200, body: Any = None):
        self.status_code = status_code
        self._body = body if body is not None else {}
        self.text = ""

    def json(self):
        return self._body


class _StubClient(_WorkflowsClientMixin):
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


# ---------------------------------------------------------------------
# Client mixin
# ---------------------------------------------------------------------


class TestWorkflowEndpoints(unittest.TestCase):
    def test_start_workflow(self):
        c = _StubClient(body=_execution_body())
        execution = c.start_workflow(
            "ns", "te", "onboarding", "wf-queue", {"user": "u-1"},
            search_attributes={"team": "growth"},
        )
        call = c.calls[0]
        self.assertEqual(call.method, "POST")
        self.assertEqual(call.path, "/v1/workflows/start")
        self.assertEqual(
            call.json,
            {
                "namespace": "ns",
                "tenant": "te",
                "workflow": "onboarding",
                "queue": "wf-queue",
                "input": {"user": "u-1"},
                "search_attributes": {"team": "growth"},
            },
        )
        self.assertEqual(execution.execution_id, "ex-1")
        self.assertEqual(execution.status, "running")

    def test_start_workflow_omits_absent_search_attributes(self):
        c = _StubClient(body=_execution_body())
        c.start_workflow("ns", "te", "onboarding", "wf-queue", {})
        self.assertNotIn("search_attributes", c.calls[0].json)

    def test_list_workflow_executions(self):
        c = _StubClient(body={"executions": [_execution_body()]})
        executions = c.list_workflow_executions(
            "ns", "te", workflow="onboarding", status="running", limit=10
        )
        call = c.calls[0]
        self.assertEqual(call.method, "GET")
        self.assertEqual(call.path, "/v1/workflows/executions")
        self.assertEqual(
            call.params,
            {
                "namespace": "ns",
                "tenant": "te",
                "workflow": "onboarding",
                "status": "running",
                "limit": 10,
            },
        )
        self.assertEqual(len(executions), 1)

    def test_get_workflow_execution(self):
        checkpoint = {
            "seq": 1,
            "name": "step:fetch#0",
            "data": {"rows": 3},
            "recorded_at": "2026-06-10T00:00:30Z",
        }
        c = _StubClient(body=_execution_body(checkpoints=[checkpoint]))
        execution = c.get_workflow_execution("ex-1", "ns", "te")
        call = c.calls[0]
        self.assertEqual(call.method, "GET")
        self.assertEqual(call.path, "/v1/workflows/executions/ex-1")
        self.assertEqual(call.params, {"namespace": "ns", "tenant": "te"})
        self.assertEqual(len(execution.checkpoints), 1)
        self.assertEqual(execution.checkpoints[0].name, "step:fetch#0")
        self.assertEqual(execution.checkpoints[0].data, {"rows": 3})

    def test_get_workflow_execution_404_returns_none(self):
        c = _StubClient(status_code=404)
        self.assertIsNone(c.get_workflow_execution("missing", "ns", "te"))

    def test_signal_workflow(self):
        c = _StubClient()
        c.signal_workflow("ex-1", "approved", "ns", "te", payload={"by": "ops"})
        call = c.calls[0]
        self.assertEqual(call.method, "POST")
        self.assertEqual(call.path, "/v1/workflows/executions/ex-1/signal/approved")
        self.assertEqual(
            call.json, {"namespace": "ns", "tenant": "te", "payload": {"by": "ops"}}
        )

    def test_cancel_workflow(self):
        c = _StubClient()
        c.cancel_workflow("ex-1", "ns", "te", reason="superseded")
        call = c.calls[0]
        self.assertEqual(call.path, "/v1/workflows/executions/ex-1/cancel")
        self.assertEqual(
            call.json, {"namespace": "ns", "tenant": "te", "reason": "superseded"}
        )

    def test_record_workflow_checkpoint(self):
        c = _StubClient(body={"name": "step:fetch#0", "seq": 1, "data": {"rows": 3}})
        checkpoint = c.record_workflow_checkpoint(
            "ex-1", "ns", "te", "step:fetch#0", {"rows": 3}
        )
        call = c.calls[0]
        self.assertEqual(call.path, "/v1/workflows/executions/ex-1/checkpoints")
        self.assertEqual(
            call.json,
            {
                "namespace": "ns",
                "tenant": "te",
                "name": "step:fetch#0",
                "data": {"rows": 3},
            },
        )
        self.assertIsInstance(checkpoint, WorkflowCheckpoint)
        self.assertEqual(checkpoint.seq, 1)
        self.assertEqual(checkpoint.data, {"rows": 3})

    def test_start_child_workflow(self):
        c = _StubClient(body={"child_execution_id": "ex-child"})
        child_id = c.start_child_workflow(
            "ex-1", "ns", "te", "child:sub#0", "sub", {"n": 1},
            queue="other", parent_close_policy="cancel",
        )
        call = c.calls[0]
        self.assertEqual(call.path, "/v1/workflows/executions/ex-1/children")
        self.assertEqual(
            call.json,
            {
                "namespace": "ns",
                "tenant": "te",
                "checkpoint": "child:sub#0",
                "workflow": "sub",
                "input": {"n": 1},
                "queue": "other",
                "parent_close_policy": "cancel",
            },
        )
        self.assertEqual(child_id, "ex-child")

    def test_get_execution_history(self):
        c = _StubClient(
            body={"execution_id": "ex-1", "events": [{"type": "started"}]}
        )
        history = c.get_execution_history("ex-1", "ns", "te")
        call = c.calls[0]
        self.assertEqual(call.method, "GET")
        self.assertEqual(call.path, "/v1/executions/ex-1/history")
        self.assertEqual(call.params, {"namespace": "ns", "tenant": "te"})
        self.assertIsInstance(history, ExecutionHistory)
        self.assertEqual(history.events, [{"type": "started"}])

    def test_path_segments_are_escaped(self):
        c = _StubClient()
        c.signal_workflow("ex/1", "go/now", "ns", "te")
        self.assertEqual(
            c.calls[0].path, "/v1/workflows/executions/ex%2F1/signal/go%2Fnow"
        )

    def test_error_body_raises_api_error(self):
        c = _StubClient(status_code=409, body={"error": "already cancelled"})
        with self.assertRaises(ApiError):
            c.cancel_workflow("ex-1", "ns", "te")

    def test_execution_model_parses_optionals(self):
        execution = WorkflowExecution.from_dict(
            _execution_body(
                status="completed",
                result={"done": True},
                parent_id="ex-parent",
                children=["ex-child"],
                awaiting={"signal": "approved"},
            )
        )
        self.assertEqual(execution.result, {"done": True})
        self.assertEqual(execution.parent_id, "ex-parent")
        self.assertEqual(execution.children, ["ex-child"])
        self.assertEqual(execution.awaiting, {"signal": "approved"})


# ---------------------------------------------------------------------
# WorkflowContext
# ---------------------------------------------------------------------


class _FakeWorkflowClient:
    """Stand-in for the bits of ``ActeonClient`` the context uses.

    Records checkpoint/child calls and emulates the server's
    idempotency: re-recording a name returns the original data.
    """

    def __init__(self):
        self.checkpoints: dict[str, Any] = {}
        self.checkpoint_calls: list[tuple[str, Any]] = []
        self.child_calls: list[dict[str, Any]] = []
        self._next_child = 0

    def record_workflow_checkpoint(
        self, execution_id, namespace, tenant, name, data
    ):
        self.checkpoint_calls.append((name, data))
        if name not in self.checkpoints:
            self.checkpoints[name] = data
        return WorkflowCheckpoint(
            seq=len(self.checkpoints), name=name, data=self.checkpoints[name]
        )

    def start_child_workflow(
        self, execution_id, namespace, tenant, checkpoint, workflow, input,
        *, queue=None, parent_close_policy=None,
    ):
        self.child_calls.append(
            {
                "checkpoint": checkpoint,
                "workflow": workflow,
                "input": input,
                "queue": queue,
                "parent_close_policy": parent_close_policy,
            }
        )
        self._next_child += 1
        return f"ex-child-{self._next_child}"


def _ctx(checkpoints: Optional[dict] = None, client=None) -> WorkflowContext:
    return WorkflowContext(
        client if client is not None else _FakeWorkflowClient(),
        "ns",
        "te",
        execution_id="ex-1",
        input={"user": "u-1"},
        checkpoints=checkpoints if checkpoints is not None else {},
    )


class TestWorkflowContext(unittest.TestCase):
    def test_properties(self):
        ctx = _ctx()
        self.assertEqual(ctx.execution_id, "ex-1")
        self.assertEqual(ctx.input, {"user": "u-1"})

    def test_step_executes_and_records_checkpoint(self):
        client = _FakeWorkflowClient()
        ctx = _ctx(client=client)
        result = ctx.step("fetch", lambda: {"rows": 3})
        self.assertEqual(result, {"rows": 3})
        self.assertEqual(client.checkpoint_calls, [("step:fetch#0", {"rows": 3})])

    def test_step_replays_without_re_executing(self):
        client = _FakeWorkflowClient()
        ctx = _ctx(checkpoints={"step:fetch#0": {"rows": 3}}, client=client)
        calls = []

        def fetch():
            calls.append(1)
            return {"rows": 99}

        result = ctx.step("fetch", fetch)
        self.assertEqual(result, {"rows": 3})  # stored data wins
        self.assertEqual(calls, [])  # fn never ran
        self.assertEqual(client.checkpoint_calls, [])  # nothing recorded

    def test_step_uses_server_data_on_idempotent_replay(self):
        # A concurrent recording won: the server returns the original
        # data, and the context must hand that back, not the local fn
        # result.
        client = _FakeWorkflowClient()
        client.checkpoints["step:fetch#0"] = {"rows": 1}
        ctx = _ctx(client=client)
        result = ctx.step("fetch", lambda: {"rows": 99})
        self.assertEqual(result, {"rows": 1})

    def test_step_counters_distinguish_repeated_names(self):
        client = _FakeWorkflowClient()
        ctx = _ctx(client=client)
        ctx.step("poll", lambda: 1)
        ctx.step("poll", lambda: 2)
        ctx.step("other", lambda: 3)
        names = [name for name, _ in client.checkpoint_calls]
        self.assertEqual(names, ["step:poll#0", "step:poll#1", "step:other#0"])

    def test_counters_are_stable_across_re_runs(self):
        # The defining property of the checkpoint model: re-running
        # the same code path yields the same keys, so recorded work
        # replays and the *next* unrecorded point picks up where the
        # previous run suspended.
        def run(ctx):
            ctx.step("a", lambda: "a0")
            ctx.step("a", lambda: "a1")
            ctx.sleep(5)
            return ctx.wait_for_signal("go")

        # Run 1: suspends at sleep#0.
        with self.assertRaises(_Suspend) as cm:
            run(_ctx())
        self.assertEqual(cm.exception.directive["checkpoint"], "sleep#0")

        # Run 2 (sleep recorded): suspends at signal:go#0.
        recorded = {"step:a#0": "a0", "step:a#1": "a1", "sleep#0": {}}
        with self.assertRaises(_Suspend) as cm:
            run(_ctx(checkpoints=dict(recorded)))
        self.assertEqual(cm.exception.directive["checkpoint"], "signal:go#0")

        # Run 3 (signal recorded): completes with the signal payload.
        recorded["signal:go#0"] = {"by": "ops"}
        self.assertEqual(run(_ctx(checkpoints=recorded)), {"by": "ops"})

    def test_sleep_suspends_with_directive(self):
        ctx = _ctx()
        with self.assertRaises(_Suspend) as cm:
            ctx.sleep(30)
        self.assertEqual(
            cm.exception.directive,
            {"directive": "sleep", "checkpoint": "sleep#0", "seconds": 30},
        )

    def test_sleep_replays_when_checkpointed(self):
        ctx = _ctx(checkpoints={"sleep#0": {}})
        self.assertIsNone(ctx.sleep(30))  # returns instead of suspending

    def test_wait_for_signal_suspends_with_directive(self):
        ctx = _ctx()
        with self.assertRaises(_Suspend) as cm:
            ctx.wait_for_signal("approved", timeout_seconds=120)
        self.assertEqual(
            cm.exception.directive,
            {
                "directive": "await_signal",
                "checkpoint": "signal:approved#0",
                "name": "approved",
                "timeout_seconds": 120,
            },
        )

    def test_wait_for_signal_omits_absent_timeout(self):
        ctx = _ctx()
        with self.assertRaises(_Suspend) as cm:
            ctx.wait_for_signal("approved")
        self.assertNotIn("timeout_seconds", cm.exception.directive)

    def test_wait_for_signal_replays_payload(self):
        ctx = _ctx(checkpoints={"signal:approved#0": {"by": "ops"}})
        self.assertEqual(ctx.wait_for_signal("approved"), {"by": "ops"})

    def test_wait_for_signal_timed_out_returns_none(self):
        ctx = _ctx(checkpoints={"signal:approved#0": {"timed_out": True}})
        self.assertIsNone(ctx.wait_for_signal("approved"))

    def test_start_child_calls_endpoint_once(self):
        client = _FakeWorkflowClient()
        ctx = _ctx(client=client)
        child_id = ctx.start_child("sub", {"n": 1}, parent_close_policy="cancel")
        self.assertEqual(child_id, "ex-child-1")
        self.assertEqual(
            client.child_calls,
            [
                {
                    "checkpoint": "child:sub#0",
                    "workflow": "sub",
                    "input": {"n": 1},
                    "queue": None,
                    "parent_close_policy": "cancel",
                }
            ],
        )

    def test_start_child_replays_recorded_child_id(self):
        client = _FakeWorkflowClient()
        ctx = _ctx(
            checkpoints={"child:sub#0": {"child_id": "ex-prior"}}, client=client
        )
        self.assertEqual(ctx.start_child("sub", {"n": 1}), "ex-prior")
        self.assertEqual(client.child_calls, [])

    def test_wait_for_child_uses_well_known_signal(self):
        ctx = _ctx()
        with self.assertRaises(_Suspend) as cm:
            ctx.wait_for_child("ex-child-1", timeout_seconds=60)
        self.assertEqual(
            cm.exception.directive,
            {
                "directive": "await_signal",
                "checkpoint": "signal:__child:ex-child-1#0",
                "name": "__child:ex-child-1",
                "timeout_seconds": 60,
            },
        )

    def test_wait_for_child_replays_close_payload(self):
        ctx = _ctx(
            checkpoints={
                "signal:__child:ex-child-1#0": {
                    "status": "completed",
                    "result": 42,
                }
            }
        )
        self.assertEqual(
            ctx.wait_for_child("ex-child-1"),
            {"status": "completed", "result": 42},
        )

    def test_suspend_escapes_broad_except(self):
        # _Suspend derives from BaseException, so a workflow body's
        # ``except Exception`` cannot swallow a suspension.
        ctx = _ctx()
        with self.assertRaises(_Suspend):
            try:
                ctx.sleep(5)
            except Exception:  # noqa: BLE001
                self.fail("suspension was swallowed by `except Exception`")


if __name__ == "__main__":
    unittest.main()
