package com.acteon.client.models;

/**
 * A single time bucket in an analytics response.
 */
public class AnalyticsBucket {
    private String timestamp;
    private int count;
    private String group;
    private Double avgDurationMs;
    private Double p50DurationMs;
    private Double p95DurationMs;
    private Double p99DurationMs;
    private Double errorRate;

    public String getTimestamp() { return timestamp; }
    public void setTimestamp(String timestamp) { this.timestamp = timestamp; }

    public int getCount() { return count; }
    public void setCount(int count) { this.count = count; }

    public String getGroup() { return group; }
    public void setGroup(String group) { this.group = group; }

    public Double getAvgDurationMs() { return avgDurationMs; }
    public void setAvgDurationMs(Double avgDurationMs) { this.avgDurationMs = avgDurationMs; }

    public Double getP50DurationMs() { return p50DurationMs; }
    public void setP50DurationMs(Double p50DurationMs) { this.p50DurationMs = p50DurationMs; }

    public Double getP95DurationMs() { return p95DurationMs; }
    public void setP95DurationMs(Double p95DurationMs) { this.p95DurationMs = p95DurationMs; }

    public Double getP99DurationMs() { return p99DurationMs; }
    public void setP99DurationMs(Double p99DurationMs) { this.p99DurationMs = p99DurationMs; }

    public Double getErrorRate() { return errorRate; }
    public void setErrorRate(Double errorRate) { this.errorRate = errorRate; }
}
