package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonCreator;
import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Collections;
import java.util.List;

/**
 * One time range inside a {@link TimeInterval}. Each populated field is
 * ANDed with the others; an empty list means "any value matches".
 */
@JsonInclude(JsonInclude.Include.NON_EMPTY)
public class TimeRange {
    @JsonProperty("times")
    private final List<TimeOfDayInput> times;

    @JsonProperty("weekdays")
    private final List<NumericRange> weekdays;

    @JsonProperty("days_of_month")
    private final List<NumericRange> daysOfMonth;

    @JsonProperty("months")
    private final List<NumericRange> months;

    @JsonProperty("years")
    private final List<NumericRange> years;

    @JsonCreator
    public TimeRange(
            @JsonProperty("times") List<TimeOfDayInput> times,
            @JsonProperty("weekdays") List<NumericRange> weekdays,
            @JsonProperty("days_of_month") List<NumericRange> daysOfMonth,
            @JsonProperty("months") List<NumericRange> months,
            @JsonProperty("years") List<NumericRange> years) {
        this.times = times == null ? Collections.emptyList() : times;
        this.weekdays = weekdays == null ? Collections.emptyList() : weekdays;
        this.daysOfMonth = daysOfMonth == null ? Collections.emptyList() : daysOfMonth;
        this.months = months == null ? Collections.emptyList() : months;
        this.years = years == null ? Collections.emptyList() : years;
    }

    public List<TimeOfDayInput> getTimes() {
        return times;
    }

    public List<NumericRange> getWeekdays() {
        return weekdays;
    }

    public List<NumericRange> getDaysOfMonth() {
        return daysOfMonth;
    }

    public List<NumericRange> getMonths() {
        return months;
    }

    public List<NumericRange> getYears() {
        return years;
    }
}
