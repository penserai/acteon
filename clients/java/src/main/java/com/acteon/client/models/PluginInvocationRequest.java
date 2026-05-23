package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Request to test-invoke a WASM plugin.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class PluginInvocationRequest {
    @JsonProperty("function")
    private String function;

    @JsonProperty("input")
    private Map<String, Object> input;

    public PluginInvocationRequest() {}

    public PluginInvocationRequest(Map<String, Object> input) {
        this.input = input;
    }

    public String getFunction() { return function; }
    public void setFunction(String function) { this.function = function; }

    public Map<String, Object> getInput() { return input; }
    public void setInput(Map<String, Object> input) { this.input = input; }
}
