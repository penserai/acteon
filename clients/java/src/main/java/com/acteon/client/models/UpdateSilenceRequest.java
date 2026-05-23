package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * Request to extend a silence or edit its comment.
 *
 * <p>Matchers are immutable — to change them, expire the silence
 * and create a new one.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class UpdateSilenceRequest {
    @JsonProperty("ends_at")
    private String endsAt;

    @JsonProperty("comment")
    private String comment;

    public UpdateSilenceRequest() {}

    public UpdateSilenceRequest(String endsAt, String comment) {
        this.endsAt = endsAt;
        this.comment = comment;
    }

    public String getEndsAt() { return endsAt; }
    public void setEndsAt(String endsAt) { this.endsAt = endsAt; }

    public String getComment() { return comment; }
    public void setComment(String comment) { this.comment = comment; }
}
