package com.acteon.client.models;

import java.util.HashMap;
import java.util.Map;

/**
 * Payload builder for the Microsoft Teams provider.
 */
public class TeamsMessagePayload {
    private String text;
    private String title;
    private String themeColor;
    private String summary;
    private Map<String, Object> adaptiveCard;

    public static TeamsMessagePayload withText(String text) {
        TeamsMessagePayload p = new TeamsMessagePayload();
        p.text = text;
        return p;
    }

    public static TeamsMessagePayload withAdaptiveCard(Map<String, Object> card) {
        TeamsMessagePayload p = new TeamsMessagePayload();
        p.adaptiveCard = card;
        return p;
    }

    public TeamsMessagePayload withTitle(String title) {
        this.title = title;
        return this;
    }

    public TeamsMessagePayload withThemeColor(String themeColor) {
        this.themeColor = themeColor;
        return this;
    }

    public TeamsMessagePayload withSummary(String summary) {
        this.summary = summary;
        return this;
    }

    public Map<String, Object> toPayload() {
        Map<String, Object> payload = new HashMap<>();
        if (text != null) payload.put("text", text);
        if (title != null) payload.put("title", title);
        if (themeColor != null) payload.put("theme_color", themeColor);
        if (summary != null) payload.put("summary", summary);
        if (adaptiveCard != null) payload.put("adaptive_card", adaptiveCard);
        return payload;
    }
}
