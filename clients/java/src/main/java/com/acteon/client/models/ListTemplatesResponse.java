package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;

/**
 * Response from listing payload templates.
 */
public class ListTemplatesResponse {
    @JsonProperty("templates")
    private List<TemplateInfo> templates;

    @JsonProperty("count")
    private int count;

    public List<TemplateInfo> getTemplates() { return templates; }
    public int getCount() { return count; }
}
