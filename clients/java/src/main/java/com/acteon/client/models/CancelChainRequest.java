package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Request body for cancelling a chain execution.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class CancelChainRequest {

    private String reason;

    @JsonProperty("cancelled_by")
    private String cancelledBy;

    public CancelChainRequest() {}

    public String getReason() { return reason; }
    public void setReason(String reason) { this.reason = reason; }

    public String getCancelledBy() { return cancelledBy; }
    public void setCancelledBy(String cancelledBy) { this.cancelledBy = cancelledBy; }
}
