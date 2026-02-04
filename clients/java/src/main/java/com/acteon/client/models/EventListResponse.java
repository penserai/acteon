package com.acteon.client.models;

import java.util.List;

/**
 * Response from listing events.
 */
public class EventListResponse {
    private List<EventState> events;
    private int count;

    public List<EventState> getEvents() {
        return events;
    }

    public void setEvents(List<EventState> events) {
        this.events = events;
    }

    public int getCount() {
        return count;
    }

    public void setCount(int count) {
        this.count = count;
    }
}
