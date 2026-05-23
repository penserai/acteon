package com.acteon.client.models;

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
}
