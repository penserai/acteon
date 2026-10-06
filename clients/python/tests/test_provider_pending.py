import unittest

from acteon_client import ActionOutcome, ProviderWorkPending


class ProviderPendingTests(unittest.TestCase):
    def test_pending_states_preserve_receipt_identity(self):
        for state in [
            {"kind": "in_flight", "attempt_id": "attempt-1"},
            {"kind": "reconciliation_required", "attempt_id": "attempt-1"},
            {"kind": "awaiting_retry", "not_before_ms": 1234},
        ]:
            with self.subTest(state=state):
                outcome = ActionOutcome.from_dict(
                    {
                        "ProviderPending": {
                            "execution_id": "00000000-0000-0000-0000-000000000001",
                            "attempts": 1,
                            "state": state,
                        }
                    }
                )
                self.assertEqual(outcome.outcome_type, "provider_pending")
                self.assertIsInstance(outcome.pending, ProviderWorkPending)
                self.assertEqual(
                    outcome.pending.execution_id, "00000000-0000-0000-0000-000000000001"
                )
                self.assertEqual(outcome.pending.attempts, 1)
                self.assertEqual(outcome.pending.state.kind, state["kind"])
                self.assertEqual(outcome.pending.state.attempt_id, state.get("attempt_id"))
                self.assertEqual(outcome.pending.state.not_before_ms, state.get("not_before_ms"))
