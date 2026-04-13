package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/** Request body for {@code POST /v1/time-intervals}. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class CreateTimeIntervalRequest {
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

    public CreateTimeIntervalRequest(
            String name,
            String namespace,
            String tenant,
            List<TimeRange> timeRanges,
            String location,
            String description) {
        this.name = name;
        this.namespace = namespace;
        this.tenant = tenant;
        this.timeRanges = timeRanges;
        this.location = location;
        this.description = description;
    }

    public CreateTimeIntervalRequest(
            String name, String namespace, String tenant, List<TimeRange> timeRanges) {
        this(name, namespace, tenant, timeRanges, null, null);
    }
}
