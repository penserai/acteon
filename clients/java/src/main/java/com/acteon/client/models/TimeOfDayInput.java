package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonCreator;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Time-of-day window expressed as `HH:MM` strings (24-hour clock).
 * `24:00` is allowed as an end-of-day sentinel.
 */
public class TimeOfDayInput {
    @JsonProperty("start")
    private final String start;

    @JsonProperty("end")
    private final String end;

    @JsonCreator
    public TimeOfDayInput(
            @JsonProperty("start") String start,
            @JsonProperty("end") String end) {
        this.start = start;
        this.end = end;
    }

    public String getStart() {
        return start;
    }

    public String getEnd() {
        return end;
    }
}
