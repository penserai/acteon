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

const histories = JSON.parse(readFileSync(new URL("../../contract-fixtures/provider-history.json", import.meta.url), "utf8"));
describe("retained provider history", () => {
  it("scopes and authenticates the read, decoding all nested outcomes", async () => {
    let index = 0;
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
      const parsed = new URL(url);
      expect(parsed.pathname).toBe(`/v1/governance/executions/${histories[0].receipt.execution_id}`);
      expect(parsed.searchParams.get("namespace")).toBe("prod");
      expect(parsed.searchParams.get("tenant")).toBe("acme");
      expect(init.method).toBe("GET");
      expect(new Headers(init.headers).get("Authorization")).toBe("Bearer operator-key");
      return new Response(JSON.stringify(histories[index++]));
    }));
    const client = new ActeonClient("https://acteon.example", { apiKey: "operator-key" });
    for (const wire of histories) {
      const result = await client.providerExecutionHistory("prod", "acme", wire.receipt.execution_id);
      expect(result.subject).toEqual(wire.subject);
      expect(result.receipt.status.state).toBe(wire.receipt.status.state);
      expect(result.metadata).toEqual(wire.metadata);
      if (result.receipt.status.state === "completed") {
        expect(result.receipt.status.outcome.type).toBe("executed");
        expect(result.attempts[0].original_outcome?.type).toBe("failed");
        expect(result.attempts[0].reconciliation?.outcome.type).toBe("executed");
        expect(result.attempts[0].original_evidence?.id).toBe("original-result");
        expect(result.attempts[0].reconciliation?.resolution.id).toBe("resolution");
        expect(result.attempts[0].reconciliation?.acceptance).toEqual(wire.attempts[0].reconciliation?.acceptance);
      }
    }
    expect(index).toBe(histories.length);
  });
});

it.each([401, 403, 404, 409, 503])("history preserves HTTP refusal %i without retry", async status => {
  const fetch = vi.fn(async () => new Response(JSON.stringify({ error: "history_denied" }), { status }));
  vi.stubGlobal("fetch", fetch);
  await expect(new ActeonClient("https://acteon.example").providerExecutionHistory("prod", "acme", histories[0].receipt.execution_id)).rejects.toMatchObject({ status });
  expect(fetch).toHaveBeenCalledTimes(1);
});


it.each([false, true])("preserves independent reconciliation capability %s", async permission => {
  const wire = { ...fixture.scope, management: { ...fixture.scope.management, can_reconcile: permission } };
  vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify(wire))));
  const view = await new ActeonClient("https://acteon.example").governance("prod", "acme");
  expect(view.management.can_reconcile).toBe(permission);
  expect(view.management.can_read_history ?? false).toBe(false);
});

const finality = JSON.parse(readFileSync(new URL("../../contract-fixtures/provider-reconciliation.json", import.meta.url), "utf8"));
it("correlates and accepts finality with exact scope, proof, authentication and nested outcomes", async () => {
  let index = 0;
  const responses = [finality.correlation, finality.receipt, finality.no_effect_receipt];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
    const parsed = new URL(url);
    const suffix = index === 0 ? "correlation" : "reconciliation";
    expect(parsed.pathname).toBe(`/v1/governance/executions/${finality.correlation.context.execution_id}/attempts/0/${suffix}`);
    expect(parsed.searchParams.get("namespace")).toBe("prod");
    expect(parsed.searchParams.get("tenant")).toBe("acme");
    expect(init.method).toBe(index === 0 ? "GET" : "POST");
    expect(new Headers(init.headers).get("Authorization")).toBe("Bearer operator-key");
    if (index > 0) expect(JSON.parse(init.body as string)).toEqual(finality.request);
    return new Response(JSON.stringify(responses[index++]));
  }));
  const client = new ActeonClient("https://acteon.example", { apiKey: "operator-key" });
  const id = finality.correlation.context.execution_id;
  expect(await client.providerReconciliationCorrelation("prod", "acme", id, 0)).toEqual(finality.correlation);
  const completed = await client.acceptProviderReconciliation("prod", "acme", id, 0, finality.request);
  expect(completed.status.state === "completed" && completed.status.outcome.type).toBe("executed");
  const fenced = await client.acceptProviderReconciliation("prod", "acme", id, 0, finality.request);
  expect(fenced.status.state === "completed" && fenced.status.outcome.type).toBe("failed");
  expect(index).toBe(3);
});
it.each([400, 401, 403, 404, 409, 503])("finality preserves refusal %i without retry", async status => {
  const fetch = vi.fn(async () => new Response('{"error":"finality_denied"}', { status }));
  vi.stubGlobal("fetch", fetch);
  await expect(new ActeonClient("https://acteon.example").acceptProviderReconciliation("prod", "acme", finality.correlation.context.execution_id, 0, finality.request)).rejects.toMatchObject({ status });
  expect(fetch).toHaveBeenCalledTimes(1);
});

const registryFixture = JSON.parse(readFileSync(new URL("../../contract-fixtures/governance-registry.json", import.meta.url), "utf8"));
describe("registry management", () => {
  it("preserves exact requests and validates matching completed receipts", async () => {
    let status = 200;
    let receipt = registryFixture.receipt;
    let view = registryFixture.view;
    const calls: {url: string; init: RequestInit}[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
      calls.push({url, init});
      expect(new Headers(init.headers).get("Authorization")).toBe("Bearer operator-key");
      expect(init.redirect).toBe("error");
      if (init.method === "GET") {
        expect(new URL(url).pathname).toBe("/v1/governance/registry/maya");
        expect(Object.fromEntries(new URL(url).searchParams)).toEqual({namespace:"prod",tenant:"acme",projection:"card"});
        return new Response(JSON.stringify(view));
      }
      expect(JSON.parse(init.body as string)).toEqual(registryFixture.request);
      return new Response(JSON.stringify(receipt), {status});
    }));
    const client = new ActeonClient("http://example.test", {apiKey:"operator-key"});
    expect(await client.registryProjection("prod","acme","maya","card")).toEqual(registryFixture.view);
    for (const [field,value] of [["tenant","other"],["version",0],["qualification_retired",null],["registry_revision",0],["value","invalid"]]) {
      view = {...registryFixture.view, [field as string]:value};
      const before = calls.length;
      await expect(client.registryProjection("prod","acme","maya","card")).rejects.toThrow();
      expect(calls.length).toBe(before+1);
    }
    for (let i=0;i<2;i++) expect(await client.mutateRegistry(registryFixture.request)).toEqual(registryFixture.receipt);
    for (const [field,value] of [["delivery_complete",false],["applied",false],["change_id","other"],["tenant","other"],["input_digest","bad"],["delivery_complete","true"]]) {
      receipt = {...registryFixture.receipt, [field as string]:value};
      const count = calls.length;
      await expect(client.mutateRegistry(registryFixture.request)).rejects.toThrow();
      expect(calls.length).toBe(count+1);
    }
    for (status of [401,403,409,503,307]) {
      const count = calls.length;
      await expect(client.mutateRegistry(registryFixture.request)).rejects.toMatchObject({status});
      expect(calls.length).toBe(count+1);
    }
  });
});


it("registry mutation correlates the sent identity despite caller mutation while awaiting", async () => {
  const request = structuredClone(registryFixture.request);
  vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit) => {
    expect(JSON.parse(init.body as string)).toEqual(registryFixture.request);
    request.tenant = "caller-changed-after-send";
    return new Response(JSON.stringify(registryFixture.receipt));
  }));
  expect(await new ActeonClient("http://example.test").mutateRegistry(request)).toEqual(registryFixture.receipt);
});
