package com.acteon.client.models;

import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;

/**
 * Response from listing WASM plugins.
 */
public class ListPluginsResponse {
    private List<WasmPlugin> plugins;
    private int count;

    public List<WasmPlugin> getPlugins() { return plugins; }
    public int getCount() { return count; }

    @SuppressWarnings("unchecked")
    public static ListPluginsResponse fromMap(Map<String, Object> data) {
        ListPluginsResponse response = new ListPluginsResponse();
        List<Map<String, Object>> items = (List<Map<String, Object>>) data.get("plugins");
        response.plugins = items.stream()
            .map(WasmPlugin::fromMap)
            .collect(Collectors.toList());
        response.count = ((Number) data.get("count")).intValue();
        return response;
    }
}
