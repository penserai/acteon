package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonCreator;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Inclusive integer range used for weekdays, days_of_month, months,
 * and years inside a {@link TimeRange}.
 */
public class NumericRange {
    @JsonProperty("start")
    private final int start;

    @JsonProperty("end")
    private final int end;

    @JsonCreator
    public NumericRange(
            @JsonProperty("start") int start,
            @JsonProperty("end") int end) {
        this.start = start;
        this.end = end;
    }

    public int getStart() {
        return start;
    }

    public int getEnd() {
        return end;
    }
}
