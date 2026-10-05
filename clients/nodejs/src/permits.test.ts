import { readFileSync } from "node:fs";
import { afterEach, expect, it, vi } from "vitest";
import { ActeonClient } from "./client.js";
import { HttpError } from "./errors.js";
import { createAction } from "./models.js";
afterEach(() => vi.unstubAllGlobals());
it("preserves credentials and body while serializing explicit permit revisions", async () => {
  const fixture = JSON.parse(readFileSync(new URL("../../contract-fixtures/execution-permits.json", import.meta.url), "utf8"));
  const permits = fixture.map((p: {id: string; accepted_revision: number}) => ({id: p.id, acceptedRevision: p.accepted_revision}));
  let count = 0;
  vi.stubGlobal("fetch", async (url: string, init: RequestInit) => {
    const headers = init.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer test-key");
    if (count++ === 0) expect(headers["x-acteon-execution-permits"]).toBeUndefined();
    else expect(JSON.parse(headers["x-acteon-execution-permits"])).toEqual(fixture);
    const body = JSON.parse(init.body as string);
    expect((Array.isArray(body) ? body[0] : body).payload).toEqual({incident: 42});
    return new Response(JSON.stringify(url.endsWith("/batch") ? ["Deduplicated"] : "Deduplicated"));
  });
  const client = new ActeonClient("http://localhost", {apiKey: "test-key"});
  const action = createAction("prod", "acme", "incident", "execute", {incident: 42});
  await client.dispatch(action);
  await client.dispatch(action, {permits});
  await client.dispatchBatch([action], {permits});
  expect(count).toBe(3);
  await expect(client.dispatch(action, {permits: [{id: "p", acceptedRevision: Number.MAX_SAFE_INTEGER + 1}]})).rejects.toThrow(RangeError);
  expect(count).toBe(3);
});

it("preserves single and batch HTTP refusals without retries", async () => {
  const client = new ActeonClient("http://localhost");
  const action = createAction("prod", "acme", "incident", "execute", {});
  for (const status of [400, 403, 409]) {
    const fetch = vi.fn(async () => new Response('{"error":"permit refused"}', {status}));
    vi.stubGlobal("fetch", fetch);
    await expect(client.dispatch(action, {permits: []})).rejects.toEqual(new HttpError(status, '{"error":"permit refused"}'));
    await expect(client.dispatchBatch([action], {permits: []})).rejects.toBeInstanceOf(HttpError);
    expect(fetch).toHaveBeenCalledTimes(2);
  }
});
