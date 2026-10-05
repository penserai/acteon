import { afterEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { ActeonClient } from "./client.js";
const fixture = JSON.parse(readFileSync(new URL("../../contract-fixtures/governance-management.json", import.meta.url), "utf8"));
afterEach(() => vi.unstubAllGlobals());
describe("typed governance", () => {
  it("retains wire contracts, scope query, and authentication", async () => {
    const requests: { url: string; init: RequestInit }[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
      requests.push({ url, init });
      return new Response(JSON.stringify(requests.length === 1 ? fixture.scope : fixture.receipt));
    }));
    const client = new ActeonClient("http://example.test", { apiKey: "operator-key" });
    expect(await client.governance("prod", "acme")).toEqual(fixture.scope);
    expect(await client.publishGovernancePermit(fixture.publication)).toEqual(fixture.receipt);
    expect(await client.interveneGovernance(fixture.intervention)).toEqual(fixture.receipt);
    expect(new URL(requests[0].url).searchParams.get("tenant")).toBe("acme");
    expect(new URL(requests[0].url).searchParams.get("namespace")).toBe("prod");
    expect(requests.map(r => r.init.method)).toEqual(["GET", "POST", "POST"]);
    expect(JSON.parse(requests[1].init.body as string)).toEqual(fixture.publication);
    expect(JSON.parse(requests[2].init.body as string)).toEqual(fixture.intervention);
    for (const r of requests) expect(new Headers(r.init.headers).get("Authorization")).toBe("Bearer operator-key");
  });
  it.each([401, 403, 409, 503])("preserves HTTP %i and never retries", async status => {
    const fetch = vi.fn(async () => new Response('{"error":"governance_denied"}', { status }));
    vi.stubGlobal("fetch", fetch);
    await expect(new ActeonClient("http://example.test").interveneGovernance(fixture.intervention)).rejects.toMatchObject({ status });
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});
