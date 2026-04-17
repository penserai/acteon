package com.acteon.client.models;

import com.acteon.client.JsonMapper;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

class WasmPluginTest {
    private static final ObjectMapper MAPPER = JsonMapper.build();

    @Test
    void wasmPluginConfigDeserializesAllFields() throws Exception {
        String json = """
            {
              "memory_limit_bytes": 16777216,
              "timeout_ms": 100,
              "allowed_host_functions": ["log", "time"]
            }
            """;

        WasmPluginConfig config = MAPPER.readValue(json, WasmPluginConfig.class);

        assertEquals(16_777_216L, config.getMemoryLimitBytes());
        assertEquals(100L, config.getTimeoutMs());
        assertEquals(2, config.getAllowedHostFunctions().size());
        assertEquals("log", config.getAllowedHostFunctions().get(0));
        assertEquals("time", config.getAllowedHostFunctions().get(1));
    }

    @Test
    void wasmPluginConfigDeserializesEmpty() throws Exception {
        WasmPluginConfig config = MAPPER.readValue("{}", WasmPluginConfig.class);

        assertNull(config.getMemoryLimitBytes());
        assertNull(config.getTimeoutMs());
        assertNull(config.getAllowedHostFunctions());
    }

    @Test
    void wasmPluginDeserializesAllFields() throws Exception {
        String json = """
            {
              "name": "my-plugin",
              "description": "A test plugin",
              "status": "active",
              "enabled": true,
              "config": {
                "memory_limit_bytes": 16777216,
                "timeout_ms": 100
              },
              "created_at": "2026-02-15T00:00:00Z",
              "updated_at": "2026-02-15T01:00:00Z",
              "invocation_count": 42
            }
            """;

        WasmPlugin plugin = MAPPER.readValue(json, WasmPlugin.class);

        assertEquals("my-plugin", plugin.getName());
        assertEquals("A test plugin", plugin.getDescription());
        assertEquals("active", plugin.getStatus());
        assertTrue(plugin.isEnabled());
        assertNotNull(plugin.getConfig());
        assertEquals(16_777_216L, plugin.getConfig().getMemoryLimitBytes());
        assertEquals("2026-02-15T00:00:00Z", plugin.getCreatedAt());
        assertEquals(42, plugin.getInvocationCount());
    }

    @Test
    void wasmPluginDefaultsEnabledToTrueWhenAbsent() throws Exception {
        // The pre-Jackson-migration fromMap treated absent "enabled"
        // as enabled. Preserve that contract via a field initializer
        // on WasmPlugin.
        String json = """
            {
              "name": "minimal-plugin",
              "status": "active",
              "created_at": "2026-02-15T00:00:00Z",
              "updated_at": "2026-02-15T00:00:00Z"
            }
            """;

        WasmPlugin plugin = MAPPER.readValue(json, WasmPlugin.class);

        assertEquals("minimal-plugin", plugin.getName());
        assertNull(plugin.getDescription());
        assertNull(plugin.getConfig());
        assertEquals(0, plugin.getInvocationCount());
        assertTrue(plugin.isEnabled(), "missing 'enabled' should default to true");
    }

    @Test
    void listPluginsResponseDeserializesItems() throws Exception {
        String json = """
            {
              "plugins": [
                {
                  "name": "plugin-a",
                  "status": "active",
                  "enabled": true,
                  "created_at": "2026-02-15T00:00:00Z",
                  "updated_at": "2026-02-15T00:00:00Z"
                },
                {
                  "name": "plugin-b",
                  "status": "disabled",
                  "enabled": false,
                  "created_at": "2026-02-15T00:00:00Z",
                  "updated_at": "2026-02-15T00:00:00Z"
                }
              ],
              "count": 2
            }
            """;

        ListPluginsResponse response = MAPPER.readValue(json, ListPluginsResponse.class);

        assertEquals(2, response.getPlugins().size());
        assertEquals(2, response.getCount());
        assertEquals("plugin-a", response.getPlugins().get(0).getName());
        assertTrue(response.getPlugins().get(0).isEnabled());
        assertEquals("plugin-b", response.getPlugins().get(1).getName());
        assertFalse(response.getPlugins().get(1).isEnabled());
    }

    @Test
    void listPluginsResponseDeserializesEmpty() throws Exception {
        String json = """
            {"plugins": [], "count": 0}
            """;

        ListPluginsResponse response = MAPPER.readValue(json, ListPluginsResponse.class);

        assertEquals(0, response.getPlugins().size());
        assertEquals(0, response.getCount());
    }

    @Test
    void pluginInvocationResponseDeserializesAllFields() throws Exception {
        String json = """
            {
              "verdict": true,
              "message": "all good",
              "metadata": {"score": 0.95},
              "duration_ms": 12.5
            }
            """;

        PluginInvocationResponse response = MAPPER.readValue(json, PluginInvocationResponse.class);

        assertTrue(response.isVerdict());
        assertEquals("all good", response.getMessage());
        assertNotNull(response.getMetadata());
        assertEquals(0.95, (Double) response.getMetadata().get("score"), 0.01);
        assertEquals(12.5, response.getDurationMs(), 0.01);
    }

    @Test
    void pluginInvocationResponseDeserializesMinimal() throws Exception {
        String json = """
            {"verdict": false}
            """;

        PluginInvocationResponse response = MAPPER.readValue(json, PluginInvocationResponse.class);

        assertFalse(response.isVerdict());
        assertNull(response.getMessage());
        assertNull(response.getMetadata());
        assertNull(response.getDurationMs());
    }
}
