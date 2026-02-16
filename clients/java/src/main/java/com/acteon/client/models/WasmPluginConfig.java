package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Configuration for a WASM plugin.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class WasmPluginConfig {
    @JsonProperty("memory_limit_bytes")
    private Long memoryLimitBytes;

    @JsonProperty("timeout_ms")
    private Long timeoutMs;

    @JsonProperty("allowed_host_functions")
    private List<String> allowedHostFunctions;

    public WasmPluginConfig() {}

    public Long getMemoryLimitBytes() { return memoryLimitBytes; }
    public void setMemoryLimitBytes(Long memoryLimitBytes) { this.memoryLimitBytes = memoryLimitBytes; }

    public Long getTimeoutMs() { return timeoutMs; }
    public void setTimeoutMs(Long timeoutMs) { this.timeoutMs = timeoutMs; }

    public List<String> getAllowedHostFunctions() { return allowedHostFunctions; }
    public void setAllowedHostFunctions(List<String> allowedHostFunctions) { this.allowedHostFunctions = allowedHostFunctions; }

    @SuppressWarnings("unchecked")
    public static WasmPluginConfig fromMap(java.util.Map<String, Object> data) {
        WasmPluginConfig config = new WasmPluginConfig();
        if (data.containsKey("memory_limit_bytes") && data.get("memory_limit_bytes") != null) {
            config.memoryLimitBytes = ((Number) data.get("memory_limit_bytes")).longValue();
        }
        if (data.containsKey("timeout_ms") && data.get("timeout_ms") != null) {
            config.timeoutMs = ((Number) data.get("timeout_ms")).longValue();
        }
        if (data.containsKey("allowed_host_functions") && data.get("allowed_host_functions") != null) {
            config.allowedHostFunctions = (List<String>) data.get("allowed_host_functions");
        }
        return config;
    }
}
