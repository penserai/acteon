package com.acteon.client.models;

import com.acteon.client.JsonMapper;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

class ProviderHealthStatusTest {
    private static final ObjectMapper MAPPER = JsonMapper.build();

    @Test
    void deserializesAllFields() throws Exception {
        String json = """
            {
              "provider": "email",
              "healthy": true,
              "health_check_error": null,
              "circuit_breaker_state": "closed",
              "total_requests": 1500,
              "successes": 1480,
              "failures": 20,
              "success_rate": 98.67,
              "avg_latency_ms": 45.2,
              "p50_latency_ms": 32.0,
              "p95_latency_ms": 120.5,
              "p99_latency_ms": 250.0,
              "last_request_at": 1707900000000,
              "last_error": "connection timeout"
            }
            """;

        ProviderHealthStatus status = MAPPER.readValue(json, ProviderHealthStatus.class);

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
        assertEquals(1_707_900_000_000L, status.getLastRequestAt());
        assertEquals("connection timeout", status.getLastError());
    }

    @Test
    void deserializesMinimalFields() throws Exception {
        String json = """
            {
              "provider": "sms",
              "healthy": false,
              "circuit_breaker_state": "open",
              "total_requests": 100,
              "successes": 50,
              "failures": 50,
              "success_rate": 50.0,
              "avg_latency_ms": 100.0,
              "p50_latency_ms": 90.0,
              "p95_latency_ms": 200.0,
              "p99_latency_ms": 300.0
            }
            """;

        ProviderHealthStatus status = MAPPER.readValue(json, ProviderHealthStatus.class);

        assertEquals("sms", status.getProvider());
        assertFalse(status.isHealthy());
        assertNull(status.getHealthCheckError());
        assertNull(status.getLastRequestAt());
        assertNull(status.getLastError());
    }

    @Test
    void deserializesWithHealthCheckError() throws Exception {
        String json = """
            {
              "provider": "slack",
              "healthy": false,
              "health_check_error": "connection refused",
              "circuit_breaker_state": "open",
              "total_requests": 500,
              "successes": 450,
              "failures": 50,
              "success_rate": 90.0,
              "avg_latency_ms": 200.0,
              "p50_latency_ms": 150.0,
              "p95_latency_ms": 400.0,
              "p99_latency_ms": 600.0
            }
            """;

        ProviderHealthStatus status = MAPPER.readValue(json, ProviderHealthStatus.class);

        assertEquals("slack", status.getProvider());
        assertFalse(status.isHealthy());
        assertEquals("connection refused", status.getHealthCheckError());
        assertEquals("open", status.getCircuitBreakerState());
    }

    @Test
    void ignoresUnknownProperties() throws Exception {
        // Forward-compat: a server that grows a new field shouldn't
        // break older clients.
        String json = """
            {
              "provider": "x",
              "healthy": true,
              "circuit_breaker_state": "closed",
              "total_requests": 0,
              "successes": 0,
              "failures": 0,
              "success_rate": 0.0,
              "avg_latency_ms": 0.0,
              "p50_latency_ms": 0.0,
              "p95_latency_ms": 0.0,
              "p99_latency_ms": 0.0,
              "future_field": "ignored"
            }
            """;

        ProviderHealthStatus status = MAPPER.readValue(json, ProviderHealthStatus.class);
        assertEquals("x", status.getProvider());
    }
}
