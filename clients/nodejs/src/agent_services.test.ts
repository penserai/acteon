import { credentialFetch } from "./test_identity.js";
import { afterEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { ActeonClient } from "./client.js";
import { AGENT_EXECUTION_CONTEXT_HEADER, AGENT_SOURCE_CONTEXT_HEADER, agentPeerCancelReceipt, agentProviderAbort } from "./agent_services.js";
import { HttpError } from "./errors.js";
const fixture = JSON.parse(readFileSync(new URL("../../contract-fixtures/agent-services.json", import.meta.url), "utf8"));
afterEach(() => vi.unstubAllGlobals());
it("streams official exact-agent A2A frames with host-owned provenance", async () => {
  const source = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  const fetch = vi.fn(async (input: string, init: RequestInit) => {
    expect(input).toBe("http://acteon/a2a/agents/notifier");
    expect(init.redirect).toBe("error");
    const headers = init.headers as Record<string, string>;
    expect(headers.Accept).toBe("text/event-stream");
    expect(headers["A2A-Version"]).toBe("1.0");
    const body = JSON.parse(init.body as string);
    if (body.method === "SubscribeToTask") {
      expect(body.params).toEqual({ id: "job-1" });
      expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBe(source.sourceContext);
    } else {
      expect(body.method).toBe("SendStreamingMessage");
      expect(body.params.message.messageId).toBe("m1");
      expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBeUndefined();
    }
    const frame = { jsonrpc: "2.0", id: body.id, result: { task: { id: "job-1" } } };
    return new Response(`: keepalive\n\ndata: ${JSON.stringify(frame)}\n\n`, {
      headers: {
        "a2a-version": "1.0",
        "content-type": "text/event-stream",
        ...(body.method === "SendStreamingMessage" ? { [AGENT_SOURCE_CONTEXT_HEADER]: source.sourceContext } : {}),
      },
    });
  });
  vi.stubGlobal("fetch", credentialFetch(fetch));
  const client = new ActeonClient("http://acteon", { apiKey: "caller-key" });
  const sent = [];
  for await (const frame of client.agentServiceSendStreamingMessage("notifier", { messageId: "m1" })) sent.push(frame);
  const subscribed = [];
  for await (const frame of client.agentServiceSubscribeTask(source)) subscribed.push(frame);
  expect(sent).toEqual([{ result: { task: { id: "job-1" } }, sourceContext: source.sourceContext }]);
  expect(subscribed).toEqual([{ result: { task: { id: "job-1" } } }]);
  expect(fetch).toHaveBeenCalledTimes(2);
});

it("rejects a mismatched A2A stream request identity", async () => {
  vi.stubGlobal("fetch", credentialFetch(async () => new Response(
    'data: {"jsonrpc":"2.0","id":"foreign","result":{}}\n\n',
    { headers: { "a2a-version": "1.0", "content-type": "text/event-stream", [AGENT_SOURCE_CONTEXT_HEADER]: fixture.jobs[0].source_context } },
  )));
  const consume = async () => {
    for await (const _frame of new ActeonClient("http://acteon").agentServiceSendStreamingMessage("notifier", { messageId: "m1" })) { /* consume */ }
  };
  await expect(consume()).rejects.toThrow("request identity mismatch");
});

it("requests official exact-agent cancellation with retained source provenance", async () => {
  const source = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  const fetch = vi.fn(async (input: string, init: RequestInit) => {
    expect(input).toBe("http://acteon/a2a/agents/notifier");
    expect(init.redirect).toBe("error");
    expect((init.headers as Record<string, string>)[AGENT_SOURCE_CONTEXT_HEADER]).toBe(source.sourceContext);
    const body = JSON.parse(init.body as string);
    expect(body).toEqual({ jsonrpc: "2.0", id: "cancel:job-1", method: "CancelTask", params: { id: "job-1" } });
    return new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, result: { id: "job-1", status: { state: "TASK_STATE_CANCELED" }, metadata: { acteonCancellation: { state: "confirmed" } } } }), { headers: { "a2a-version": "1.0" } });
  });
  vi.stubGlobal("fetch", credentialFetch(fetch));
  const task = await new ActeonClient("http://acteon").agentServiceCancelTask(source);
  expect(task.id).toBe("job-1");
  expect((task.metadata as { acteonCancellation: { state: string } }).acteonCancellation.state).toBe("confirmed");
  expect(fetch).toHaveBeenCalledTimes(1);
});

it("lists exact-agent tasks with a typed filter-bound page", async () => {
  const source = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  const fetch = vi.fn(async (input: string, init: RequestInit) => {
    expect(input).toBe("http://acteon/a2a/agents/notifier");
    expect(init.redirect).toBe("error");
    expect((init.headers as Record<string, string>)[AGENT_SOURCE_CONTEXT_HEADER]).toBe(source.sourceContext);
    const body = JSON.parse(init.body as string);
    expect(body).toEqual({ jsonrpc: "2.0", id: "list:notifier", method: "ListTasks", params: { status: "TASK_STATE_WORKING", pageSize: 1 } });
    return new Response(JSON.stringify({
      jsonrpc: "2.0", id: body.id,
      result: { tasks: [{ id: "job-1", status: { state: "TASK_STATE_WORKING" } }], nextPageToken: "next-1", pageSize: 1, totalSize: 2 },
    }), { headers: { "a2a-version": "1.0" } });
  });
  vi.stubGlobal("fetch", credentialFetch(fetch));
  const page = await new ActeonClient("http://acteon").agentServiceListTasks("notifier", {
    source, status: "TASK_STATE_WORKING", pageSize: 1,
  });
  expect(page.tasks[0]?.id).toBe("job-1");
  expect(page.nextPageToken).toBe("next-1");
  expect(page.totalSize).toBe(2);
});

it("manages exact-agent push configurations with task-bound typed responses", async () => {
  const source = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  const stored = { id: "push-1", taskId: "job-1", url: "https://hooks.example/a2a", tokenRef: { provider: "vault", key: "callbacks/token", version: "v2" }, authentication: { scheme: "Bearer", credentialsRef: { provider: "vault", key: "callbacks/auth", version: "v2" } } };
  const methods: string[] = [];
  const fetch = vi.fn(async (input: string, init: RequestInit) => {
    expect(input).toBe("http://acteon/a2a/agents/notifier");
    expect(init.redirect).toBe("error");
    expect((init.headers as Record<string, string>)[AGENT_SOURCE_CONTEXT_HEADER]).toBe(source.sourceContext);
    const body = JSON.parse(init.body as string);
    methods.push(body.method);
    expect(body.params.taskId).toBe("job-1");
    const result = body.method === "ListTaskPushNotificationConfigs"
      ? { configs: [stored], nextPageToken: "next" }
      : body.method === "DeleteTaskPushNotificationConfig" ? {} : stored;
    return new Response(JSON.stringify({ jsonrpc: "2.0", id: body.id, result }), { headers: { "a2a-version": "1.0" } });
  });
  vi.stubGlobal("fetch", credentialFetch(fetch));
  const client = new ActeonClient("http://acteon");
  const created = await client.agentServiceCreatePushConfig("notifier", "job-1", { id: "push-1", url: stored.url, tokenRef: stored.tokenRef, authentication: stored.authentication }, source);
  expect(created.authentication?.credentialsRef?.key).toBe("callbacks/auth");
  expect(created.tokenRef?.version).toBe("v2");
  expect((await client.agentServiceGetPushConfig("notifier", "job-1", "push-1", source)).id).toBe("push-1");
  expect((await client.agentServiceListPushConfigs("notifier", "job-1", { pageSize: 1, source })).nextPageToken).toBe("next");
  await client.agentServiceDeletePushConfig("notifier", "job-1", "push-1", source);
  expect(methods).toEqual(["CreateTaskPushNotificationConfig", "GetTaskPushNotificationConfig", "ListTaskPushNotificationConfigs", "DeleteTaskPushNotificationConfig"]);
  expect(fetch).toHaveBeenCalledTimes(4);
  await expect(client.agentServiceGetPushConfig("notifier", "job-2", "push-1", source)).rejects.toThrow("does not match");
  expect(fetch).toHaveBeenCalledTimes(4);
});

it("carries an opaque governed parent with explicit permits", async () => {
  const fetch = vi.fn(async (_input: string, init: RequestInit) => {
    const headers = init.headers as Record<string, string>;
    expect(headers[AGENT_EXECUTION_CONTEXT_HEADER]).toBe(fixture.parent.execution_context);
    expect(JSON.parse(headers["x-acteon-execution-permits"])).toEqual(fixture.parent.permits);
    expect(headers[AGENT_SOURCE_CONTEXT_HEADER]).toBeUndefined();
    return new Response(JSON.stringify(fixture.jobs[0].task), {headers:{"a2a-version":"1.0", [AGENT_SOURCE_CONTEXT_HEADER]:fixture.jobs[0].source_context}});
  });
  vi.stubGlobal("fetch", credentialFetch(fetch));
  await new ActeonClient("http://acteon").agentServiceSendMessage("notifier", {}, {
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
  vi.stubGlobal("fetch", credentialFetch(fetch));
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
  vi.stubGlobal("fetch", credentialFetch(fetch));
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
  vi.stubGlobal("fetch", credentialFetch(fetch));
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
  vi.stubGlobal("fetch", credentialFetch(fetch));
  const client = new ActeonClient("http://acteon");
  const continued = await client.agentServiceContinuePeer(source, "team/resolver", "diagnose", peer, message);
  expect(continued.status.state).toBe("accepted");
  await expect(client.agentServiceContinuePeer(source, "team/resolver", "diagnose", peer, { ...message, taskId: "forged" })).rejects.toThrow("unbound user response");
  expect((await client.agentServiceContinueTask(source, message)).id).toBe("job-1");
  expect(fetch).toHaveBeenCalledTimes(2);
});
it("resolves peer authorization with only an opaque challenge selector", async () => {
  const source = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  const peer = { submissionId: "f47ac10b-58cc-5372-a567-0e02b2c3d479", status: { state: "accepted" as const, task: fixture.jobs[1].task } };
  const fetch = vi.fn(async (input: string, init: RequestInit) => {
    expect(input).toContain(peer.submissionId + "/authorization:resolve");
    expect(JSON.parse(init.body as string)).toEqual({ challengeId: "challenge-42" });
    expect((init.headers as Record<string, string>)[AGENT_SOURCE_CONTEXT_HEADER]).toBeUndefined();
    return new Response(JSON.stringify({ submission_id: peer.submissionId, authorization_id: "67e55044-10b1-526f-9247-bb680e5fe0c8", status: { state: "resolved", task: { ...peer.status.task, status: { state: "working" } }, progress_cursor: '"job-2:2"' } }), { headers: { "a2a-version": "1.0" } });
  });
  vi.stubGlobal("fetch", credentialFetch(fetch));
  const receipt = await new ActeonClient("http://acteon").agentServiceAuthorizePeer(source, "team/resolver", "diagnose", peer, "challenge-42");
  expect(receipt.status.state).toBe("resolved");
  expect(fetch).toHaveBeenCalledTimes(1);
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
  vi.stubGlobal("fetch", credentialFetch(fetch));
  const client = new ActeonClient("http://acteon");
  await client.agentServiceRequestAuthorization("notifier", "job-1", "opaque-flow-42");
  await client.agentServiceResolveAuthorization(source, "challenge-42");
  expect(fetch).toHaveBeenCalledTimes(2);
});
it("keeps concurrent job headers separate and ignores mutable task metadata", async () => {
  vi.stubGlobal("fetch", credentialFetch(async (input: string, init: RequestInit) => {
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
  }));
  const client = new ActeonClient("http://acteon", {apiKey:"caller-key"});
  const receipts = await Promise.all([1,2].map(i => client.agentServiceSendMessage("notifier", {messageId:`m${i}`})));
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
    vi.stubGlobal("fetch", credentialFetch(fetch));
    await expect(new ActeonClient("http://acteon").agentServiceSendMessage("notifier", {})).rejects.toThrow("source context");
    expect(fetch).toHaveBeenCalledTimes(1);
  }
});
it("preserves typed HTTP statuses with one request", async () => {
  for (const status of [...fixture.error_statuses,307]) {
    const fetch = vi.fn(async () => new Response("denied", {status}));
    vi.stubGlobal("fetch", credentialFetch(fetch));
    await expect(new ActeonClient("http://acteon").agentServiceSendMessage("notifier", {})).rejects.toEqual(new HttpError(status,"denied"));
    expect(fetch).toHaveBeenCalledTimes(1);
  }
});
it("escapes route segments and rejects mismatched observed identity", async () => {
  const fetch = vi.fn(async (input: string) => {
    expect(input).toContain("/agents/team%2Fchild%20%3F%23/v1/tasks/job-1");
    return new Response(JSON.stringify(fixture.jobs[1].task), {headers:{"a2a-version":"1.0"}});
  });
  vi.stubGlobal("fetch", credentialFetch(fetch));
  await expect(new ActeonClient("http://acteon").agentServiceGetTask({namespace:"prod", tenant:"acme", agent:"team/child ?#", taskId:"job-1", sourceContext:fixture.jobs[0].source_context, task:fixture.jobs[0].task})).rejects.toThrow("identity mismatch");
});

it("rejects false stop acknowledgements and foreign task identities without retries", async () => {
  const receipt = { namespace: "prod", tenant: "acme", agent: "notifier", taskId: "job-1", sourceContext: fixture.jobs[0].source_context, task: fixture.jobs[0].task };
  for (const payload of [null, {}, {task: fixture.jobs[0].task, future_starts_blocked: false}, {task: fixture.jobs[0].task, future_starts_blocked: "true"}, {task: fixture.jobs[1].task, future_starts_blocked: true}, {task: fixture.jobs[0].task, future_starts_blocked: true, provider_abort: {state: "uncertain", attempt_id: "bad"}}]) {
    const fetch = vi.fn(async () => new Response(JSON.stringify(payload), {headers: {"a2a-version":"1.0"}}));
    vi.stubGlobal("fetch", credentialFetch(fetch));
    await expect(new ActeonClient("http://acteon").agentServiceStopTask(receipt)).rejects.toThrow();
    expect(fetch).toHaveBeenCalledTimes(1);
  }
  for (const status of [...fixture.error_statuses, 307]) {
    const fetch = vi.fn(async () => new Response("denied", {status}));
    vi.stubGlobal("fetch", credentialFetch(fetch));
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
