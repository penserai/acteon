package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * A registered WASM plugin.
 */
public class WasmPlugin {
    @JsonProperty("name")
    private String name;

    @JsonProperty("description")
    private String description;

    @JsonProperty("status")
    private String status;

    @JsonProperty("enabled")
    private boolean enabled;

    @JsonProperty("config")
    private WasmPluginConfig config;

    @JsonProperty("created_at")
    private String createdAt;

    @JsonProperty("updated_at")
    private String updatedAt;

    @JsonProperty("invocation_count")
    private long invocationCount;

    public String getName() { return name; }
    public String getDescription() { return description; }
    public String getStatus() { return status; }
    public boolean isEnabled() { return enabled; }
    public WasmPluginConfig getConfig() { return config; }
    public String getCreatedAt() { return createdAt; }
    public String getUpdatedAt() { return updatedAt; }
    public long getInvocationCount() { return invocationCount; }

    @SuppressWarnings("unchecked")
    public static WasmPlugin fromMap(Map<String, Object> data) {
        WasmPlugin plugin = new WasmPlugin();
        plugin.name = (String) data.get("name");
        plugin.description = (String) data.get("description");
        plugin.status = (String) data.get("status");
        plugin.enabled = data.containsKey("enabled") ? (Boolean) data.get("enabled") : true;
        plugin.createdAt = (String) data.get("created_at");
        plugin.updatedAt = (String) data.get("updated_at");
        if (data.containsKey("invocation_count") && data.get("invocation_count") != null) {
            plugin.invocationCount = ((Number) data.get("invocation_count")).longValue();
        }
        if (data.containsKey("config") && data.get("config") != null) {
            plugin.config = WasmPluginConfig.fromMap((Map<String, Object>) data.get("config"));
        }
        return plugin;
    }
}
