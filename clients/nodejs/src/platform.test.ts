import { afterEach, describe, expect, it, vi } from "vitest";
import { ActeonClient } from "./client.js";
import { platformOperations, type PlatformOperation } from "./platform.js";
import { HttpError } from "./errors.js";
import { createBusSubscriptionBody, parseBusSubscription } from "./bus_models.js";

afterEach(() => vi.unstubAllGlobals());
describe("platform operations", () => {
  it("preserves every method, scoped path, body, query and response envelope", async () => {
    const client = new ActeonClient("http://localhost", { apiKey: "local-test" });
    for (const operation of Object.keys(platformOperations) as PlatformOperation[]) {
      const spec = platformOperations[operation];
      const path = Object.fromEntries(spec.parameters.map(key => [key, "team/child ?#%"]));
      let expected: string = spec.path;
      for (const [key, value] of Object.entries(path)) expected = expected.replace(`{${key}}`, encodeURIComponent(value));
      const body = spec.method === "GET" ? undefined : { request_id: "stable", payload: [1, null] };
      const fetch = vi.fn(async (input: string, init: RequestInit) => {
        const url = new URL(input);
        expect(url.pathname).toBe(expected);
        expect(url.searchParams.getAll("filter")).toEqual(["a b", "c&d"]);
        expect(init.method).toBe(spec.method);
        expect((init.headers as Record<string, string>).Authorization).toBe("Bearer local-test");
        expect(init.body).toBe(body === undefined ? undefined : JSON.stringify(body));
        return new Response(spec.response === "text" ? "metric 1\n" : '{"opaque":[1,null]}');
      });
      vi.stubGlobal("fetch", fetch);
      expect(await client.platformRequest(operation, { path, query: { filter: ["a b", "c&d"] }, body })).toEqual(spec.response === "text" ? "metric 1\n" : { opaque: [1, null] });
      expect(fetch).toHaveBeenCalledTimes(1);
    }
  });
  it("propagates status errors without retries and accepts 204", async () => {
    const client = new ActeonClient("http://localhost");
    for (const status of [403, 409, 429, 503]) {
      const fetch = vi.fn(async () => new Response("denied", { status }));
      vi.stubGlobal("fetch", fetch);
      await expect(client.platformRequest("auth_logout")).rejects.toEqual(new HttpError(status, "denied"));
      expect(fetch).toHaveBeenCalledTimes(1);
    }
    vi.stubGlobal("fetch", async () => new Response(null, { status: 204 }));
    expect(await client.platformRequest("auth_logout")).toBeNull();
    await expect(client.platformRequest("bus_stages_status", { path: { namespace: "n", tenant: "t", id: ".." } })).rejects.toThrow();
  });
  it("preserves subscription receipt mode and scoped group", () => {
    expect(createBusSubscriptionBody({ id: "s", topic: "n.t.logs", namespace: "n", tenant: "t", receiptRequired: true }).receipt_required).toBe(true);
    const sub = parseBusSubscription({ id: "s", receipt_required: true, consumer_group: "scoped" });
    expect(sub.receiptRequired).toBe(true);
    expect(sub.consumerGroup).toBe("scoped");
  });
});

import { readFileSync } from "node:fs";
import { parseActionOutcome, parseBatchResult } from "./models.js";
it("preserves governance outcomes and all fields in single and batch dispatch", () => {
  const fixtures = JSON.parse(readFileSync(new URL("../../contract-fixtures/dispatch-outcomes.json", import.meta.url), "utf8"));
  for (const fixture of fixtures) {
    const outcome = parseActionOutcome(fixture.wire);
    expect(outcome.type).toBe(fixture.type);
    expect(parseBatchResult(fixture.wire)).toEqual({ success: true, outcome });
    if (typeof fixture.wire === "object") {
      for (const [key, value] of Object.entries(Object.values(fixture.wire)[0] as object)) {
        const camel = key.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
        expect((outcome as unknown as Record<string, unknown>)[camel]).toEqual(value);
      }
    }
  }
});

it("preserves false JSON request bodies", async () => {
  const fetch = vi.fn(async (_input: string, init: RequestInit) => {
    expect(init.body).toBe("false");
    return new Response("null");
  });
  vi.stubGlobal("fetch", fetch);
  await new ActeonClient("http://localhost").platformRequest("dispatch_dispatch", { body: false });
});

it("resolves scoped topics before deleting by registered Kafka name", async () => {
  const topic = { name: "logs", namespace: "n", tenant: "t", kafka_name: "actual.logs" };
  const seen: string[] = [];
  vi.stubGlobal("fetch", async (input: string, init: RequestInit) => {
    const url = new URL(input);
    seen.push(`${init.method} ${url.pathname}`);
    if (init.method === "GET") {
      expect(url.searchParams.get("namespace")).toBe("n");
      expect(url.searchParams.get("tenant")).toBe("t");
      return new Response(JSON.stringify({ topics: [{ ...topic, tenant: "other" }, topic] }));
    }
    return new Response(null, { status: 204 });
  });
  await new ActeonClient("http://localhost").deleteBusTopic("n", "t", "logs");
  expect(seen).toEqual(["GET /v1/bus/topics", "DELETE /v1/bus/topics/actual.logs"]);
});
