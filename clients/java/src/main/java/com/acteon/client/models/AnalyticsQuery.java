package com.acteon.client.models;

/**
 * Query parameters for the analytics endpoint.
 */
public class AnalyticsQuery {
    private String metric;
    private String namespace;
    private String tenant;
    private String provider;
    private String actionType;
    private String outcome;
    private String interval;
    private String from;
    private String to;
    private String groupBy;
    private Integer topN;

    public static Builder builder() {
        return new Builder();
    }

    public String getMetric() { return metric; }
    public void setMetric(String metric) { this.metric = metric; }

    public String getNamespace() { return namespace; }
    public void setNamespace(String namespace) { this.namespace = namespace; }

    public String getTenant() { return tenant; }
    public void setTenant(String tenant) { this.tenant = tenant; }

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public String getActionType() { return actionType; }
    public void setActionType(String actionType) { this.actionType = actionType; }

    public String getOutcome() { return outcome; }
    public void setOutcome(String outcome) { this.outcome = outcome; }

    public String getInterval() { return interval; }
    public void setInterval(String interval) { this.interval = interval; }

    public String getFrom() { return from; }
    public void setFrom(String from) { this.from = from; }

    public String getTo() { return to; }
    public void setTo(String to) { this.to = to; }

    public String getGroupBy() { return groupBy; }
    public void setGroupBy(String groupBy) { this.groupBy = groupBy; }

    public Integer getTopN() { return topN; }
    public void setTopN(Integer topN) { this.topN = topN; }

    public static class Builder {
        private String metric;
        private String namespace;
        private String tenant;
        private String provider;
        private String actionType;
        private String outcome;
        private String interval;
        private String from;
        private String to;
        private String groupBy;
        private Integer topN;

        public Builder metric(String metric) { this.metric = metric; return this; }
        public Builder namespace(String namespace) { this.namespace = namespace; return this; }
        public Builder tenant(String tenant) { this.tenant = tenant; return this; }
        public Builder provider(String provider) { this.provider = provider; return this; }
        public Builder actionType(String actionType) { this.actionType = actionType; return this; }
        public Builder outcome(String outcome) { this.outcome = outcome; return this; }
        public Builder interval(String interval) { this.interval = interval; return this; }
        public Builder from(String from) { this.from = from; return this; }
        public Builder to(String to) { this.to = to; return this; }
        public Builder groupBy(String groupBy) { this.groupBy = groupBy; return this; }
        public Builder topN(int topN) { this.topN = topN; return this; }

        public AnalyticsQuery build() {
            AnalyticsQuery query = new AnalyticsQuery();
            query.metric = this.metric;
            query.namespace = this.namespace;
            query.tenant = this.tenant;
            query.provider = this.provider;
            query.actionType = this.actionType;
            query.outcome = this.outcome;
            query.interval = this.interval;
            query.from = this.from;
            query.to = this.to;
            query.groupBy = this.groupBy;
            query.topN = this.topN;
            return query;
        }
    }
}
