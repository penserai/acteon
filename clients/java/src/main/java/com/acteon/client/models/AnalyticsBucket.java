package com.acteon.client.models;

import java.util.Map;

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

    @SuppressWarnings("unchecked")
    public static AnalyticsBucket fromMap(Map<String, Object> data) {
        AnalyticsBucket bucket = new AnalyticsBucket();
        bucket.timestamp = (String) data.get("timestamp");
        bucket.count = ((Number) data.get("count")).intValue();
        bucket.group = (String) data.get("group");

        if (data.get("avg_duration_ms") != null) {
            bucket.avgDurationMs = ((Number) data.get("avg_duration_ms")).doubleValue();
        }
        if (data.get("p50_duration_ms") != null) {
            bucket.p50DurationMs = ((Number) data.get("p50_duration_ms")).doubleValue();
        }
        if (data.get("p95_duration_ms") != null) {
            bucket.p95DurationMs = ((Number) data.get("p95_duration_ms")).doubleValue();
        }
        if (data.get("p99_duration_ms") != null) {
            bucket.p99DurationMs = ((Number) data.get("p99_duration_ms")).doubleValue();
        }
        if (data.get("error_rate") != null) {
            bucket.errorRate = ((Number) data.get("error_rate")).doubleValue();
        }

        return bucket;
    }
}
