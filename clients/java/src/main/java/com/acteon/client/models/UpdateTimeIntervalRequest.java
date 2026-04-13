package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/** Partial update for a time interval. {@code null} fields are unchanged. */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class UpdateTimeIntervalRequest {
    @JsonProperty("time_ranges")
    private final List<TimeRange> timeRanges;

    @JsonProperty("location")
    private final String location;

    @JsonProperty("description")
    private final String description;

    public UpdateTimeIntervalRequest(
            List<TimeRange> timeRanges, String location, String description) {
        this.timeRanges = timeRanges;
        this.location = location;
        this.description = description;
    }
}
