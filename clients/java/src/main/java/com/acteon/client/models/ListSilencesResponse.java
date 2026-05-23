package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Response from listing silences.
 */
public class ListSilencesResponse {
    @JsonProperty("silences")
    private List<Silence> silences;

    @JsonProperty("count")
    private int count;

    public List<Silence> getSilences() { return silences; }
    public int getCount() { return count; }
}
