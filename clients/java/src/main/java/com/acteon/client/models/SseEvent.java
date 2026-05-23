package com.acteon.client.models;

/**
 * A parsed Server-Sent Event from the Acteon streaming endpoints.
 */
public class SseEvent {
    private String id;
    private String event;
    private String data;

    public SseEvent() {}

    public SseEvent(String id, String event, String data) {
        this.id = id;
        this.event = event;
        this.data = data;
    }

    /**
     * The event ID (from the {@code id:} field).
     */
    public String getId() { return id; }
    public void setId(String id) { this.id = id; }

    /**
     * The event type (from the {@code event:} field).
     */
    public String getEvent() { return event; }
    public void setEvent(String event) { this.event = event; }

    /**
     * The event data payload (from the {@code data:} field).
     */
    public String getData() { return data; }
    public void setData(String data) { this.data = data; }

    /**
     * Returns true if this is a keep-alive ping event.
     */
    public boolean isPing() {
        return "ping".equals(data) && event == null;
    }

    @Override
    public String toString() {
        return "SseEvent{id='" + id + "', event='" + event + "', data='" + data + "'}";
    }
}
