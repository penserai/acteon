package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonCreator;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Collections;
import java.util.List;

public class ListTimeIntervalsResponse {
    @JsonProperty("time_intervals")
    private final List<TimeInterval> timeIntervals;

    @JsonProperty("count")
    private final int count;

    @JsonCreator
    public ListTimeIntervalsResponse(
            @JsonProperty("time_intervals") List<TimeInterval> timeIntervals,
            @JsonProperty("count") int count) {
        this.timeIntervals = timeIntervals == null ? Collections.emptyList() : timeIntervals;
        this.count = count;
    }

    public List<TimeInterval> getTimeIntervals() {
        return timeIntervals;
    }

    public int getCount() {
        return count;
    }
}
