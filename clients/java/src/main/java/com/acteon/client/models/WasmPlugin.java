package com.acteon.client.models;

/**
 * A registered WASM plugin.
 */
public class WasmPlugin {
    private String name;
    private String description;
    private String status;
    /**
     * Default to {@code true} so a server response that omits
     * {@code "enabled"} preserves the pre-Jackson-migration behavior
     * (the old {@code fromMap} treated absent as enabled).
     */
    private boolean enabled = true;
    private WasmPluginConfig config;
    private String createdAt;
    private String updatedAt;
    private long invocationCount;

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public String getDescription() { return description; }
    public void setDescription(String description) { this.description = description; }

    public String getStatus() { return status; }
    public void setStatus(String status) { this.status = status; }

    public boolean isEnabled() { return enabled; }
    public void setEnabled(boolean enabled) { this.enabled = enabled; }

    public WasmPluginConfig getConfig() { return config; }
    public void setConfig(WasmPluginConfig config) { this.config = config; }

    public String getCreatedAt() { return createdAt; }
    public void setCreatedAt(String createdAt) { this.createdAt = createdAt; }

    public String getUpdatedAt() { return updatedAt; }
    public void setUpdatedAt(String updatedAt) { this.updatedAt = updatedAt; }

    public long getInvocationCount() { return invocationCount; }
    public void setInvocationCount(long invocationCount) { this.invocationCount = invocationCount; }
}
