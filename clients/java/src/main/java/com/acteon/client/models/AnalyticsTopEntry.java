package com.acteon.client.models;

import java.util.Map;

/**
 * A single entry in a top-N analytics result.
 */
public class AnalyticsTopEntry {
    private String label;
    private int count;
    private double percentage;

    public String getLabel() { return label; }
    public void setLabel(String label) { this.label = label; }

    public int getCount() { return count; }
    public void setCount(int count) { this.count = count; }

    public double getPercentage() { return percentage; }
    public void setPercentage(double percentage) { this.percentage = percentage; }

    @SuppressWarnings("unchecked")
    public static AnalyticsTopEntry fromMap(Map<String, Object> data) {
        AnalyticsTopEntry entry = new AnalyticsTopEntry();
        entry.label = (String) data.get("label");
        entry.count = ((Number) data.get("count")).intValue();
        entry.percentage = ((Number) data.get("percentage")).doubleValue();
        return entry;
    }
}
