import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { createWebhookAction, parseProviderHealthStatus, parseListProviderHealthResponse } from "./models.js";

describe("createWebhookAction", () => {
  it("creates a basic webhook action with defaults", () => {
    const action = createWebhookAction(
      "notifications",
      "tenant-1",
      "https://example.com/hook",
      { message: "hello" }
    );

    assert.equal(action.namespace, "notifications");
    assert.equal(action.tenant, "tenant-1");
    assert.equal(action.provider, "webhook");
    assert.equal(action.actionType, "webhook");
    assert.equal(action.payload.url, "https://example.com/hook");
    assert.equal(action.payload.method, "POST");
    assert.deepStrictEqual(action.payload.body, { message: "hello" });
    assert.equal(action.payload.headers, undefined);
    assert.ok(action.id); // auto-generated
    assert.ok(action.createdAt); // auto-generated
  });

  it("uses custom method", () => {
    const action = createWebhookAction(
      "ns",
      "t1",
      "https://example.com/hook",
      { key: "value" },
      { method: "PUT" }
    );

    assert.equal(action.payload.method, "PUT");
  });

  it("includes custom headers", () => {
    const action = createWebhookAction(
      "ns",
      "t1",
      "https://example.com/hook",
      {},
      { headers: { "X-Custom": "abc", "Authorization": "Bearer tok" } }
    );

    const headers = action.payload.headers as Record<string, string>;
    assert.equal(headers["X-Custom"], "abc");
    assert.equal(headers["Authorization"], "Bearer tok");
  });

  it("sets custom action type", () => {
    const action = createWebhookAction(
      "ns",
      "t1",
      "https://example.com/hook",
      {},
      { actionType: "custom_hook" }
    );

    assert.equal(action.actionType, "custom_hook");
  });

  it("passes through dedupKey and metadata", () => {
    const action = createWebhookAction(
      "ns",
      "t1",
      "https://example.com/hook",
      {},
      {
        dedupKey: "dedup-1",
        metadata: { env: "prod" },
      }
    );

    assert.equal(action.dedupKey, "dedup-1");
    assert.deepStrictEqual(action.metadata, { env: "prod" });
  });

  it("omits headers from payload when not provided", () => {
    const action = createWebhookAction(
      "ns",
      "t1",
      "https://example.com/hook",
      { data: 123 }
    );

    assert.equal(action.payload.headers, undefined);
  });

  it("provider is always webhook", () => {
    const action = createWebhookAction(
      "ns",
      "t1",
      "https://example.com/hook",
      {}
    );

    assert.equal(action.provider, "webhook");
  });
});

describe("parseProviderHealthStatus", () => {
  it("parses provider health status with all fields", () => {
    const data = {
      provider: "email",
      healthy: true,
      health_check_error: null,
      circuit_breaker_state: "closed",
      total_requests: 1500,
      successes: 1480,
      failures: 20,
      success_rate: 98.67,
      avg_latency_ms: 45.2,
      p50_latency_ms: 32.0,
      p95_latency_ms: 120.5,
      p99_latency_ms: 250.0,
      last_request_at: 1707900000000,
      last_error: "connection timeout",
    };

    const status = parseProviderHealthStatus(data);

    assert.equal(status.provider, "email");
    assert.equal(status.healthy, true);
    assert.equal(status.healthCheckError, null);
    assert.equal(status.circuitBreakerState, "closed");
    assert.equal(status.totalRequests, 1500);
    assert.equal(status.successes, 1480);
    assert.equal(status.failures, 20);
    assert.equal(status.successRate, 98.67);
    assert.equal(status.avgLatencyMs, 45.2);
    assert.equal(status.p50LatencyMs, 32.0);
    assert.equal(status.p95LatencyMs, 120.5);
    assert.equal(status.p99LatencyMs, 250.0);
    assert.equal(status.lastRequestAt, 1707900000000);
    assert.equal(status.lastError, "connection timeout");
  });

  it("parses provider health status with minimal fields", () => {
    const data = {
      provider: "slack",
      healthy: false,
      circuit_breaker_state: "open",
      total_requests: 100,
      successes: 50,
      failures: 50,
      success_rate: 50.0,
      avg_latency_ms: 100.0,
      p50_latency_ms: 90.0,
      p95_latency_ms: 200.0,
      p99_latency_ms: 300.0,
    };

    const status = parseProviderHealthStatus(data);

    assert.equal(status.provider, "slack");
    assert.equal(status.healthy, false);
    assert.equal(status.healthCheckError, undefined);
    assert.equal(status.lastRequestAt, undefined);
    assert.equal(status.lastError, undefined);
  });
});

describe("parseListProviderHealthResponse", () => {
  it("parses empty provider list", () => {
    const data = { providers: [] };
    const response = parseListProviderHealthResponse(data);
    assert.equal(response.providers.length, 0);
  });

  it("parses multiple providers", () => {
    const data = {
      providers: [
        {
          provider: "email",
          healthy: true,
          circuit_breaker_state: "closed",
          total_requests: 1000,
          successes: 990,
          failures: 10,
          success_rate: 99.0,
          avg_latency_ms: 50.0,
          p50_latency_ms: 40.0,
          p95_latency_ms: 100.0,
          p99_latency_ms: 150.0,
        },
        {
          provider: "sms",
          healthy: false,
          health_check_error: "connection refused",
          circuit_breaker_state: "open",
          total_requests: 500,
          successes: 450,
          failures: 50,
          success_rate: 90.0,
          avg_latency_ms: 200.0,
          p50_latency_ms: 150.0,
          p95_latency_ms: 400.0,
          p99_latency_ms: 600.0,
        },
      ],
    };

    const response = parseListProviderHealthResponse(data);

    assert.equal(response.providers.length, 2);
    assert.equal(response.providers[0].provider, "email");
    assert.equal(response.providers[0].healthy, true);
    assert.equal(response.providers[1].provider, "sms");
    assert.equal(response.providers[1].healthy, false);
    assert.equal(response.providers[1].healthCheckError, "connection refused");
  });
});
