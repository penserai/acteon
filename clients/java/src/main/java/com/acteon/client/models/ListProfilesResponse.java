package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Response from listing template profiles.
 */
public class ListProfilesResponse {
    @JsonProperty("profiles")
    private List<TemplateProfileInfo> profiles;

    @JsonProperty("count")
    private int count;

    public List<TemplateProfileInfo> getProfiles() { return profiles; }
    public int getCount() { return count; }
}
