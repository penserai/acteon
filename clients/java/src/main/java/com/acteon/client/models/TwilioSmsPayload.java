package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the Twilio SMS provider.
 */
public class TwilioSmsPayload {
    private final String to;
    private final String body;
    private String from;
    private String mediaUrl;

    public TwilioSmsPayload(String to, String body) {
        this.to = to;
        this.body = body;
    }

    public TwilioSmsPayload withFrom(String from) {
        this.from = from;
        return this;
    }

    public TwilioSmsPayload withMediaUrl(String mediaUrl) {
        this.mediaUrl = mediaUrl;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        payload.put("to", to);
        payload.put("body", body);
        if (from != null) payload.put("from", from);
        if (mediaUrl != null) payload.put("media_url", mediaUrl);
        return payload;
    }
}
