package com.acteon.client.models;

import java.util.Map;

/**
 * Health and metrics for a single provider.
 */
public class ProviderHealthStatus {
    private String provider;
    private boolean healthy;
    private String healthCheckError;
    private String circuitBreakerState;
    private int totalRequests;
    private int successes;
    private int failures;
    private double successRate;
    private double avgLatencyMs;
    private double p50LatencyMs;
    private double p95LatencyMs;
    private double p99LatencyMs;
    private Long lastRequestAt;
    private String lastError;

    public String getProvider() { return provider; }
    public void setProvider(String provider) { this.provider = provider; }

    public boolean isHealthy() { return healthy; }
    public void setHealthy(boolean healthy) { this.healthy = healthy; }

    public String getHealthCheckError() { return healthCheckError; }
    public void setHealthCheckError(String healthCheckError) { this.healthCheckError = healthCheckError; }

    public String getCircuitBreakerState() { return circuitBreakerState; }
    public void setCircuitBreakerState(String circuitBreakerState) { this.circuitBreakerState = circuitBreakerState; }

    public int getTotalRequests() { return totalRequests; }
    public void setTotalRequests(int totalRequests) { this.totalRequests = totalRequests; }

    public int getSuccesses() { return successes; }
    public void setSuccesses(int successes) { this.successes = successes; }

    public int getFailures() { return failures; }
    public void setFailures(int failures) { this.failures = failures; }

    public double getSuccessRate() { return successRate; }
    public void setSuccessRate(double successRate) { this.successRate = successRate; }

    public double getAvgLatencyMs() { return avgLatencyMs; }
    public void setAvgLatencyMs(double avgLatencyMs) { this.avgLatencyMs = avgLatencyMs; }

    public double getP50LatencyMs() { return p50LatencyMs; }
    public void setP50LatencyMs(double p50LatencyMs) { this.p50LatencyMs = p50LatencyMs; }

    public double getP95LatencyMs() { return p95LatencyMs; }
    public void setP95LatencyMs(double p95LatencyMs) { this.p95LatencyMs = p95LatencyMs; }

    public double getP99LatencyMs() { return p99LatencyMs; }
    public void setP99LatencyMs(double p99LatencyMs) { this.p99LatencyMs = p99LatencyMs; }

    public Long getLastRequestAt() { return lastRequestAt; }
    public void setLastRequestAt(Long lastRequestAt) { this.lastRequestAt = lastRequestAt; }

    public String getLastError() { return lastError; }
    public void setLastError(String lastError) { this.lastError = lastError; }

    @SuppressWarnings("unchecked")
    public static ProviderHealthStatus fromMap(Map<String, Object> data) {
        ProviderHealthStatus status = new ProviderHealthStatus();
        status.provider = (String) data.get("provider");
        status.healthy = (Boolean) data.get("healthy");
        status.healthCheckError = (String) data.get("health_check_error");
        status.circuitBreakerState = (String) data.get("circuit_breaker_state");
        status.totalRequests = ((Number) data.get("total_requests")).intValue();
        status.successes = ((Number) data.get("successes")).intValue();
        status.failures = ((Number) data.get("failures")).intValue();
        status.successRate = ((Number) data.get("success_rate")).doubleValue();
        status.avgLatencyMs = ((Number) data.get("avg_latency_ms")).doubleValue();
        status.p50LatencyMs = ((Number) data.get("p50_latency_ms")).doubleValue();
        status.p95LatencyMs = ((Number) data.get("p95_latency_ms")).doubleValue();
        status.p99LatencyMs = ((Number) data.get("p99_latency_ms")).doubleValue();

        if (data.containsKey("last_request_at") && data.get("last_request_at") != null) {
            status.lastRequestAt = ((Number) data.get("last_request_at")).longValue();
        }
        if (data.containsKey("last_error") && data.get("last_error") != null) {
            status.lastError = (String) data.get("last_error");
        }

        return status;
    }
}
