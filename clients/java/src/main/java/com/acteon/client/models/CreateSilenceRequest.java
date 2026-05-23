package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Request to create a silence.
 *
 * <p>Either {@code endsAt} or {@code durationSeconds} must be
 * supplied. When {@code startsAt} is null the server uses its
 * current time.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class CreateSilenceRequest {
    @JsonProperty("namespace")
    private String namespace;

    @JsonProperty("tenant")
    private String tenant;

    @JsonProperty("matchers")
    private List<SilenceMatcher> matchers;

    @JsonProperty("comment")
    private String comment;

    @JsonProperty("starts_at")
    private String startsAt;

    @JsonProperty("ends_at")
    private String endsAt;

    @JsonProperty("duration_seconds")
    private Long durationSeconds;

    public CreateSilenceRequest() {}

    public CreateSilenceRequest(
            String namespace,
            String tenant,
            List<SilenceMatcher> matchers,
            String comment,
            long durationSeconds) {
        this.namespace = namespace;
        this.tenant = tenant;
        this.matchers = matchers;
        this.comment = comment;
        this.durationSeconds = durationSeconds;
    }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public List<SilenceMatcher> getMatchers() { return matchers; }
    public void setMatchers(List<SilenceMatcher> matchers) { this.matchers = matchers; }

    public String getComment() { return comment; }
    public void setComment(String comment) { this.comment = comment; }

    public String getStartsAt() { return startsAt; }
    public void setStartsAt(String startsAt) { this.startsAt = startsAt; }

    public String getEndsAt() { return endsAt; }
    public void setEndsAt(String endsAt) { this.endsAt = endsAt; }

    public Long getDurationSeconds() { return durationSeconds; }
    public void setDurationSeconds(Long durationSeconds) { this.durationSeconds = durationSeconds; }
}
