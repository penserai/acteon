package com.acteon.client.models;

import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;

/**
 * Response from listing provider health.
 */
public class ListProviderHealthResponse {
    private List<ProviderHealthStatus> providers;

    public List<ProviderHealthStatus> getProviders() { return providers; }
    public void setProviders(List<ProviderHealthStatus> providers) { this.providers = providers; }

    @SuppressWarnings("unchecked")
    public static ListProviderHealthResponse fromMap(Map<String, Object> data) {
        ListProviderHealthResponse response = new ListProviderHealthResponse();
        List<Map<String, Object>> providersData = (List<Map<String, Object>>) data.get("providers");
        response.providers = providersData.stream()
                .map(ProviderHealthStatus::fromMap)
                .collect(Collectors.toList());
        return response;
    }
}
