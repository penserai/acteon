import { afterEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { ActeonClient } from "./client.js";
import { AGENT_SOURCE_CONTEXT_HEADER } from "./agent_services.js";
import { HttpError } from "./errors.js";
const fixture = JSON.parse(readFileSync(new URL("../../contract-fixtures/agent-services.json", import.meta.url), "utf8"));
afterEach(() => vi.unstubAllGlobals());
it("keeps concurrent job headers separate and ignores mutable task metadata", async () => {
  vi.stubGlobal("fetch", async (input: string, init: RequestInit) => {
    const headers = init.headers as Record<string, string>;
    expect(headers.Authorization).toBe("Bearer caller-key");
    expect(init.redirect).toBe("error");
    expect(headers["A2A-Version"]).toBe("1.0");
    const stop = input.endsWith("/stop");
    const index = stop ? Number(input.split("/").at(-2)!.slice(-1))-1 : init.method === "POST" ? Number(JSON.parse(init.body as string).message.messageId.slice(-1))-1 : Number(input.slice(-1))-1;
    const job = fixture.jobs[index];
    if (init.method === "GET" || stop) expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBe(job.source_context);
    else expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBeUndefined();
    return new Response(JSON.stringify(stop ? job.stop_response : job.task), { headers: { "a2a-version":"1.0", [AGENT_SOURCE_CONTEXT_HEADER]:job.source_context } });
  });
  const client = new ActeonClient("http://acteon", {apiKey:"caller-key"});
  const receipts = await Promise.all([1,2].map(i => client.agentServiceSendMessage("prod", "acme", "notifier", {messageId:`m${i}`})));
  receipts[0].task.id = "tampered-model-id";
  const tasks = await Promise.all(receipts.reverse().map(receipt => client.agentServiceGetTask(receipt)));
  expect(tasks.map(task => task.id)).toEqual(["job-2","job-1"]);
  const stopped = await Promise.all(receipts.map(receipt => client.agentServiceStopTask(receipt)));
  expect(stopped.map(r => r.task.id)).toEqual(["job-2", "job-1"]);
  expect(stopped.every(r => r.futureStartsBlocked && (r.task.status as {state: string}).state === "submitted")).toBe(true);
});
it("rejects absent or malformed admission context without metadata fallback", async () => {
  for (const source of [null, "", "bad token", "x".repeat(8193)]) {
    const fetch = vi.fn(async () => new Response(JSON.stringify(fixture.jobs[0].task), {headers:{"a2a-version":"1.0", ...(source === null ? {} : {[AGENT_SOURCE_CONTEXT_HEADER]:source})}}));
    vi.stubGlobal("fetch", fetch);
    await expect(new ActeonClient("http://acteon").agentServiceSendMessage("prod","acme","notifier",{})).rejects.toThrow("source context");
    expect(fetch).toHaveBeenCalledTimes(1);
  }
});
it("preserves typed HTTP statuses with one request", async () => {
  for (const status of [...fixture.error_statuses,307]) {
    const fetch = vi.fn(async () => new Response("denied", {status}));
    vi.stubGlobal("fetch", fetch);
    await expect(new ActeonClient("http://acteon").agentServiceSendMessage("prod","acme","notifier",{})).rejects.toEqual(new HttpError(status,"denied"));
    expect(fetch).toHaveBeenCalledTimes(1);
  }
});
it("escapes route segments and rejects mismatched observed identity", async () => {
  const fetch = vi.fn(async (input: string) => {
    expect(input).toContain("/agents/team%2Fchild%20%3F%23/v1/tasks/job-1");
    return new Response(JSON.stringify(fixture.jobs[1].task), {headers:{"a2a-version":"1.0"}});
  });
  vi.stubGlobal("fetch", fetch);
  await expect(new ActeonClient("http://acteon").agentServiceGetTask({namespace:"prod", tenant:"acme", agent:"team/child ?#", taskId:"job-1", sourceContext:fixture.jobs[0].source_context, task:fixture.jobs[0].task})).rejects.toThrow("identity mismatch");
});

it("rejects false stop acknowledgements and foreign task identities without retries", async () => {
  const receipt = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  for (const payload of [null, {}, {task: fixture.jobs[0].task, future_starts_blocked: false}, {task: fixture.jobs[0].task, future_starts_blocked: "true"}, {task: fixture.jobs[1].task, future_starts_blocked: true}]) {
    const fetch = vi.fn(async () => new Response(JSON.stringify(payload), {headers: {"a2a-version":"1.0"}}));
    vi.stubGlobal("fetch", fetch);
    await expect(new ActeonClient("http://acteon").agentServiceStopTask(receipt)).rejects.toThrow();
    expect(fetch).toHaveBeenCalledTimes(1);
  }
  for (const status of [...fixture.error_statuses, 307]) {
    const fetch = vi.fn(async () => new Response("denied", {status}));
    vi.stubGlobal("fetch", fetch);
    await expect(new ActeonClient("http://acteon").agentServiceStopTask(receipt)).rejects.toEqual(new HttpError(status, "denied"));
    expect(fetch).toHaveBeenCalledTimes(1);
  }
});
