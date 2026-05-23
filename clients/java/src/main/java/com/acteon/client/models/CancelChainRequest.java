package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Request body for cancelling a chain execution.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class CancelChainRequest {
    private String namespace;
    private String tenant;
    private String reason;

    @JsonProperty("cancelled_by")
    private String cancelledBy;

    public CancelChainRequest() {}

    public CancelChainRequest(String namespace, String tenant) {
        this.namespace = namespace;
        this.tenant = tenant;
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getReason() { return reason; }
    public void setReason(String reason) { this.reason = reason; }

    public String getCancelledBy() { return cancelledBy; }
    public void setCancelledBy(String cancelledBy) { this.cancelledBy = cancelledBy; }
}
