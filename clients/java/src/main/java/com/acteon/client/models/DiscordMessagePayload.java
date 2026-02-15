package com.acteon.client.models;

import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * Payload builder for the Discord webhook provider.
 */
public class DiscordMessagePayload {
    private String content;
    private List<Map<String, Object>> embeds;
    private String username;
    private String avatarUrl;
    private Boolean tts;

    public static DiscordMessagePayload withContent(String content) {
        DiscordMessagePayload p = new DiscordMessagePayload();
        p.content = content;
        return p;
    }

    public static DiscordMessagePayload withEmbeds(List<Map<String, Object>> embeds) {
        DiscordMessagePayload p = new DiscordMessagePayload();
        p.embeds = embeds;
        return p;
    }

    public DiscordMessagePayload withUsername(String username) {
        this.username = username;
        return this;
    }

    public DiscordMessagePayload withAvatarUrl(String avatarUrl) {
        this.avatarUrl = avatarUrl;
        return this;
    }

    public DiscordMessagePayload withTts(boolean tts) {
        this.tts = tts;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        if (content != null) payload.put("content", content);
        if (embeds != null) payload.put("embeds", embeds);
        if (username != null) payload.put("username", username);
        if (avatarUrl != null) payload.put("avatar_url", avatarUrl);
        if (tts != null) payload.put("tts", tts);
        return payload;
    }
}
