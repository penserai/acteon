package com.acteon.client.models;

import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

class WasmPluginTest {

    @Test
    void testWasmPluginConfigFromMapComplete() {
        Map<String, Object> data = new HashMap<>();
        data.put("memory_limit_bytes", 16777216L);
        data.put("timeout_ms", 100L);
        List<String> funcs = new ArrayList<>();
        funcs.add("log");
        funcs.add("time");
        data.put("allowed_host_functions", funcs);

        WasmPluginConfig config = WasmPluginConfig.fromMap(data);

        assertEquals(16777216L, config.getMemoryLimitBytes());
        assertEquals(100L, config.getTimeoutMs());
        assertEquals(2, config.getAllowedHostFunctions().size());
        assertEquals("log", config.getAllowedHostFunctions().get(0));
        assertEquals("time", config.getAllowedHostFunctions().get(1));
    }

    @Test
    void testWasmPluginConfigFromMapMinimal() {
        WasmPluginConfig config = WasmPluginConfig.fromMap(new HashMap<>());

        assertNull(config.getMemoryLimitBytes());
        assertNull(config.getTimeoutMs());
        assertNull(config.getAllowedHostFunctions());
    }

    @Test
    void testWasmPluginFromMapComplete() {
        Map<String, Object> configData = new HashMap<>();
        configData.put("memory_limit_bytes", 16777216L);
        configData.put("timeout_ms", 100L);

        Map<String, Object> data = new HashMap<>();
        data.put("name", "my-plugin");
        data.put("description", "A test plugin");
        data.put("status", "active");
        data.put("enabled", true);
        data.put("config", configData);
        data.put("created_at", "2026-02-15T00:00:00Z");
        data.put("updated_at", "2026-02-15T01:00:00Z");
        data.put("invocation_count", 42);

        WasmPlugin plugin = WasmPlugin.fromMap(data);

        assertEquals("my-plugin", plugin.getName());
        assertEquals("A test plugin", plugin.getDescription());
        assertEquals("active", plugin.getStatus());
        assertTrue(plugin.isEnabled());
        assertNotNull(plugin.getConfig());
        assertEquals(16777216L, plugin.getConfig().getMemoryLimitBytes());
        assertEquals("2026-02-15T00:00:00Z", plugin.getCreatedAt());
        assertEquals(42, plugin.getInvocationCount());
    }

    @Test
    void testWasmPluginFromMapMinimal() {
        Map<String, Object> data = new HashMap<>();
        data.put("name", "minimal-plugin");
        data.put("status", "active");
        data.put("created_at", "2026-02-15T00:00:00Z");
        data.put("updated_at", "2026-02-15T00:00:00Z");

        WasmPlugin plugin = WasmPlugin.fromMap(data);

        assertEquals("minimal-plugin", plugin.getName());
        assertNull(plugin.getDescription());
        assertNull(plugin.getConfig());
        assertEquals(0, plugin.getInvocationCount());
    }

    @SuppressWarnings("unchecked")
    @Test
    void testListPluginsResponseFromMap() {
        Map<String, Object> pluginA = new HashMap<>();
        pluginA.put("name", "plugin-a");
        pluginA.put("status", "active");
        pluginA.put("enabled", true);
        pluginA.put("created_at", "2026-02-15T00:00:00Z");
        pluginA.put("updated_at", "2026-02-15T00:00:00Z");

        Map<String, Object> pluginB = new HashMap<>();
        pluginB.put("name", "plugin-b");
        pluginB.put("status", "disabled");
        pluginB.put("enabled", false);
        pluginB.put("created_at", "2026-02-15T00:00:00Z");
        pluginB.put("updated_at", "2026-02-15T00:00:00Z");

        List<Map<String, Object>> plugins = new ArrayList<>();
        plugins.add(pluginA);
        plugins.add(pluginB);

        Map<String, Object> data = new HashMap<>();
        data.put("plugins", plugins);
        data.put("count", 2);

        ListPluginsResponse response = ListPluginsResponse.fromMap(data);

        assertEquals(2, response.getPlugins().size());
        assertEquals(2, response.getCount());
        assertEquals("plugin-a", response.getPlugins().get(0).getName());
        assertTrue(response.getPlugins().get(0).isEnabled());
        assertEquals("plugin-b", response.getPlugins().get(1).getName());
        assertFalse(response.getPlugins().get(1).isEnabled());
    }

    @Test
    void testListPluginsResponseFromMapEmpty() {
        Map<String, Object> data = new HashMap<>();
        data.put("plugins", new ArrayList<>());
        data.put("count", 0);

        ListPluginsResponse response = ListPluginsResponse.fromMap(data);

        assertEquals(0, response.getPlugins().size());
        assertEquals(0, response.getCount());
    }

    @Test
    void testPluginInvocationResponseFromMapComplete() {
        Map<String, Object> metadata = new HashMap<>();
        metadata.put("score", 0.95);

        Map<String, Object> data = new HashMap<>();
        data.put("verdict", true);
        data.put("message", "all good");
        data.put("metadata", metadata);
        data.put("duration_ms", 12.5);

        PluginInvocationResponse response = PluginInvocationResponse.fromMap(data);

        assertTrue(response.isVerdict());
        assertEquals("all good", response.getMessage());
        assertNotNull(response.getMetadata());
        assertEquals(0.95, (Double) response.getMetadata().get("score"), 0.01);
        assertEquals(12.5, response.getDurationMs(), 0.01);
    }

    @Test
    void testPluginInvocationResponseFromMapMinimal() {
        Map<String, Object> data = new HashMap<>();
        data.put("verdict", false);

        PluginInvocationResponse response = PluginInvocationResponse.fromMap(data);

        assertFalse(response.isVerdict());
        assertNull(response.getMessage());
        assertNull(response.getMetadata());
        assertNull(response.getDurationMs());
    }
}
