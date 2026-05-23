package com.acteon.client.models;

import java.util.List;

/**
 * Response from listing WASM plugins.
 */
public class ListPluginsResponse {
    private List<WasmPlugin> plugins;
    private int count;

    public List<WasmPlugin> getPlugins() { return plugins; }
    public void setPlugins(List<WasmPlugin> plugins) { this.plugins = plugins; }

    public int getCount() { return count; }
    public void setCount(int count) { this.count = count; }
}
