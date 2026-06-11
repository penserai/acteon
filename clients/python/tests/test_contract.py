"""Python side of the cross-SDK workflow contract.

Drives the public SDK surface (``WorkflowContext`` + ``Worker``) against
the shared fixtures in ``clients/contract-fixtures/workflow-contract.json``
— the same file consumed by the Node.js SDK tests and by the Rust server
tests — so checkpoint-key derivation, directive wire shapes, and the
integer-seconds coercions stay identical across languages. A workflow
execution can migrate between Python and Node workers mid-flight; these
keys and shapes are the entire compatibility surface.
"""

import json
import unittest
from pathlib import Path
from typing import Any

from acteon_client.worker import WORKFLOW_ACTION_TYPE, Worker
from acteon_client.workflows import WorkflowCheckpoint, WorkflowContext, _Suspend

FIXTURES = json.loads(
    (Path(__file__).parent / "../../contract-fixtures/workflow-contract.json")
    .resolve()
    .read_text()
)


class _ScenarioClient:
    """Stub client recording checkpoint keys in first-recorded order."""

    def __init__(self, checkpoints: dict[str, Any], keys: list[str]):
        self._checkpoints = checkpoints
        self._keys = keys
        self._children = 0

    def record_workflow_checkpoint(self, execution_id, namespace, tenant, name, data):
        if name not in self._checkpoints:
            self._keys.append(name)
            self._checkpoints[name] = data
        return WorkflowCheckpoint(seq=len(self._keys), name=name, data=data)

    def start_child_workflow(
        self, execution_id, namespace, tenant, checkpoint, workflow, input, **kwargs
    ):
        if checkpoint not in self._checkpoints:
            self._keys.append(checkpoint)
            self._children += 1
            self._checkpoints[checkpoint] = {"child_id": f"child-{self._children}"}
        return self._checkpoints[checkpoint]["child_id"]


def _run_scenario(ops: list[dict[str, Any]]) -> list[str]:
    """Run an op sequence continuation-style, collecting checkpoint keys.

    Mirrors how a real execution proceeds: run the workflow function from
    the top; the first un-checkpointed suspension unwinds it; record the
    suspension's checkpoint (as the server would on resume) and re-run.
    The returned keys are in first-recorded order.
    """
    checkpoints: dict[str, Any] = {}
    keys: list[str] = []

    def workflow(ctx: WorkflowContext) -> None:
        for op in ops:
            if op["op"] == "step":
                ctx.step(op["name"], lambda: {"r": 1})
            elif op["op"] == "sleep":
                ctx.sleep(op.get("seconds", 1))
            elif op["op"] == "wait_for_signal":
                ctx.wait_for_signal(op["name"])
            elif op["op"] == "start_child":
                ctx.start_child(op["workflow"], {})
            elif op["op"] == "wait_for_child":
                ctx.wait_for_child(op["child_id"])
            else:  # pragma: no cover - fixture/runner drift
                raise AssertionError(f"unknown op: {op}")

    for _ in range(100):  # bounded; each round resolves one suspension
        client = _ScenarioClient(checkpoints, keys)
        ctx = WorkflowContext(client, "ns", "t1", "ex-1", {}, dict(checkpoints))
        try:
            workflow(ctx)
            return keys
        except _Suspend as s:
            key = s.directive["checkpoint"]
            assert key not in checkpoints, "suspended on a recorded checkpoint"
            keys.append(key)
            # Resolve the suspension the way the server would.
            checkpoints[key] = (
                {} if s.directive["directive"] == "sleep" else {"payload": True}
            )
    raise AssertionError("scenario did not settle in 100 continuations")


def _suspension_directive(run) -> dict[str, Any]:
    """Capture the directive raised by a context operation."""
    ctx = WorkflowContext(None, "ns", "t1", "ex-1", {}, {})
    try:
        run(ctx)
    except _Suspend as s:
        return s.directive
    raise AssertionError("operation did not suspend")


class TestConstants(unittest.TestCase):
    def test_workflow_task_action_type(self):
        self.assertEqual(
            WORKFLOW_ACTION_TYPE,
            FIXTURES["constants"]["workflow_task_action_type"],
        )


class TestCheckpointKeys(unittest.TestCase):
    def test_scenarios(self):
        for scenario in FIXTURES["checkpoint_key_scenarios"]:
            with self.subTest(scenario=scenario["name"]):
                self.assertEqual(
                    _run_scenario(scenario["ops"]), scenario["expected_keys"]
                )


class TestDirectiveShapes(unittest.TestCase):
    """The directives this SDK emits must equal the fixture JSON exactly."""

    def _fixture(self, name: str) -> dict[str, Any]:
        return next(d for d in FIXTURES["directives"] if d["name"] == name)["json"]

    def test_sleep(self):
        directive = _suspension_directive(lambda ctx: ctx.sleep(30))
        self.assertEqual(directive, self._fixture("sleep"))

    def test_await_signal_with_timeout(self):
        directive = _suspension_directive(
            lambda ctx: ctx.wait_for_signal("approved", timeout_seconds=300)
        )
        self.assertEqual(directive, self._fixture("await_signal"))

    def test_await_signal_without_timeout(self):
        directive = _suspension_directive(lambda ctx: ctx.wait_for_signal("go"))
        self.assertEqual(directive, self._fixture("await_signal_no_timeout"))

    def test_complete_and_fail_via_worker(self):
        # complete/fail are wrapped by the worker when settling the task;
        # exercise that seam with fat payloads (no execution fetch needed).
        for fixture_name, fn in [
            ("complete", lambda ctx, inp: {"ok": True, "count": 3}),
            ("fail", _raise_provisioning_broke),
        ]:
            with self.subTest(directive=fixture_name):
                settled = _settle_workflow(fn)
                self.assertEqual(settled, self._fixture(fixture_name))


def _raise_provisioning_broke(ctx, inp):
    raise RuntimeError("provisioning broke")


def _settle_workflow(fn) -> dict[str, Any]:
    """Run one continuation through the Worker and return the settle body."""
    from acteon_client.queues import WorkerTask

    task = WorkerTask(
        task_id="t-1",
        queue="q",
        action_type=WORKFLOW_ACTION_TYPE,
        payload={
            "execution_id": "ex-1",
            "workflow": "wf",
            "input": {},
            "checkpoints": [],
        },
        status="leased",
        attempt=1,
        max_attempts=3,
        lease_token="lease-1",
        created_at="2026-06-11T00:00:00Z",
        updated_at="2026-06-11T00:00:00Z",
    )

    settled: list[Any] = []

    class _Client:
        def poll_tasks(self, *args, **kwargs):
            return [task] if not settled else []

        def heartbeat_task(self, *args, **kwargs):
            return task

        def complete_task(self, task_id, namespace, tenant, lease_token, result):
            settled.append(result)
            return task

        def fail_task(self, *args, **kwargs):  # pragma: no cover - contract drift
            raise AssertionError("workflow continuation must settle via complete")

    worker = Worker(_Client(), "ns", "t1", "q", lease_seconds=60)
    worker.register_workflow("wf", fn)
    worker.run_once()
    assert len(settled) == 1
    return settled[0]


class TestCoercions(unittest.TestCase):
    def test_sleep_seconds_coercion(self):
        for case in FIXTURES["sleep_coercions"]:
            with self.subTest(input=case["input_seconds"]):
                directive = _suspension_directive(
                    lambda ctx, s=case["input_seconds"]: ctx.sleep(s)
                )
                self.assertEqual(directive["seconds"], case["expected_seconds"])
                self.assertIsInstance(directive["seconds"], int)

    def test_signal_timeout_coercion(self):
        for case in FIXTURES["signal_timeout_coercions"]:
            with self.subTest(input=case["input_seconds"]):
                directive = _suspension_directive(
                    lambda ctx, s=case["input_seconds"]: ctx.wait_for_signal(
                        "x", timeout_seconds=s
                    )
                )
                self.assertEqual(
                    directive["timeout_seconds"], case["expected_seconds"]
                )
                self.assertIsInstance(directive["timeout_seconds"], int)


class TestTimedOutMarker(unittest.TestCase):
    def test_timed_out_checkpoint_replays_as_none(self):
        marker = FIXTURES["constants"]["timed_out_marker"]
        ctx = WorkflowContext(None, "ns", "t1", "ex-1", {}, {"signal:x#0": marker})
        self.assertIsNone(ctx.wait_for_signal("x"))


if __name__ == "__main__":
    unittest.main()
