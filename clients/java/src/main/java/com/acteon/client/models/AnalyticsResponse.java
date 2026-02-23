package com.acteon.client.models;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Response from the analytics endpoint.
 */
public class AnalyticsResponse {
    private String metric;
    private String interval;
    private String from;
    private String to;
    private List<AnalyticsBucket> buckets;
    private List<AnalyticsTopEntry> topEntries;
    private int totalCount;

    public String getMetric() { return metric; }
    public void setMetric(String metric) { this.metric = metric; }

    public String getInterval() { return interval; }
    public void setInterval(String interval) { this.interval = interval; }

    public String getFrom() { return from; }
    public void setFrom(String from) { this.from = from; }

    public String getTo() { return to; }
    public void setTo(String to) { this.to = to; }

    public List<AnalyticsBucket> getBuckets() { return buckets; }
    public void setBuckets(List<AnalyticsBucket> buckets) { this.buckets = buckets; }

    public List<AnalyticsTopEntry> getTopEntries() { return topEntries; }
    public void setTopEntries(List<AnalyticsTopEntry> topEntries) { this.topEntries = topEntries; }

    public int getTotalCount() { return totalCount; }
    public void setTotalCount(int totalCount) { this.totalCount = totalCount; }

    @SuppressWarnings("unchecked")
    public static AnalyticsResponse fromMap(Map<String, Object> data) {
        AnalyticsResponse response = new AnalyticsResponse();
        response.metric = (String) data.get("metric");
        response.interval = (String) data.get("interval");
        response.from = (String) data.get("from");
        response.to = (String) data.get("to");
        response.totalCount = ((Number) data.get("total_count")).intValue();

        response.buckets = new ArrayList<>();
        List<Map<String, Object>> bucketsData = (List<Map<String, Object>>) data.get("buckets");
        if (bucketsData != null) {
            for (Map<String, Object> bucketData : bucketsData) {
                response.buckets.add(AnalyticsBucket.fromMap(bucketData));
            }
        }

        response.topEntries = new ArrayList<>();
        List<Map<String, Object>> topEntriesData = (List<Map<String, Object>>) data.get("top_entries");
        if (topEntriesData != null) {
            for (Map<String, Object> entryData : topEntriesData) {
                response.topEntries.add(AnalyticsTopEntry.fromMap(entryData));
            }
        }

        return response;
    }
}
