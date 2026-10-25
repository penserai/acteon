package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Request body for hash chain verification.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class VerifyHashChainRequest {

    @JsonProperty("from")
    private String from;
    @JsonProperty("to")
    private String to;

    public VerifyHashChainRequest() {

    }

    public String getFrom() { return from; }
    public void setFrom(String from) { this.from = from; }

    public String getTo() { return to; }
    public void setTo(String to) { this.to = to; }
}
