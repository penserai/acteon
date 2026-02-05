#!/usr/bin/env python3
"""Test script for the Python Acteon client.

Usage:
    ACTEON_URL=http://localhost:8080 python test_python_client.py
"""

import os
import sys
import uuid

# Add the client to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../../clients/python"))

from acteon_client import ActeonClient, Action, AuditQuery


def main():
    base_url = os.environ.get("ACTEON_URL", "http://localhost:8080")
    print(f"Python Client Test - connecting to {base_url}")
    print("=" * 60)

    client = ActeonClient(base_url)
    results = {"passed": 0, "failed": 0}

    def test(name: str, fn):
        try:
            fn()
            print(f"  [PASS] {name}")
            results["passed"] += 1
        except Exception as e:
            print(f"  [FAIL] {name}: {e}")
            results["failed"] += 1

    # Test: Health check
    def test_health():
        assert client.health(), "Health check failed"

    test("health()", test_health)

    # Test: Single dispatch
    dispatched_id = None

    def test_dispatch():
        nonlocal dispatched_id
        action = Action(
            namespace="test",
            tenant="python-client",
            provider="email",
            action_type="send_notification",
            payload={"to": "test@example.com", "subject": "Python test"},
        )
        dispatched_id = action.id
        outcome = client.dispatch(action)
        assert outcome.outcome_type in [
            "executed",
            "deduplicated",
            "suppressed",
            "rerouted",
            "throttled",
            "failed",
        ], f"Unexpected outcome: {outcome.outcome_type}"

    test("dispatch()", test_dispatch)

    # Test: Batch dispatch
    def test_batch_dispatch():
        actions = [
            Action(
                namespace="test",
                tenant="python-client",
                provider="email",
                action_type="batch_test",
                payload={"seq": i},
            )
            for i in range(3)
        ]
        results_list = client.dispatch_batch(actions)
        assert len(results_list) == 3, f"Expected 3 results, got {len(results_list)}"

    test("dispatch_batch()", test_batch_dispatch)

    # Test: List rules
    def test_list_rules():
        rules = client.list_rules()
        assert isinstance(rules, list), "Expected list of rules"

    test("list_rules()", test_list_rules)

    # Test: Deduplication
    def test_deduplication():
        dedup_key = f"python-dedup-{uuid.uuid4()}"
        action1 = Action(
            namespace="test",
            tenant="python-client",
            provider="email",
            action_type="dedup_test",
            payload={"msg": "first"},
            dedup_key=dedup_key,
        )
        action2 = Action(
            namespace="test",
            tenant="python-client",
            provider="email",
            action_type="dedup_test",
            payload={"msg": "second"},
            dedup_key=dedup_key,
        )
        outcome1 = client.dispatch(action1)
        outcome2 = client.dispatch(action2)
        # Second should be deduplicated (if dedup rule is active)
        # or executed (if no dedup rule)
        assert outcome1.outcome_type in ["executed", "failed"]
        # outcome2 could be deduplicated or executed depending on rules

    test("deduplication", test_deduplication)

    # Test: Query audit (may be empty if audit disabled)
    def test_query_audit():
        query = AuditQuery(tenant="python-client", limit=10)
        page = client.query_audit(query)
        assert hasattr(page, "total"), "Expected AuditPage with total"
        assert hasattr(page, "records"), "Expected AuditPage with records"

    test("query_audit()", test_query_audit)

    # Summary
    print("=" * 60)
    total = results["passed"] + results["failed"]
    print(f"Results: {results['passed']}/{total} passed")

    return 0 if results["failed"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
