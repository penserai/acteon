package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * A unique combination of coverage dimensions.
 */
public class CoverageKey {
    private String namespace;
    private String tenant;
    private String provider;
    @JsonProperty("action_type")
    private String actionType;

    public CoverageKey() {}

    public CoverageKey(String namespace, String tenant, String provider, String actionType) {
        this.namespace = namespace;
        this.tenant = tenant;
        this.provider = provider;
        this.actionType = actionType;
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getActionType() { return actionType; }
    public void setActionType(String actionType) { this.actionType = actionType; }
}
