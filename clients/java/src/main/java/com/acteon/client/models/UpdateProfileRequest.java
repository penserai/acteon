package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Request to update a template profile.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class UpdateProfileRequest {
    @JsonProperty("fields")
    private Map<String, Object> fields;

    @JsonProperty("description")
    private String description;

    @JsonProperty("labels")
    private Map<String, String> labels;

    public UpdateProfileRequest() {}

    public Map<String, Object> getFields() { return fields; }
    public void setFields(Map<String, Object> fields) { this.fields = fields; }

    public String getDescription() { return description; }
    public void setDescription(String description) { this.description = description; }

    public Map<String, String> getLabels() { return labels; }
    public void setLabels(Map<String, String> labels) { this.labels = labels; }
}
