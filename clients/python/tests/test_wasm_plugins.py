"""Tests for WASM plugin models in acteon_client.models."""

import unittest
from acteon_client.models import (
    WasmPluginConfig,
    WasmPlugin,
    RegisterPluginRequest,
    ListPluginsResponse,
    PluginInvocationRequest,
    PluginInvocationResponse,
)


class TestWasmPluginConfig(unittest.TestCase):
    """Tests for the WasmPluginConfig dataclass."""

    def test_from_dict_complete(self):
        """WasmPluginConfig.from_dict() should parse all fields."""
        data = {
            "memory_limit_bytes": 16777216,
            "timeout_ms": 100,
            "allowed_host_functions": ["log", "time"],
        }
        config = WasmPluginConfig.from_dict(data)
        self.assertEqual(config.memory_limit_bytes, 16777216)
        self.assertEqual(config.timeout_ms, 100)
        self.assertEqual(config.allowed_host_functions, ["log", "time"])

    def test_from_dict_minimal(self):
        """WasmPluginConfig.from_dict() should handle missing fields."""
        config = WasmPluginConfig.from_dict({})
        self.assertIsNone(config.memory_limit_bytes)
        self.assertIsNone(config.timeout_ms)
        self.assertIsNone(config.allowed_host_functions)

    def test_to_dict(self):
        """WasmPluginConfig.to_dict() should only include non-None fields."""
        config = WasmPluginConfig(memory_limit_bytes=1024, timeout_ms=50)
        d = config.to_dict()
        self.assertEqual(d["memory_limit_bytes"], 1024)
        self.assertEqual(d["timeout_ms"], 50)
        self.assertNotIn("allowed_host_functions", d)

    def test_to_dict_empty(self):
        """WasmPluginConfig.to_dict() should return empty dict when all None."""
        config = WasmPluginConfig()
        self.assertEqual(config.to_dict(), {})


class TestWasmPlugin(unittest.TestCase):
    """Tests for the WasmPlugin dataclass."""

    def test_from_dict_complete(self):
        """WasmPlugin.from_dict() should parse all fields."""
        data = {
            "name": "my-plugin",
            "description": "A test plugin",
            "status": "active",
            "enabled": True,
            "config": {
                "memory_limit_bytes": 16777216,
                "timeout_ms": 100,
            },
            "created_at": "2026-02-15T00:00:00Z",
            "updated_at": "2026-02-15T01:00:00Z",
            "invocation_count": 42,
        }
        plugin = WasmPlugin.from_dict(data)
        self.assertEqual(plugin.name, "my-plugin")
        self.assertEqual(plugin.description, "A test plugin")
        self.assertEqual(plugin.status, "active")
        self.assertTrue(plugin.enabled)
        self.assertIsNotNone(plugin.config)
        self.assertEqual(plugin.config.memory_limit_bytes, 16777216)
        self.assertEqual(plugin.created_at, "2026-02-15T00:00:00Z")
        self.assertEqual(plugin.invocation_count, 42)

    def test_from_dict_minimal(self):
        """WasmPlugin.from_dict() should handle optional fields."""
        data = {
            "name": "minimal-plugin",
            "status": "active",
            "created_at": "2026-02-15T00:00:00Z",
            "updated_at": "2026-02-15T00:00:00Z",
        }
        plugin = WasmPlugin.from_dict(data)
        self.assertEqual(plugin.name, "minimal-plugin")
        self.assertIsNone(plugin.description)
        self.assertIsNone(plugin.config)
        self.assertEqual(plugin.invocation_count, 0)


class TestRegisterPluginRequest(unittest.TestCase):
    """Tests for the RegisterPluginRequest dataclass."""

    def test_to_dict_minimal(self):
        """RegisterPluginRequest.to_dict() with only name."""
        req = RegisterPluginRequest(name="test-plugin")
        d = req.to_dict()
        self.assertEqual(d, {"name": "test-plugin"})

    def test_to_dict_complete(self):
        """RegisterPluginRequest.to_dict() with all fields."""
        config = WasmPluginConfig(memory_limit_bytes=1024, timeout_ms=50)
        req = RegisterPluginRequest(
            name="test-plugin",
            description="A test",
            wasm_path="/plugins/test.wasm",
            config=config,
        )
        d = req.to_dict()
        self.assertEqual(d["name"], "test-plugin")
        self.assertEqual(d["description"], "A test")
        self.assertEqual(d["wasm_path"], "/plugins/test.wasm")
        self.assertNotIn("wasm_bytes", d)
        self.assertIn("config", d)
        self.assertEqual(d["config"]["memory_limit_bytes"], 1024)


class TestListPluginsResponse(unittest.TestCase):
    """Tests for the ListPluginsResponse dataclass."""

    def test_from_dict_empty(self):
        """ListPluginsResponse.from_dict() should handle empty list."""
        data = {"plugins": [], "count": 0}
        response = ListPluginsResponse.from_dict(data)
        self.assertEqual(response.plugins, [])
        self.assertEqual(response.count, 0)

    def test_from_dict_multiple(self):
        """ListPluginsResponse.from_dict() should parse multiple plugins."""
        data = {
            "plugins": [
                {
                    "name": "plugin-a",
                    "status": "active",
                    "enabled": True,
                    "created_at": "2026-02-15T00:00:00Z",
                    "updated_at": "2026-02-15T00:00:00Z",
                },
                {
                    "name": "plugin-b",
                    "status": "disabled",
                    "enabled": False,
                    "created_at": "2026-02-15T00:00:00Z",
                    "updated_at": "2026-02-15T00:00:00Z",
                },
            ],
            "count": 2,
        }
        response = ListPluginsResponse.from_dict(data)
        self.assertEqual(len(response.plugins), 2)
        self.assertEqual(response.count, 2)
        self.assertEqual(response.plugins[0].name, "plugin-a")
        self.assertTrue(response.plugins[0].enabled)
        self.assertEqual(response.plugins[1].name, "plugin-b")
        self.assertFalse(response.plugins[1].enabled)


class TestPluginInvocationRequest(unittest.TestCase):
    """Tests for the PluginInvocationRequest dataclass."""

    def test_to_dict_minimal(self):
        """PluginInvocationRequest.to_dict() with only input."""
        req = PluginInvocationRequest(input={"key": "value"})
        d = req.to_dict()
        self.assertEqual(d, {"input": {"key": "value"}})

    def test_to_dict_with_function(self):
        """PluginInvocationRequest.to_dict() with custom function."""
        req = PluginInvocationRequest(input={"key": "value"}, function="custom_fn")
        d = req.to_dict()
        self.assertEqual(d["function"], "custom_fn")
        self.assertEqual(d["input"], {"key": "value"})


class TestPluginInvocationResponse(unittest.TestCase):
    """Tests for the PluginInvocationResponse dataclass."""

    def test_from_dict_complete(self):
        """PluginInvocationResponse.from_dict() should parse all fields."""
        data = {
            "verdict": True,
            "message": "all good",
            "metadata": {"score": 0.95},
            "duration_ms": 12.5,
        }
        resp = PluginInvocationResponse.from_dict(data)
        self.assertTrue(resp.verdict)
        self.assertEqual(resp.message, "all good")
        self.assertEqual(resp.metadata, {"score": 0.95})
        self.assertEqual(resp.duration_ms, 12.5)

    def test_from_dict_minimal(self):
        """PluginInvocationResponse.from_dict() should handle missing fields."""
        data = {"verdict": False}
        resp = PluginInvocationResponse.from_dict(data)
        self.assertFalse(resp.verdict)
        self.assertIsNone(resp.message)
        self.assertIsNone(resp.metadata)
        self.assertIsNone(resp.duration_ms)


if __name__ == "__main__":
    unittest.main()
