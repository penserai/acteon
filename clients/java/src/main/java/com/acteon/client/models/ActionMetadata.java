package com.acteon.client.models;

import java.util.Map;
import java.util.HashMap;
import com.fasterxml.jackson.annotation.JsonAnyGetter;
import com.fasterxml.jackson.annotation.JsonAnySetter;
import com.fasterxml.jackson.annotation.JsonIgnore;

/**
 * Metadata for an action.
 */
public class ActionMetadata {
    private Map<String, String> labels = new HashMap<>();

    public ActionMetadata() {}

    public ActionMetadata(Map<String, String> labels) {
        this.labels = labels;
    }

    @JsonAnyGetter
    public Map<String, String> getLabels() { return labels; }
    @JsonIgnore
    public void setLabels(Map<String, String> labels) { this.labels = labels; }
    @JsonAnySetter
    public void putLabel(String name, String value) { labels.put(name, value); }
}
