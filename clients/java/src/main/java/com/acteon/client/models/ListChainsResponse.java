package com.acteon.client.models;

import java.util.List;

/**
 * Response from listing chain executions.
 */
public class ListChainsResponse {
    private List<ChainSummary> chains;

    public List<ChainSummary> getChains() { return chains; }
    public void setChains(List<ChainSummary> chains) { this.chains = chains; }
}
