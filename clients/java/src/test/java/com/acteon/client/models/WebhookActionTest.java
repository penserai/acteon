package com.acteon.client.models;

import org.junit.jupiter.api.Test;

import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

class WebhookActionTest {

    @Test
    void createBasicWebhookAction() {
        Action action = WebhookAction.create("https://example.com/hook", Map.of("message", "hello"));

        assertFalse(new com.fasterxml.jackson.databind.ObjectMapper().convertValue(action, Map.class).containsKey("namespace"));
        assertFalse(new com.fasterxml.jackson.databind.ObjectMapper().convertValue(action, Map.class).containsKey("tenant"));
        assertEquals("webhook", action.getProvider());
        assertEquals("webhook", action.getActionType());
        assertNotNull(action.getId());
        assertNotNull(action.getCreatedAt());

        Map<String, Object> payload = action.getPayload();
        assertEquals("https://example.com/hook", payload.get("url"));
        assertEquals("POST", payload.get("method"));
        assertEquals(Map.of("message", "hello"), payload.get("body"));
        assertFalse(payload.containsKey("headers"));
    }

    @Test
    void builderWithAllOptions() {
        Action action = WebhookAction.builder()

                .url("https://example.com/hook")
                .method("PUT")
                .actionType("custom_hook")
                .body(Map.of("key", "value"))
                .header("X-Custom", "abc")
                .header("Authorization", "Bearer tok")
                .dedupKey("dedup-1")
                .labels(Map.of("env", "prod"))
                .build();

        assertFalse(new com.fasterxml.jackson.databind.ObjectMapper().convertValue(action, Map.class).containsKey("namespace"));
        assertFalse(new com.fasterxml.jackson.databind.ObjectMapper().convertValue(action, Map.class).containsKey("tenant"));
        assertEquals("webhook", action.getProvider());
        assertEquals("custom_hook", action.getActionType());
        assertEquals("dedup-1", action.getDedupKey());
        assertNotNull(action.getMetadata());

        Map<String, Object> payload = action.getPayload();
        assertEquals("https://example.com/hook", payload.get("url"));
        assertEquals("PUT", payload.get("method"));
        assertEquals(Map.of("key", "value"), payload.get("body"));

        @SuppressWarnings("unchecked")
        Map<String, String> headers = (Map<String, String>) payload.get("headers");
        assertEquals("abc", headers.get("X-Custom"));
        assertEquals("Bearer tok", headers.get("Authorization"));
    }

    @Test
    void builderDefaultMethod() {
        Action action = WebhookAction.builder()

                .url("https://example.com/hook")
                .body(Map.of())
                .build();

        assertEquals("POST", action.getPayload().get("method"));
    }

    @Test
    void builderDefaultActionType() {
        Action action = WebhookAction.builder()

                .url("https://example.com/hook")
                .body(Map.of())
                .build();

        assertEquals("webhook", action.getActionType());
    }

    @Test
    void builderDoesNotRequireContext() {
        assertDoesNotThrow( () ->
                WebhookAction.builder()

                        .url("https://example.com/hook")
                        .body(Map.of())
                        .build()
        );
    }

    @Test
    void builderAllowsTenantInBusinessPayload() {
        Action action = WebhookAction.create("https://example.com/hook", Map.of("tenant", "business-tenant"));
        assertEquals(Map.of("tenant", "business-tenant"), action.getPayload().get("body"));
    }

    @Test
    void builderRejectsNullUrl() {
        assertThrows(IllegalArgumentException.class, () ->
                WebhookAction.builder()

                        .body(Map.of())
                        .build()
        );
    }

    @Test
    void providerIsAlwaysWebhook() {
        Action action = WebhookAction.builder()

                .url("https://example.com/hook")
                .body(Map.of())
                .build();

        assertEquals("webhook", action.getProvider());
    }

    @Test
    void headersOmittedWhenEmpty() {
        Action action = WebhookAction.builder()

                .url("https://example.com/hook")
                .body(Map.of("data", 123))
                .build();

        assertFalse(action.getPayload().containsKey("headers"));
    }

    @Test
    void headersMapMerge() {
        Action action = WebhookAction.builder()

                .url("https://example.com/hook")
                .body(Map.of())
                .header("X-First", "1")
                .headers(Map.of("X-Second", "2"))
                .build();

        @SuppressWarnings("unchecked")
        Map<String, String> headers = (Map<String, String>) action.getPayload().get("headers");
        assertEquals("1", headers.get("X-First"));
        assertEquals("2", headers.get("X-Second"));
    }
}
