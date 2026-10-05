import { afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { ActeonClient } from "./client.js";
import type { WorkforceChangeRequest, WorkforceScopeView } from "./workforce.js";
const fixture: { scope: WorkforceScopeView; changes: WorkforceChangeRequest[]; receipt: unknown } = JSON.parse(readFileSync(new URL("../../contract-fixtures/workforce-management.json", import.meta.url), "utf8"));
afterEach(() => vi.unstubAllGlobals());
describe("typed workforce", () => {
  it("sends every operation with exact fields and scope authentication", async () => {
    const requests: { url: string; init: RequestInit }[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
      requests.push({ url, init });
      return new Response(JSON.stringify(init.method === "GET" ? fixture.scope : fixture.receipt));
    }));
    const client = new ActeonClient("http://city.test", { apiKey: "operator-key" });
    expect(await client.workforce("prod", "acme")).toEqual(fixture.scope);
    const inspect = new URL(requests[0].url);
    expect(inspect.pathname).toBe("/v1/workforce");
    expect(Object.fromEntries(inspect.searchParams)).toEqual({ namespace: "prod", tenant: "acme" });
    for (const change of fixture.changes) {
      expect(await client.changeWorkforce(change)).toEqual(fixture.receipt);
      const request = requests.at(-1)!;
      expect(new URL(request.url).pathname).toBe("/v1/workforce/changes");
      expect(request.init.method).toBe("POST");
      expect(JSON.parse(request.init.body as string)).toEqual(change);
    }
    for (const request of requests) expect(new Headers(request.init.headers).get("Authorization")).toBe("Bearer operator-key");
  });
  it.each([401, 403, 409, 503])("retains HTTP %i without mutation retries", async status => {
    const fetch = vi.fn(async () => new Response('{"error":"denied"}', { status }));
    vi.stubGlobal("fetch", fetch);
    await expect(new ActeonClient("http://city.test").changeWorkforce(fixture.changes[0])).rejects.toMatchObject({ status });
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});
