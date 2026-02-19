package com.acteon.client.models;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.time.Instant;
import java.util.Map;
import java.util.UUID;

/**
 * An action to be dispatched through Acteon.
 */
@JsonInclude(JsonInclude.Include.NON_NULL)
public class Action {
    private String id;
    private String namespace;
    private String tenant;
    private String provider;
    @JsonProperty("action_type")
    private String actionType;
    private Map<String, Object> payload;
    @JsonProperty("dedup_key")
    private String dedupKey;
    private ActionMetadata metadata;
    @JsonProperty("created_at")
    private String createdAt;
    private String template;

    public Action() {
        this.id = UUID.randomUUID().toString();
        this.createdAt = Instant.now().toString();
    }

    public Action(String namespace, String tenant, String provider, String actionType, Map<String, Object> payload) {
        this.id = UUID.randomUUID().toString();
        this.namespace = namespace;
        this.tenant = tenant;
        this.provider = provider;
        this.actionType = actionType;
        this.payload = payload;
        this.createdAt = Instant.now().toString();
    }

    public static Builder builder() {
        return new Builder();
    }

    // Getters and setters
    public String getId() { return id; }
    public void setId(String id) { this.id = id; }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getActionType() { return actionType; }
    public void setActionType(String actionType) { this.actionType = actionType; }

    public Map<String, Object> getPayload() { return payload; }
    public void setPayload(Map<String, Object> payload) { this.payload = payload; }

    public String getDedupKey() { return dedupKey; }
    public void setDedupKey(String dedupKey) { this.dedupKey = dedupKey; }

    public ActionMetadata getMetadata() { return metadata; }
    public void setMetadata(ActionMetadata metadata) { this.metadata = metadata; }

    public String getCreatedAt() { return createdAt; }
    public void setCreatedAt(String createdAt) { this.createdAt = createdAt; }

    public String getTemplate() { return template; }
    public void setTemplate(String template) { this.template = template; }

    public Action withDedupKey(String dedupKey) {
        this.dedupKey = dedupKey;
        return this;
    }

    public Action withMetadata(Map<String, String> labels) {
        this.metadata = new ActionMetadata(labels);
        return this;
    }

    public Action withTemplate(String template) {
        this.template = template;
        return this;
    }

    public static class Builder {
        private String id = UUID.randomUUID().toString();
        private String namespace;
        private String tenant;
        private String provider;
        private String actionType;
        private Map<String, Object> payload;
        private String dedupKey;
        private Map<String, String> labels;
        private String createdAt = Instant.now().toString();
        private String template;

        public Builder id(String id) { this.id = id; return this; }
        public Builder namespace(String namespace) { this.namespace = namespace; return this; }
        public Builder tenant(String tenant) { this.tenant = tenant; return this; }
        public Builder provider(String provider) { this.provider = provider; return this; }
        public Builder actionType(String actionType) { this.actionType = actionType; return this; }
        public Builder payload(Map<String, Object> payload) { this.payload = payload; return this; }
        public Builder dedupKey(String dedupKey) { this.dedupKey = dedupKey; return this; }
        public Builder labels(Map<String, String> labels) { this.labels = labels; return this; }
        public Builder createdAt(String createdAt) { this.createdAt = createdAt; return this; }
        public Builder template(String template) { this.template = template; return this; }

        public Action build() {
            Action action = new Action();
            action.id = this.id;
            action.namespace = this.namespace;
            action.tenant = this.tenant;
            action.provider = this.provider;
            action.actionType = this.actionType;
            action.payload = this.payload;
            action.dedupKey = this.dedupKey;
            action.createdAt = this.createdAt;
            action.template = this.template;
            if (this.labels != null) {
                action.metadata = new ActionMetadata(this.labels);
            }
            return action;
        }
    }
}
