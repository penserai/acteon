package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Response from listing recurring actions.
 */
public class ListRecurringResponse {
    @JsonProperty("recurring_actions")
    private List<RecurringSummary> recurringActions;

    @JsonProperty("count")
    private int count;

    public List<RecurringSummary> getRecurringActions() { return recurringActions; }
    public int getCount() { return count; }
}
