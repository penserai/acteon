import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  createWebhookAction,
  parseProviderHealthStatus,
  parseListProviderHealthResponse,
  parseWasmPluginConfig,
  parseWasmPlugin,
  parseListPluginsResponse,
  parsePluginInvocationResponse,
  registerPluginRequestToApi,
} from "./models.js";

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

// ---------------------------------------------------------------------------
// WASM Plugin types
// ---------------------------------------------------------------------------

describe("parseWasmPluginConfig", () => {
  it("parses all fields", () => {
    const data = {
      memory_limit_bytes: 16777216,
      timeout_ms: 100,
      allowed_host_functions: ["log", "time"],
    };
    const config = parseWasmPluginConfig(data);
    assert.equal(config.memoryLimitBytes, 16777216);
    assert.equal(config.timeoutMs, 100);
    assert.deepStrictEqual(config.allowedHostFunctions, ["log", "time"]);
  });

  it("handles missing fields", () => {
    const config = parseWasmPluginConfig({});
    assert.equal(config.memoryLimitBytes, undefined);
    assert.equal(config.timeoutMs, undefined);
    assert.equal(config.allowedHostFunctions, undefined);
  });
});

describe("parseWasmPlugin", () => {
  it("parses a complete plugin", () => {
    const data = {
      name: "my-plugin",
      description: "A test plugin",
      status: "active",
      enabled: true,
      config: {
        memory_limit_bytes: 16777216,
        timeout_ms: 100,
      },
      created_at: "2026-02-15T00:00:00Z",
      updated_at: "2026-02-15T01:00:00Z",
      invocation_count: 42,
    };
    const plugin = parseWasmPlugin(data);
    assert.equal(plugin.name, "my-plugin");
    assert.equal(plugin.description, "A test plugin");
    assert.equal(plugin.status, "active");
    assert.equal(plugin.enabled, true);
    assert.notEqual(plugin.config, undefined);
    assert.equal(plugin.config!.memoryLimitBytes, 16777216);
    assert.equal(plugin.createdAt, "2026-02-15T00:00:00Z");
    assert.equal(plugin.invocationCount, 42);
  });

  it("handles minimal plugin", () => {
    const data = {
      name: "minimal-plugin",
      status: "active",
      created_at: "2026-02-15T00:00:00Z",
      updated_at: "2026-02-15T00:00:00Z",
    };
    const plugin = parseWasmPlugin(data);
    assert.equal(plugin.name, "minimal-plugin");
    assert.equal(plugin.description, undefined);
    assert.equal(plugin.config, undefined);
    assert.equal(plugin.invocationCount, 0);
  });
});

describe("registerPluginRequestToApi", () => {
  it("converts minimal request", () => {
    const result = registerPluginRequestToApi({ name: "test-plugin" });
    assert.deepStrictEqual(result, { name: "test-plugin" });
  });

  it("converts complete request", () => {
    const result = registerPluginRequestToApi({
      name: "test-plugin",
      description: "A test",
      wasmPath: "/plugins/test.wasm",
      config: { memoryLimitBytes: 1024, timeoutMs: 50 },
    });
    assert.equal(result.name, "test-plugin");
    assert.equal(result.description, "A test");
    assert.equal(result.wasm_path, "/plugins/test.wasm");
    assert.equal((result.config as Record<string, unknown>).memory_limit_bytes, 1024);
    assert.equal((result.config as Record<string, unknown>).timeout_ms, 50);
  });
});

describe("parseListPluginsResponse", () => {
  it("parses empty list", () => {
    const response = parseListPluginsResponse({ plugins: [], count: 0 });
    assert.equal(response.plugins.length, 0);
    assert.equal(response.count, 0);
  });

  it("parses multiple plugins", () => {
    const data = {
      plugins: [
        {
          name: "plugin-a",
          status: "active",
          enabled: true,
          created_at: "2026-02-15T00:00:00Z",
          updated_at: "2026-02-15T00:00:00Z",
        },
        {
          name: "plugin-b",
          status: "disabled",
          enabled: false,
          created_at: "2026-02-15T00:00:00Z",
          updated_at: "2026-02-15T00:00:00Z",
        },
      ],
      count: 2,
    };
    const response = parseListPluginsResponse(data);
    assert.equal(response.plugins.length, 2);
    assert.equal(response.count, 2);
    assert.equal(response.plugins[0].name, "plugin-a");
    assert.equal(response.plugins[0].enabled, true);
    assert.equal(response.plugins[1].name, "plugin-b");
    assert.equal(response.plugins[1].enabled, false);
  });
});

describe("parsePluginInvocationResponse", () => {
  it("parses complete response", () => {
    const data = {
      verdict: true,
      message: "all good",
      metadata: { score: 0.95 },
      duration_ms: 12.5,
    };
    const resp = parsePluginInvocationResponse(data);
    assert.equal(resp.verdict, true);
    assert.equal(resp.message, "all good");
    assert.deepStrictEqual(resp.metadata, { score: 0.95 });
    assert.equal(resp.durationMs, 12.5);
  });

  it("handles minimal response", () => {
    const resp = parsePluginInvocationResponse({ verdict: false });
    assert.equal(resp.verdict, false);
    assert.equal(resp.message, undefined);
    assert.equal(resp.metadata, undefined);
    assert.equal(resp.durationMs, undefined);
  });
});
