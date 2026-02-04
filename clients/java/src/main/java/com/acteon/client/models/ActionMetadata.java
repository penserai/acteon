package com.acteon.client.models;

import java.util.Map;

/**
 * Metadata for an action.
 */
public class ActionMetadata {
    private Map<String, String> labels;

    public ActionMetadata() {}

    public ActionMetadata(Map<String, String> labels) {
        this.labels = labels;
    }

    public Map<String, String> getLabels() { return labels; }
    public void setLabels(Map<String, String> labels) { this.labels = labels; }
}
