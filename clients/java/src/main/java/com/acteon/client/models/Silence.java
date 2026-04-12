package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * A time-bounded label-pattern mute.
 *
 * <p>{@code active} is {@code true} when the current server time is
 * within {@code [startsAt, endsAt)}.
 */
public class Silence {
    @JsonProperty("id")
    private String id;

    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("matchers")
    private List<SilenceMatcher> matchers;

    @JsonProperty("starts_at")
    private String startsAt;

    @JsonProperty("ends_at")
    private String endsAt;

    @JsonProperty("created_by")
    private String createdBy;

    @JsonProperty("comment")
    private String comment;

    @JsonProperty("created_at")
    private String createdAt;

    @JsonProperty("updated_at")
    private String updatedAt;

    @JsonProperty("active")
    private boolean active;

    public String getId() { return id; }
    public String getNamespace() { return namespace; }
    public String getTenant() { return tenant; }
    public List<SilenceMatcher> getMatchers() { return matchers; }
    public String getStartsAt() { return startsAt; }
    public String getEndsAt() { return endsAt; }
    public String getCreatedBy() { return createdBy; }
    public String getComment() { return comment; }
    public String getCreatedAt() { return createdAt; }
    public String getUpdatedAt() { return updatedAt; }
    public boolean isActive() { return active; }
}
