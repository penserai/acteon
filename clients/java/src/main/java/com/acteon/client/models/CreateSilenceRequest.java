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

    public CreateSilenceRequest(List<SilenceMatcher> matchers, String comment, long durationSeconds) {

        this.matchers = matchers;
        this.comment = comment;
        this.durationSeconds = durationSeconds;
    }

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
