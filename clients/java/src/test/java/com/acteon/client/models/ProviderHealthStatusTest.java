package com.acteon.client.models;

import org.junit.jupiter.api.Test;

import java.util.HashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

class ProviderHealthStatusTest {

    @Test
    void testFromMapWithAllFields() {
        Map<String, Object> data = new HashMap<>();
        data.put("provider", "email");
        data.put("healthy", true);
        data.put("health_check_error", null);
        data.put("circuit_breaker_state", "closed");
        data.put("total_requests", 1500);
        data.put("successes", 1480);
        data.put("failures", 20);
        data.put("success_rate", 98.67);
        data.put("avg_latency_ms", 45.2);
        data.put("p50_latency_ms", 32.0);
        data.put("p95_latency_ms", 120.5);
        data.put("p99_latency_ms", 250.0);
        data.put("last_request_at", 1707900000000L);
        data.put("last_error", "connection timeout");

        ProviderHealthStatus status = ProviderHealthStatus.fromMap(data);

        assertEquals("email", status.getProvider());
        assertTrue(status.isHealthy());
        assertNull(status.getHealthCheckError());
        assertEquals("closed", status.getCircuitBreakerState());
        assertEquals(1500, status.getTotalRequests());
        assertEquals(1480, status.getSuccesses());
        assertEquals(20, status.getFailures());
        assertEquals(98.67, status.getSuccessRate(), 0.01);
        assertEquals(45.2, status.getAvgLatencyMs(), 0.01);
        assertEquals(32.0, status.getP50LatencyMs(), 0.01);
        assertEquals(120.5, status.getP95LatencyMs(), 0.01);
        assertEquals(250.0, status.getP99LatencyMs(), 0.01);
        assertEquals(1707900000000L, status.getLastRequestAt());
        assertEquals("connection timeout", status.getLastError());
    }

    @Test
    void testFromMapWithMinimalFields() {
        Map<String, Object> data = new HashMap<>();
        data.put("provider", "sms");
        data.put("healthy", false);
        data.put("circuit_breaker_state", "open");
        data.put("total_requests", 100);
        data.put("successes", 50);
        data.put("failures", 50);
        data.put("success_rate", 50.0);
        data.put("avg_latency_ms", 100.0);
        data.put("p50_latency_ms", 90.0);
        data.put("p95_latency_ms", 200.0);
        data.put("p99_latency_ms", 300.0);

        ProviderHealthStatus status = ProviderHealthStatus.fromMap(data);

        assertEquals("sms", status.getProvider());
        assertFalse(status.isHealthy());
        assertNull(status.getHealthCheckError());
        assertNull(status.getLastRequestAt());
        assertNull(status.getLastError());
    }

    @Test
    void testFromMapWithHealthCheckError() {
        Map<String, Object> data = new HashMap<>();
        data.put("provider", "slack");
        data.put("healthy", false);
        data.put("health_check_error", "connection refused");
        data.put("circuit_breaker_state", "open");
        data.put("total_requests", 500);
        data.put("successes", 450);
        data.put("failures", 50);
        data.put("success_rate", 90.0);
        data.put("avg_latency_ms", 200.0);
        data.put("p50_latency_ms", 150.0);
        data.put("p95_latency_ms", 400.0);
        data.put("p99_latency_ms", 600.0);

        ProviderHealthStatus status = ProviderHealthStatus.fromMap(data);

        assertEquals("slack", status.getProvider());
        assertFalse(status.isHealthy());
        assertEquals("connection refused", status.getHealthCheckError());
        assertEquals("open", status.getCircuitBreakerState());
    }
}
