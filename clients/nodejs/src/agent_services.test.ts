import { afterEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { ActeonClient } from "./client.js";
import { AGENT_EXECUTION_CONTEXT_HEADER, AGENT_SOURCE_CONTEXT_HEADER, agentPeerCancelReceipt, agentProviderAbort } from "./agent_services.js";
import { HttpError } from "./errors.js";
const fixture = JSON.parse(readFileSync(new URL("../../contract-fixtures/agent-services.json", import.meta.url), "utf8"));
afterEach(() => vi.unstubAllGlobals());
it("carries an opaque governed parent with explicit permits", async () => {
  const fetch = vi.fn(async (_input: string, init: RequestInit) => {
    const headers = init.headers as Record<string, string>;
    expect(headers[AGENT_EXECUTION_CONTEXT_HEADER]).toBe(fixture.parent.execution_context);
    expect(JSON.parse(headers["x-acteon-execution-permits"])).toEqual(fixture.parent.permits);
    expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBeUndefined();
    return new Response(JSON.stringify(fixture.jobs[0].task), {headers:{"a2a-version":"1.0", [AGENT_SOURCE_CONTEXT_HEADER]:fixture.jobs[0].source_context}});
  });
  vi.stubGlobal("fetch", fetch);
  await new ActeonClient("http://acteon").agentServiceSendMessage("prod", "acme", "notifier", {}, {
    executionContext: fixture.parent.execution_context,
    permits: fixture.parent.permits.map((p: {id: string; accepted_revision: number}) => ({id:p.id, acceptedRevision:p.accepted_revision})),
  });
  expect(fetch).toHaveBeenCalledTimes(1);
});
it("sends peer tool input without authority fields and validates the durable receipt", async () => {
  const source = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  const fetch = vi.fn(async (input: string, init: RequestInit) => {
    expect(input).toContain("/tasks/job-1/peers/team%2Fresolver/diagnose/");
    const headers = init.headers as Record<string, string>;
    expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBeUndefined();
    expect(headers[AGENT_EXECUTION_CONTEXT_HEADER]).toBeUndefined();
    expect(headers["x-acteon-execution-permits"]).toBeUndefined();
    if (input.endsWith(":refresh")) expect(init.body).toBeUndefined();
    else expect(JSON.parse(init.body as string)).toEqual({message:{messageId:"peer-1"}});
    return new Response(JSON.stringify({submission_id:"f47ac10b-58cc-5372-a567-0e02b2c3d479",status:{state:"accepted",task:fixture.jobs[1].task}}), {headers:{"a2a-version":"1.0"}});
  });
  vi.stubGlobal("fetch", fetch);
  const receipt = await new ActeonClient("http://acteon").agentServiceSendPeer(source, "team/resolver", "diagnose", {messageId:"peer-1"});
  expect(receipt.status.state).toBe("accepted");
  const refreshed = await new ActeonClient("http://acteon").agentServiceRefreshPeer(source, "team/resolver", "diagnose", receipt);
  expect(refreshed.submissionId).toBe(receipt.submissionId);
  expect(fetch.mock.calls[1][0]).toContain("/submissions/f47ac10b-58cc-5372-a567-0e02b2c3d479:refresh");
  expect(fetch).toHaveBeenCalledTimes(2);
});
it("discovers only strict safe peer options without authority fields", async () => {
  const source = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  const fetch = vi.fn(async (input: string, init: RequestInit) => {
    expect(input).toContain("/tasks/job-1/peers?skill=diagnose");
    const headers = init.headers as Record<string, string>;
    expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBeUndefined();
    expect(headers[AGENT_EXECUTION_CONTEXT_HEADER]).toBeUndefined();
    expect(headers["x-acteon-execution-permits"]).toBeUndefined();
    return new Response(JSON.stringify({peers:[{agent_id:"resolver",skill:"diagnose",description_untrusted:"Investigates incidents",card_version:"v1",binding_digest:"a".repeat(64),checked_at_ms:42}]}), {headers:{"a2a-version":"1.0"}});
  });
  vi.stubGlobal("fetch", fetch);
  const peers = await new ActeonClient("http://acteon").agentServiceDiscoverPeers(source, "diagnose");
  expect(peers).toEqual([{agentId:"resolver",skill:"diagnose",descriptionUntrusted:"Investigates incidents",cardVersion:"v1",bindingDigest:"a".repeat(64),checkedAtMs:42}]);
  expect(Object.keys(peers[0]!)).not.toContain("endpoint");
});
it("cancels an accepted peer at most once and validates restricted task identity", async () => {
  const source = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  const peer = { submissionId: "f47ac10b-58cc-5372-a567-0e02b2c3d479", status: { state: "accepted" as const, task: fixture.jobs[1].task } };
  const task = structuredClone(fixture.jobs[1].task);
  const fetch = vi.fn(async (input: string, init: RequestInit) => {
    expect(input).toContain("/submissions/f47ac10b-58cc-5372-a567-0e02b2c3d479:cancel");
    expect(init.body).toBeUndefined();
    return new Response(JSON.stringify({ submission_id: peer.submissionId, cancellation_id: "67e55044-10b1-526f-9247-bb680e5fe0c8", status: { state: "restricted", task } }), { headers: { "a2a-version": "1.0" } });
  });
  vi.stubGlobal("fetch", fetch);
  const receipt = await new ActeonClient("http://acteon").agentServiceCancelPeer(source, "team/resolver", "diagnose", peer);
  expect(receipt.status.state).toBe("restricted");
  const malformedTask = structuredClone(task) as Record<string, unknown>;
  delete malformedTask.status;
  expect(() => agentPeerCancelReceipt({ submission_id: peer.submissionId, cancellation_id: "67e55044-10b1-526f-9247-bb680e5fe0c8", status: { state: "restricted", task: malformedTask } }, "prod", "acme", peer)).toThrow("missing or malformed");
  await expect(new ActeonClient("http://acteon").agentServiceCancelPeer(source, "team/resolver", "diagnose", { ...peer, status: { state: "accepted", task: { ...peer.status.task, id: "" } } })).rejects.toThrow("identity mismatch");
  expect(fetch).toHaveBeenCalledTimes(1);
});
it("continues peer challenges without client-supplied bindings and retains direct source context", async () => {
  const source = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  const peer = { submissionId: "f47ac10b-58cc-5372-a567-0e02b2c3d479", status: { state: "accepted" as const, task: fixture.jobs[1].task } };
  const message = { messageId: "answer-1", role: "user", parts: [{ kind: "text", text: "proceed" }] };
  const fetch = vi.fn(async (input: string, init: RequestInit) => {
    expect(JSON.parse(init.body as string)).toEqual({ message });
    const headers = init.headers as Record<string, string>;
    if (input.includes("/submissions/")) {
      expect(input).toContain(peer.submissionId + "/message:send");
      expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBeUndefined();
      return new Response(JSON.stringify({ submission_id: peer.submissionId, continuation_id: "67e55044-10b1-526f-9247-bb680e5fe0c8", status: { state: "accepted", task: peer.status.task, progress_cursor: "\"job-2:2\"" } }), { headers: { "a2a-version": "1.0" } });
    }
    expect(input).toContain("/tasks/job-1/message:send");
    expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBe(source.sourceContext);
    return new Response(JSON.stringify(source.task), { headers: { "a2a-version": "1.0" } });
  });
  vi.stubGlobal("fetch", fetch);
  const client = new ActeonClient("http://acteon");
  const continued = await client.agentServiceContinuePeer(source, "team/resolver", "diagnose", peer, message);
  expect(continued.status.state).toBe("accepted");
  await expect(client.agentServiceContinuePeer(source, "team/resolver", "diagnose", peer, { ...message, taskId: "forged" })).rejects.toThrow("unbound user response");
  expect((await client.agentServiceContinueTask(source, message)).id).toBe("job-1");
  expect(fetch).toHaveBeenCalledTimes(2);
});
it("keeps authorization credentials out of both typed calls", async () => {
  const source = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  const fetch = vi.fn(async (input: string, init: RequestInit) => {
    const body = JSON.parse(init.body as string);
    const headers = init.headers as Record<string, string>;
    if (input.endsWith("authorization:request")) {
      expect(body).toEqual({ authorizationRequestId: "opaque-flow-42" });
      expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBeUndefined();
    } else {
      expect(input).toMatch(/authorization:resolve$/);
      expect(body).toEqual({ challengeId: "challenge-42" });
      expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBe(source.sourceContext);
    }
    return new Response(JSON.stringify(source.task), { headers: { "a2a-version": "1.0" } });
  });
  vi.stubGlobal("fetch", fetch);
  const client = new ActeonClient("http://acteon");
  await client.agentServiceRequestAuthorization("prod", "acme", "notifier", "job-1", "opaque-flow-42");
  await client.agentServiceResolveAuthorization(source, "challenge-42");
  expect(fetch).toHaveBeenCalledTimes(2);
});
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
  expect(stopped.map(r => r.providerAbort?.state)).toEqual(["uncertain", "restricted_only"]);
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
  for (const payload of [null, {}, {task: fixture.jobs[0].task, future_starts_blocked: false}, {task: fixture.jobs[0].task, future_starts_blocked: "true"}, {task: fixture.jobs[1].task, future_starts_blocked: true}, {task: fixture.jobs[0].task, future_starts_blocked: true, provider_abort: {state: "uncertain", attempt_id: "bad"}}]) {
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

it("requires canonical lowercase UUIDv5 provider-abort attempts", () => {
  expect(agentProviderAbort(null)).toBeUndefined();
  for (const attempt_id of ["F47AC10B-58CC-5372-A567-0E02B2C3D479", "f47ac10b-58cc-4372-a567-0e02b2c3d479"]) {
    expect(() => agentProviderAbort({ state: "uncertain", attempt_id })).toThrow("malformed");
  }
});
