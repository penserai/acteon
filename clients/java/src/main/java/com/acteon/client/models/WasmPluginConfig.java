package com.acteon.client.models;

import java.util.List;

/**
 * Configuration for a WASM plugin.
 */
public class WasmPluginConfig {
    private Long memoryLimitBytes;
    private Long timeoutMs;
    private List<String> allowedHostFunctions;

    public WasmPluginConfig() {}

    public Long getMemoryLimitBytes() { return memoryLimitBytes; }
    public void setMemoryLimitBytes(Long memoryLimitBytes) { this.memoryLimitBytes = memoryLimitBytes; }

    public Long getTimeoutMs() { return timeoutMs; }
    public void setTimeoutMs(Long timeoutMs) { this.timeoutMs = timeoutMs; }

    public List<String> getAllowedHostFunctions() { return allowedHostFunctions; }
    public void setAllowedHostFunctions(List<String> allowedHostFunctions) { this.allowedHostFunctions = allowedHostFunctions; }
}
