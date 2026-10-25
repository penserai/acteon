package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/** Request body for {@code POST /v1/time-intervals}. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class CreateTimeIntervalRequest {
    @JsonProperty("name")
    private final String name;

    @JsonProperty("time_ranges")
    private final List<TimeRange> timeRanges;

    @JsonProperty("location")
    private final String location;

    @JsonProperty("description")
    private final String description;

    public CreateTimeIntervalRequest(String name, List<TimeRange> timeRanges, String location, String description) {
        this.name = name;

        this.timeRanges = timeRanges;
        this.location = location;
        this.description = description;
    }

    public CreateTimeIntervalRequest(String name, List<TimeRange> timeRanges) {
        this(name, timeRanges, null, null);
    }
}
