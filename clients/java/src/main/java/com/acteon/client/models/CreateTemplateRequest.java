package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Map;

/**
 * Request to create a payload template.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class CreateTemplateRequest {
    @JsonProperty("name")
    private String name;

    @JsonProperty("content")
    private String content;

    @JsonProperty("description")
    private String description;

    @JsonProperty("labels")
    private Map<String, String> labels;

    public CreateTemplateRequest() {}

    public CreateTemplateRequest(String name, String content) {
        this.name = name;

        this.content = content;
    }

    public String getName() { return name; }
    public void setName(String name) { this.name = name; }

    public String getContent() { return content; }
    public void setContent(String content) { this.content = content; }

    public String getDescription() { return description; }
    public void setDescription(String description) { this.description = description; }

    public Map<String, String> getLabels() { return labels; }
    public void setLabels(Map<String, String> labels) { this.labels = labels; }
}
