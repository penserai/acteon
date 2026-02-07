import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { createWebhookAction } from "./models.js";

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
