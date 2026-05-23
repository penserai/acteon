package com.acteon.client.models;

import java.util.List;

/**
 * Response from listing provider health.
 */
public class ListProviderHealthResponse {
    private List<ProviderHealthStatus> providers;

    public List<ProviderHealthStatus> getProviders() { return providers; }
    public void setProviders(List<ProviderHealthStatus> providers) { this.providers = providers; }
}
