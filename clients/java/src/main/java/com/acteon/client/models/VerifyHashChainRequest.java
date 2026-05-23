package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Request body for hash chain verification.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class VerifyHashChainRequest {
    private String namespace;
    private String tenant;
    @JsonProperty("from")
    private String from;
    @JsonProperty("to")
    private String to;

    public VerifyHashChainRequest(String namespace, String tenant) {
        this.namespace = namespace;
        this.tenant = tenant;
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getFrom() { return from; }
    public void setFrom(String from) { this.from = from; }

    public String getTo() { return to; }
    public void setTo(String to) { this.to = to; }
}
