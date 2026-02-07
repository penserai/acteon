"""Tests for webhook helpers in acteon_client.models."""

import unittest
from acteon_client.models import Action, WebhookPayload, create_webhook_action


class TestWebhookPayload(unittest.TestCase):
    """Tests for the WebhookPayload dataclass."""

    def test_creation_with_defaults(self):
        """WebhookPayload should default method to POST and headers to None."""
        payload = WebhookPayload(
            url="https://example.com/hook",
            body={"key": "value"},
        )
        self.assertEqual(payload.url, "https://example.com/hook")
        self.assertEqual(payload.body, {"key": "value"})
        self.assertEqual(payload.method, "POST")
        self.assertIsNone(payload.headers)

    def test_creation_with_explicit_values(self):
        """WebhookPayload should accept explicit method and headers."""
        headers = {"Authorization": "Bearer token123"}
        payload = WebhookPayload(
            url="https://example.com/hook",
            body={"msg": "hello"},
            method="PUT",
            headers=headers,
        )
        self.assertEqual(payload.method, "PUT")
        self.assertEqual(payload.headers, {"Authorization": "Bearer token123"})

    def test_to_dict_without_headers(self):
        """to_dict() should omit headers when they are None."""
        payload = WebhookPayload(
            url="https://example.com/hook",
            body={"alert": True},
        )
        result = payload.to_dict()
        self.assertEqual(result, {
            "url": "https://example.com/hook",
            "method": "POST",
            "body": {"alert": True},
        })
        self.assertNotIn("headers", result)

    def test_to_dict_with_headers(self):
        """to_dict() should include headers when they are provided."""
        headers = {"X-Custom": "abc", "Content-Type": "application/json"}
        payload = WebhookPayload(
            url="https://example.com/hook",
            body={"data": 42},
            method="PATCH",
            headers=headers,
        )
        result = payload.to_dict()
        self.assertEqual(result, {
            "url": "https://example.com/hook",
            "method": "PATCH",
            "body": {"data": 42},
            "headers": {"X-Custom": "abc", "Content-Type": "application/json"},
        })

    def test_to_dict_with_empty_headers(self):
        """to_dict() should omit headers when the dict is empty (falsy)."""
        payload = WebhookPayload(
            url="https://example.com/hook",
            body={},
            headers={},
        )
        result = payload.to_dict()
        self.assertNotIn("headers", result)

    def test_to_dict_with_empty_body(self):
        """to_dict() should include body even when it is an empty dict."""
        payload = WebhookPayload(
            url="https://example.com/hook",
            body={},
        )
        result = payload.to_dict()
        self.assertEqual(result["body"], {})

    def test_to_dict_with_nested_body(self):
        """to_dict() should preserve nested structures in body."""
        body = {"user": {"name": "Alice", "roles": ["admin", "editor"]}}
        payload = WebhookPayload(
            url="https://example.com/hook",
            body=body,
        )
        result = payload.to_dict()
        self.assertEqual(result["body"]["user"]["name"], "Alice")
        self.assertEqual(result["body"]["user"]["roles"], ["admin", "editor"])


class TestCreateWebhookAction(unittest.TestCase):
    """Tests for the create_webhook_action convenience function."""

    def test_minimal_args(self):
        """create_webhook_action with only required args should use defaults."""
        action = create_webhook_action(
            namespace="ns",
            tenant="t1",
            url="https://example.com/hook",
            body={"event": "fired"},
        )
        self.assertIsInstance(action, Action)
        self.assertEqual(action.namespace, "ns")
        self.assertEqual(action.tenant, "t1")
        self.assertEqual(action.provider, "webhook")
        self.assertEqual(action.action_type, "webhook")
        self.assertEqual(action.payload["url"], "https://example.com/hook")
        self.assertEqual(action.payload["method"], "POST")
        self.assertEqual(action.payload["body"], {"event": "fired"})
        self.assertNotIn("headers", action.payload)
        self.assertIsNone(action.dedup_key)
        self.assertIsNone(action.metadata)

    def test_all_optional_args(self):
        """create_webhook_action should pass through all optional arguments."""
        action = create_webhook_action(
            namespace="alerts",
            tenant="tenant-42",
            url="https://hooks.example.com/endpoint",
            body={"severity": "critical"},
            method="PUT",
            headers={"Authorization": "Bearer secret"},
            action_type="custom_webhook",
            dedup_key="dedup-123",
            metadata={"env": "production", "region": "us-east"},
        )
        self.assertEqual(action.namespace, "alerts")
        self.assertEqual(action.tenant, "tenant-42")
        self.assertEqual(action.provider, "webhook")
        self.assertEqual(action.action_type, "custom_webhook")
        self.assertEqual(action.payload["url"], "https://hooks.example.com/endpoint")
        self.assertEqual(action.payload["method"], "PUT")
        self.assertEqual(action.payload["body"], {"severity": "critical"})
        self.assertEqual(action.payload["headers"], {"Authorization": "Bearer secret"})
        self.assertEqual(action.dedup_key, "dedup-123")
        self.assertEqual(action.metadata, {"env": "production", "region": "us-east"})

    def test_provider_is_always_webhook(self):
        """The provider field must always be 'webhook', regardless of inputs."""
        action = create_webhook_action(
            namespace="ns",
            tenant="t1",
            url="https://example.com/hook",
            body={},
            action_type="anything",
        )
        self.assertEqual(action.provider, "webhook")

    def test_default_method_is_post(self):
        """The default HTTP method in the payload should be POST."""
        action = create_webhook_action(
            namespace="ns",
            tenant="t1",
            url="https://example.com/hook",
            body={},
        )
        self.assertEqual(action.payload["method"], "POST")

    def test_default_action_type_is_webhook(self):
        """The default action_type should be 'webhook'."""
        action = create_webhook_action(
            namespace="ns",
            tenant="t1",
            url="https://example.com/hook",
            body={},
        )
        self.assertEqual(action.action_type, "webhook")

    def test_custom_method(self):
        """A custom HTTP method should be reflected in the payload."""
        action = create_webhook_action(
            namespace="ns",
            tenant="t1",
            url="https://example.com/hook",
            body={"data": 1},
            method="DELETE",
        )
        self.assertEqual(action.payload["method"], "DELETE")

    def test_action_has_auto_generated_id(self):
        """Each action should receive a unique auto-generated id."""
        action1 = create_webhook_action(
            namespace="ns", tenant="t1",
            url="https://example.com/hook", body={},
        )
        action2 = create_webhook_action(
            namespace="ns", tenant="t1",
            url="https://example.com/hook", body={},
        )
        self.assertIsNotNone(action1.id)
        self.assertIsNotNone(action2.id)
        self.assertNotEqual(action1.id, action2.id)

    def test_action_has_created_at(self):
        """Each action should have a created_at timestamp."""
        action = create_webhook_action(
            namespace="ns", tenant="t1",
            url="https://example.com/hook", body={},
        )
        self.assertIsNotNone(action.created_at)

    def test_to_dict_round_trip(self):
        """The action's to_dict() should include all webhook payload fields."""
        action = create_webhook_action(
            namespace="ns",
            tenant="t1",
            url="https://example.com/hook",
            body={"key": "val"},
            headers={"X-Trace": "abc123"},
            dedup_key="dup-1",
            metadata={"team": "platform"},
        )
        d = action.to_dict()
        self.assertEqual(d["namespace"], "ns")
        self.assertEqual(d["tenant"], "t1")
        self.assertEqual(d["provider"], "webhook")
        self.assertEqual(d["action_type"], "webhook")
        self.assertEqual(d["payload"]["url"], "https://example.com/hook")
        self.assertEqual(d["payload"]["method"], "POST")
        self.assertEqual(d["payload"]["body"], {"key": "val"})
        self.assertEqual(d["payload"]["headers"], {"X-Trace": "abc123"})
        self.assertEqual(d["dedup_key"], "dup-1")
        self.assertEqual(d["metadata"], {"labels": {"team": "platform"}})

    def test_headers_omitted_when_none(self):
        """When headers are not provided, they should not appear in the payload."""
        action = create_webhook_action(
            namespace="ns", tenant="t1",
            url="https://example.com/hook", body={},
        )
        self.assertNotIn("headers", action.payload)


if __name__ == "__main__":
    unittest.main()
