package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Request to register a new WASM plugin.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class RegisterPluginRequest {
    @JsonProperty("name")
    private String name;

    @JsonProperty("description")
    private String description;

    @JsonProperty("wasm_bytes")
    private String wasmBytes;

    @JsonProperty("wasm_path")
    private String wasmPath;

    @JsonProperty("config")
    private WasmPluginConfig config;

    public RegisterPluginRequest() {}

    public RegisterPluginRequest(String name) {
        this.name = name;
    }

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public String getDescription() { return description; }
    public void setDescription(String description) { this.description = description; }

    public String getWasmBytes() { return wasmBytes; }
    public void setWasmBytes(String wasmBytes) { this.wasmBytes = wasmBytes; }

    public String getWasmPath() { return wasmPath; }
    public void setWasmPath(String wasmPath) { this.wasmPath = wasmPath; }

    public WasmPluginConfig getConfig() { return config; }
    public void setConfig(WasmPluginConfig config) { this.config = config; }
}
