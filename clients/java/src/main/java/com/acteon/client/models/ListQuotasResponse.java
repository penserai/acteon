package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Response from listing quota policies.
 */
public class ListQuotasResponse {
    @JsonProperty("quotas")
    private List<QuotaPolicy> quotas;

    @JsonProperty("count")
    private int count;

    public List<QuotaPolicy> getQuotas() { return quotas; }
    public int getCount() { return count; }
}
