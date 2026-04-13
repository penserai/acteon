package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonCreator;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Collections;
import java.util.List;

/**
 * A named, tenant-scoped recurring schedule. Rules reference time
 * intervals from {@code mute_time_intervals} or
 * {@code active_time_intervals} to gate dispatch by wall-clock time.
 */
public class TimeInterval {
    @JsonProperty("name")
    private final String name;

    @JsonProperty("namespace")
    private final String namespace;

    @JsonProperty("tenant")
    private final String tenant;

    @JsonProperty("time_ranges")
    private final List<TimeRange> timeRanges;

    @JsonProperty("location")
    private final String location;

    @JsonProperty("description")
    private final String description;

    @JsonProperty("created_by")
    private final String createdBy;

    @JsonProperty("created_at")
    private final String createdAt;

    @JsonProperty("updated_at")
    private final String updatedAt;

    @JsonProperty("matches_now")
    private final boolean matchesNow;

    @JsonCreator
    public TimeInterval(
            @JsonProperty("name") String name,
            @JsonProperty("namespace") String namespace,
            @JsonProperty("tenant") String tenant,
            @JsonProperty("time_ranges") List<TimeRange> timeRanges,
            @JsonProperty("location") String location,
            @JsonProperty("description") String description,
            @JsonProperty("created_by") String createdBy,
            @JsonProperty("created_at") String createdAt,
            @JsonProperty("updated_at") String updatedAt,
            @JsonProperty("matches_now") boolean matchesNow) {
        this.name = name;
        this.namespace = namespace;
        this.tenant = tenant;
        this.timeRanges = timeRanges == null ? Collections.emptyList() : timeRanges;
        this.location = location;
        this.description = description;
        this.createdBy = createdBy;
        this.createdAt = createdAt;
        this.updatedAt = updatedAt;
        this.matchesNow = matchesNow;
    }

    public String getName() {
        return name;
    }

    public String getNamespace() {
        return namespace;
    }

    public String getTenant() {
        return tenant;
    }

    public List<TimeRange> getTimeRanges() {
        return timeRanges;
    }

    public String getLocation() {
        return location;
    }

    public String getDescription() {
        return description;
    }

    public String getCreatedBy() {
        return createdBy;
    }

    public String getCreatedAt() {
        return createdAt;
    }

    public String getUpdatedAt() {
        return updatedAt;
    }

    public boolean isMatchesNow() {
        return matchesNow;
    }
}
