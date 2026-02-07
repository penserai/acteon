package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Convenience class for creating Actions targeted at the webhook provider.
 *
 * <p>Wraps the URL, method, headers, and body into the correct payload format
 * expected by the Acteon webhook provider.</p>
 *
 * <pre>{@code
 * Action action = WebhookAction.builder()
 *     .namespace("notifications")
 *     .tenant("tenant-1")
 *     .url("https://hooks.example.com/alert")
 *     .body(Map.of("message", "Server is down", "severity", "critical"))
 *     .header("X-Custom-Header", "value")
 *     .build();
 * }</pre>
 */
public class WebhookAction {

    private WebhookAction() {
    }

    /**
     * Create a new builder for a webhook action.
     *
     * @return a new Builder
     */
    public static Builder builder() {
        return new Builder();
    }

    /**
     * Create a simple webhook action with default settings (POST, no extra headers).
     *
     * @param namespace the action namespace
     * @param tenant    the tenant identifier
     * @param url       the webhook target URL
     * @param body      the JSON body to send
     * @return a configured Action
     */
    public static Action create(String namespace, String tenant, String url, Map<String, Object> body) {
        return builder()
                .namespace(namespace)
                .tenant(tenant)
                .url(url)
                .body(body)
                .build();
    }

    /**
     * Builder for constructing webhook Actions.
     */
    public static class Builder {
        private String namespace;
        private String tenant;
        private String url;
        private String method = "POST";
        private String actionType = "webhook";
        private Map<String, Object> body = new HashMap<>();
        private Map<String, String> headers = new HashMap<>();
        private String dedupKey;
        private Map<String, String> labels;

        public Builder namespace(String namespace) {
            this.namespace = namespace;
            return this;
        }

        public Builder tenant(String tenant) {
            this.tenant = tenant;
            return this;
        }

        public Builder url(String url) {
            this.url = url;
            return this;
        }

        public Builder method(String method) {
            this.method = method;
            return this;
        }

        public Builder actionType(String actionType) {
            this.actionType = actionType;
            return this;
        }

        public Builder body(Map<String, Object> body) {
            this.body = body;
            return this;
        }

        public Builder header(String key, String value) {
            this.headers.put(key, value);
            return this;
        }

        public Builder headers(Map<String, String> headers) {
            this.headers.putAll(headers);
            return this;
        }

        public Builder dedupKey(String dedupKey) {
            this.dedupKey = dedupKey;
            return this;
        }

        public Builder labels(Map<String, String> labels) {
            this.labels = labels;
            return this;
        }

        /**
         * Build the webhook Action.
         *
         * @return a configured Action for the webhook provider
         * @throws IllegalArgumentException if required fields are missing
         */
        public Action build() {
            if (namespace == null || namespace.isEmpty()) {
                throw new IllegalArgumentException("namespace is required");
            }
            if (tenant == null || tenant.isEmpty()) {
                throw new IllegalArgumentException("tenant is required");
            }
            if (url == null || url.isEmpty()) {
                throw new IllegalArgumentException("url is required");
            }

            Map<String, Object> payload = new HashMap<>();
            payload.put("url", url);
            payload.put("method", method);
            payload.put("body", body);
            if (!headers.isEmpty()) {
                payload.put("headers", headers);
            }

            Action.Builder actionBuilder = Action.builder()
                    .namespace(namespace)
                    .tenant(tenant)
                    .provider("webhook")
                    .actionType(actionType)
                    .payload(payload);

            if (dedupKey != null) {
                actionBuilder.dedupKey(dedupKey);
            }
            if (labels != null) {
                actionBuilder.labels(labels);
            }

            return actionBuilder.build();
        }
    }
}
