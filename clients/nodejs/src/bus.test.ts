/**
 * Phase 8b: Node SDK bus surface tests.
 *
 * Live HTTP tests would need a running Acteon instance with the
 * bus feature enabled; these tests exercise serde (request body
 * builders + response parsers) and URL encoding. The contract
 * under test: every wire field round-trips, optional fields drop
 * cleanly, and path segments are properly percent-encoded.
 */

import { describe, it, expect, assert } from "vitest";
import {
  createBusTopicBody,
  publishBusMessageBody,
  createBusSubscriptionBody,
  registerBusSchemaBody,
  registerBusAgentBody,
  createBusConversationBody,
  appendBusConversationMessageBody,
  postBusToolCallBody,
  postBusToolResultBody,
  postBusStreamChunkBody,
  postBusStreamEndBody,
  busApprovalDecisionBody,
  busToolResultLookupParams,
  parseBusTopic,
  parseBusSubscription,
  parseBusLag,
  parseBusSchema,
  parseBusAgent,
  parseBusConversation,
  parseBusReplayResponse,
  parseBusToolEnvelopeReceipt,
  parseBusToolResultLookup,
  parseBusStreamEnvelopeReceipt,
  parseBusApprovalView,
  parseBusApprovalDecisionResponse,
  parseBusApprovalParkedReceipt,
  parseBusConsumedMessage,
  parsePublishReceipt,
  parseStreamChunkEnvelope,
  parseStreamEndEnvelope,
} from "./bus_models.js";
import { ActeonClient } from "./client.js";

describe("bus request body builders", () => {
  it("create topic minimal", () => {
    const body = createBusTopicBody({ name: "t", namespace: "n", tenant: "te" });
    assert.deepEqual(body, { name: "t", namespace: "n", tenant: "te" });
  });

  it("create topic snake_cases optional fields", () => {
    const body = createBusTopicBody({
      name: "t",
      namespace: "n",
      tenant: "te",
      partitions: 4,
      replicationFactor: 2,
      retentionMs: 86_400_000,
      labels: { env: "prod" },
    });
    assert.equal(body.replication_factor, 2);
    assert.equal(body.retention_ms, 86_400_000);
    assert.deepEqual(body.labels, { env: "prod" });
  });

  it("publish message — payload is required", () => {
    const body = publishBusMessageBody({ topic: "n.te.t", payload: { x: 1 } });
    assert.deepEqual(body.payload, { x: 1 });
  });

  it("subscription request snake-cases ack-mode + dlq", () => {
    const body = createBusSubscriptionBody({
      id: "s1",
      topic: "n.te.t",
      namespace: "n",
      tenant: "te",
      ackMode: "manual",
      ackTimeoutMs: 30_000,
      deadLetterTopic: "n.te.t-dlq",
    });
    assert.equal(body.ack_mode, "manual");
    assert.equal(body.ack_timeout_ms, 30_000);
    assert.equal(body.dead_letter_topic, "n.te.t-dlq");
  });

  it("schema request preserves arbitrary body", () => {
    const body = registerBusSchemaBody({
      subject: "orders",
      namespace: "n",
      tenant: "te",
      body: { type: "object", properties: { x: { type: "number" } } },
    });
    assert.deepEqual(body.body, {
      type: "object",
      properties: { x: { type: "number" } },
    });
  });

  it("agent request snake-cases id + ttl", () => {
    const body = registerBusAgentBody({
      agentId: "a1",
      namespace: "n",
      tenant: "te",
      capabilities: ["tools.calendar"],
      heartbeatTtlMs: 30_000,
    });
    assert.equal(body.agent_id, "a1");
    assert.equal(body.heartbeat_ttl_ms, 30_000);
  });

  it("conversation request snake-cases id + topic-subject", () => {
    const body = createBusConversationBody({
      conversationId: "c1",
      namespace: "n",
      tenant: "te",
      participants: ["a1", "a2"],
      topicSubject: "agents.demo.thread",
    });
    assert.equal(body.conversation_id, "c1");
    assert.equal(body.topic_subject, "agents.demo.thread");
    assert.deepEqual(body.participants, ["a1", "a2"]);
  });

  it("append message — sender + headers optional", () => {
    const minimal = appendBusConversationMessageBody({ payload: { ok: true } });
    assert.equal("sender" in minimal, false);
    assert.equal("headers" in minimal, false);

    const full = appendBusConversationMessageBody({
      payload: {},
      sender: "a1",
      headers: { "trace-id": "x" },
    });
    assert.equal(full.sender, "a1");
    assert.deepEqual(full.headers, { "trace-id": "x" });
  });

  it("tool call body — basic", () => {
    const body = postBusToolCallBody({ callId: "call-1", tool: "calendar.list" });
    assert.equal(body.call_id, "call-1");
    assert.deepEqual(body.arguments, {});
    assert.equal("require_approval" in body, false);
  });

  it("tool call body — Phase 6c gate", () => {
    const body = postBusToolCallBody({
      callId: "call-1",
      tool: "billing.charge",
      arguments: { usd: 42 },
      sender: "planner-1",
      requireApproval: true,
      approvalReason: "paid action",
      approvalTtlMs: 600_000,
    });
    assert.equal(body.require_approval, true);
    assert.equal(body.approval_reason, "paid action");
    assert.equal(body.approval_ttl_ms, 600_000);
  });

  it("tool result body — error case", () => {
    const body = postBusToolResultBody({
      callId: "call-1",
      status: "error",
      errorMessage: "upstream gave up",
    });
    assert.equal(body.status, "error");
    assert.equal(body.error_message, "upstream gave up");
  });

  it("stream chunk body", () => {
    const body = postBusStreamChunkBody({
      streamId: "s1",
      chunkSeq: 0,
      body: { token: "Once " },
    });
    assert.equal(body.stream_id, "s1");
    assert.equal(body.chunk_seq, 0);
    assert.deepEqual(body.body, { token: "Once " });
  });

  it("stream end body — complete", () => {
    const body = postBusStreamEndBody({
      streamId: "s1",
      chunkSeq: 5,
      status: "complete",
    });
    assert.equal(body.status, "complete");
  });

  it("approval decision body", () => {
    const body = busApprovalDecisionBody({
      decidedBy: "ops-1",
      decisionNote: "verified PO",
    });
    assert.equal(body.decided_by, "ops-1");
    assert.equal(body.decision_note, "verified PO");
  });

  it("tool-result lookup query string", () => {
    // Phase 10 dropped `asAgent` — read-side identity is grant-
    // derived now, not a query parameter.
    const params = busToolResultLookupParams({
      conversationId: "c1",
      cursor: "abc",
      timeoutMs: 5_000,
    });
    assert.equal(params.get("conversation_id"), "c1");
    assert.equal(params.get("cursor"), "abc");
    assert.equal(params.get("timeout_ms"), "5000");
    assert.equal(params.get("as_agent"), null);
  });
});

describe("bus response parsers", () => {
  it("topic — optional fields default to null/[]", () => {
    const t = parseBusTopic({
      name: "t",
      namespace: "n",
      tenant: "te",
      kafka_name: "n.te.t",
      partitions: 4,
      replication_factor: 2,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    });
    assert.equal(t.kafkaName, "n.te.t");
    assert.equal(t.schemaSubject, null);
    assert.deepEqual(t.labels, {});
  });

  it("subscription — full payload", () => {
    const s = parseBusSubscription({
      id: "s1",
      topic: "n.te.t",
      namespace: "n",
      tenant: "te",
      starting_offset: "latest",
      ack_mode: "manual",
      dead_letter_topic: "n.te.t-dlq",
      ack_timeout_ms: 30_000,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    });
    assert.equal(s.deadLetterTopic, "n.te.t-dlq");
    assert.equal(s.ackTimeoutMs, 30_000);
  });

  it("lag — partitions array", () => {
    const lag = parseBusLag({
      subscription_id: "s1",
      topic: "n.te.t",
      partitions: [
        { partition: 0, committed: 10, high_water_mark: 12, lag: 2 },
        { partition: 1, committed: 0, high_water_mark: 0, lag: 0 },
      ],
      total_lag: 2,
    });
    assert.equal(lag.totalLag, 2);
    assert.equal(lag.partitions.length, 2);
    assert.equal(lag.partitions[0].highWaterMark, 12);
  });

  it("schema — preserves body shape", () => {
    const s = parseBusSchema({
      subject: "orders",
      version: 3,
      namespace: "n",
      tenant: "te",
      body: { type: "object" },
      created_at: "2026-01-01T00:00:00Z",
    });
    assert.equal(s.version, 3);
    assert.deepEqual(s.body, { type: "object" });
  });

  it("agent — heartbeat may be null", () => {
    const a = parseBusAgent({
      agent_id: "a1",
      namespace: "n",
      tenant: "te",
      capabilities: [],
      inbox_topic: "n.te.agents.a1",
      status: "registered",
      heartbeat_ttl_ms: 30_000,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    });
    assert.equal(a.lastHeartbeatAt, null);
    assert.deepEqual(a.capabilities, []);
  });

  it("conversation — open default", () => {
    const c = parseBusConversation({
      conversation_id: "c1",
      namespace: "n",
      tenant: "te",
      participants: [],
      state: "open",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
    });
    assert.equal(c.state, "open");
    assert.deepEqual(c.participants, []);
  });

  it("replay — single message", () => {
    const r = parseBusReplayResponse({
      conversation_id: "c1",
      events_topic: "n.te.conversations-events",
      messages: [
        {
          partition: 0,
          offset: 7,
          produced_at: "2026-01-01T00:00:00Z",
          sender: "a1",
          payload: { text: "hi" },
          headers: { "acteon.envelope.kind": "tool_call" },
        },
      ],
      exit_reason: "limit",
    });
    assert.equal(r.messages.length, 1);
    assert.equal(r.messages[0].sender, "a1");
    assert.equal(r.exitReason, "limit");
  });

  it("tool-envelope receipt", () => {
    const r = parseBusToolEnvelopeReceipt({
      events_topic: "n.te.events",
      conversation_id: "c1",
      call_id: "call-1",
      partition: 0,
      offset: 42,
      produced_at: "2026-01-01T00:00:00Z",
      cursor: "eyIwIjogNDJ9",
    });
    assert.equal(r.cursor, "eyIwIjogNDJ9");
  });

  it("tool-result lookup nests result", () => {
    const l = parseBusToolResultLookup({
      call_id: "call-1",
      events_topic: "n.te.events",
      conversation_id: "c1",
      partition: 0,
      offset: 43,
      produced_at: "2026-01-01T00:00:00Z",
      result: {
        call_id: "call-1",
        status: "ok",
        output: { events: [] },
        created_at: "2026-01-01T00:00:00Z",
      },
    });
    assert.equal(l.result.status, "ok");
  });

  it("stream receipt", () => {
    const r = parseBusStreamEnvelopeReceipt({
      events_topic: "n.te.events",
      conversation_id: "c1",
      stream_id: "s1",
      chunk_seq: 0,
      partition: 0,
      offset: 5,
      produced_at: "2026-01-01T00:00:00Z",
      cursor: "abc",
    });
    assert.equal(r.streamId, "s1");
  });

  it("approval view — pending row has null decision fields", () => {
    const v = parseBusApprovalView({
      approval_id: "appr-1",
      namespace: "n",
      tenant: "te",
      conversation_id: "c1",
      correlation_token: "call-1",
      envelope_kind: "tool_call",
      status: "pending",
      created_at: "2026-01-01T00:00:00Z",
      expires_at: "2026-01-02T00:00:00Z",
      envelope: { kind: "tool_call" },
    });
    assert.equal(v.status, "pending");
    assert.equal(v.decidedBy, null);
    assert.equal(v.producedOffset, null);
  });

  it("decision response — approved with receipt", () => {
    const r = parseBusApprovalDecisionResponse({
      approval: {
        approval_id: "appr-1",
        namespace: "n",
        tenant: "te",
        conversation_id: "c1",
        correlation_token: "call-1",
        envelope_kind: "tool_call",
        status: "approved",
        created_at: "2026-01-01T00:00:00Z",
        expires_at: "2026-01-02T00:00:00Z",
        envelope: {},
        decided_by: "ops-1",
      },
      receipt: {
        events_topic: "n.te.events",
        conversation_id: "c1",
        call_id: "call-1",
        partition: 0,
        offset: 99,
        produced_at: "2026-01-01T00:00:01Z",
        cursor: "xx",
      },
    });
    assert.equal(r.approval.status, "approved");
    assert.notEqual(r.receipt, null);
    assert.equal(r.receipt!.offset, 99);
  });

  it("decision response — rejected has null receipt", () => {
    const r = parseBusApprovalDecisionResponse({
      approval: {
        approval_id: "appr-1",
        namespace: "n",
        tenant: "te",
        conversation_id: "c1",
        correlation_token: "call-1",
        envelope_kind: "tool_call",
        status: "rejected",
        created_at: "2026-01-01T00:00:00Z",
        expires_at: "2026-01-02T00:00:00Z",
        envelope: {},
        decided_by: "ops-1",
        decision_note: "scope too broad",
      },
      receipt: null,
    });
    assert.equal(r.approval.status, "rejected");
    assert.equal(r.receipt, null);
  });

  it("approval parked receipt", () => {
    const r = parseBusApprovalParkedReceipt({
      approval_id: "appr-1",
      namespace: "n",
      tenant: "te",
      conversation_id: "c1",
      correlation_token: "call-1",
      status: "pending",
      created_at: "2026-01-01T00:00:00Z",
      expires_at: "2026-01-02T00:00:00Z",
    });
    assert.equal(r.status, "pending");
    assert.equal(r.correlationToken, "call-1");
  });

  it("publish receipt", () => {
    const r = parsePublishReceipt({
      topic: "n.te.t",
      partition: 0,
      offset: 17,
      produced_at: "2026-01-01T00:00:00Z",
    });
    assert.equal(r.offset, 17);
  });
});

describe("busStreamConsumeUrl", () => {
  it("encodes path segments with embedded slashes + spaces", () => {
    const c = new ActeonClient("http://localhost:3000");
    const url = c.busStreamConsumeUrl(
      "agents/x",
      "demo",
      "thread/with/slashes",
      "story 1",
    );
    // The slashes inside segments must be %2F-encoded so they
    // don't escape into URL grammar.
    assert.match(url, /agents%2Fx/);
    assert.match(url, /thread%2Fwith%2Fslashes/);
    assert.match(url, /story%201/);
  });

  it("returns the canonical URL for simple segments", () => {
    const c = new ActeonClient("http://localhost:3000");
    const url = c.busStreamConsumeUrl("agents", "demo", "thread-1", "stream-1");
    assert.equal(
      url,
      "http://localhost:3000/v1/bus/streams/agents/demo/thread-1/stream-1",
    );
  });

  it("trims trailing slash from the base URL", () => {
    const c = new ActeonClient("http://localhost:3000/");
    const url = c.busStreamConsumeUrl("a", "b", "c", "d");
    // The constructor strips the trailing slash; consume URL inherits.
    assert.equal(url, "http://localhost:3000/v1/bus/streams/a/b/c/d");
  });
});

describe("SSE consumer DTOs", () => {
  it("BusConsumedMessage round-trips snake_case wire form", () => {
    const m = parseBusConsumedMessage({
      topic: "agents.demo.events",
      payload: { k: "v" },
      partition: 0,
      offset: 7,
      key: "alpha",
      headers: { trace: "abc" },
      timestamp: "2026-05-02T12:00:00Z",
    });
    assert.equal(m.topic, "agents.demo.events");
    assert.equal(m.offset, 7);
    assert.deepEqual(m.payload, { k: "v" });
    assert.equal(m.headers.trace, "abc");
  });

  it("BusConsumedMessage defaults headers to empty when absent", () => {
    const m = parseBusConsumedMessage({ topic: "t" });
    assert.deepEqual(m.headers, {});
    expect(m.partition).toBeUndefined();
    expect(m.offset).toBeUndefined();
  });

  it("StreamChunkEnvelope maps snake_case to camelCase", () => {
    const c = parseStreamChunkEnvelope({
      stream_id: "s1",
      chunk_seq: 3,
      body: { token: "hi" },
      sender: "agent-A",
      created_at: "2026-05-02T12:00:00Z",
    });
    assert.equal(c.streamId, "s1");
    assert.equal(c.chunkSeq, 3);
    assert.equal(c.sender, "agent-A");
    assert.deepEqual(c.body, { token: "hi" });
    assert.equal(c.createdAt, "2026-05-02T12:00:00Z");
  });

  it("StreamEndEnvelope round-trips error status with message", () => {
    const e = parseStreamEndEnvelope({
      stream_id: "s1",
      chunk_seq: 4,
      status: "error",
      error_message: "broker disconnected",
    });
    assert.equal(e.streamId, "s1");
    assert.equal(e.status, "error");
    assert.equal(e.errorMessage, "broker disconnected");
  });
});

describe("SSE consumer line-protocol", () => {
  // The bus SSE consumers split on `\n` for performance, so a trailing
  // `\r` (from intermediaries that normalise to CRLF) needs to be
  // stripped explicitly — otherwise the empty-line frame trigger
  // misses entirely and frames stop dispatching.

  it("consumeBusSubscription handles CRLF-terminated frames", async () => {
    const http = await import("node:http");
    const body =
      ":keep-alive\r\n\r\n" +
      "event: bus.message\r\nid: 5\r\n" +
      'data: {"topic":"agents.demo.events","offset":5}\r\n\r\n';
    const server = http.createServer((req, res) => {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write(body);
      res.end();
    });
    await new Promise<void>((resolve) => server.listen(0, resolve));
    const port = (server.address() as { port: number }).port;
    try {
      const c = new ActeonClient(`http://localhost:${port}`);
      const items: string[] = [];
      for await (const item of c.consumeBusSubscription("agent-A", {
        topic: "agents.demo.events",
      })) {
        items.push(item.kind);
        if (items.length >= 2) break;
      }
      assert.deepEqual(items, ["keepAlive", "message"]);
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });

  it("consumeBusStream handles CRLF-terminated chunk + end", async () => {
    const http = await import("node:http");
    const body =
      "event: bus.stream.chunk\r\nid: 0\r\n" +
      'data: {"stream_id":"s1","chunk_seq":3,"body":{"t":"hi"},"created_at":"2026-05-02T12:00:00Z"}\r\n\r\n' +
      "event: bus.stream.end\r\nid: 1\r\n" +
      'data: {"stream_id":"s1","chunk_seq":4,"status":"complete","created_at":"2026-05-02T12:00:01Z"}\r\n\r\n';
    const server = http.createServer((req, res) => {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write(body);
      res.end();
    });
    await new Promise<void>((resolve) => server.listen(0, resolve));
    const port = (server.address() as { port: number }).port;
    try {
      const c = new ActeonClient(`http://localhost:${port}`);
      const items: string[] = [];
      for await (const item of c.consumeBusStream("agents", "demo", "thread-1", "s1")) {
        items.push(item.kind);
      }
      assert.deepEqual(items, ["chunk", "end"]);
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});

describe("consumeBusSubscription reconnect", () => {
  it("yields a `reconnected` boundary item between attempts", async () => {
    const http = await import("node:http");
    let connectionCount = 0;
    const server = http.createServer((_req, res) => {
      connectionCount += 1;
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      const offset = connectionCount === 1 ? 1 : 2;
      res.write(
        `event: bus.message\nid: ${offset}\ndata: {"topic":"agents.demo.events","offset":${offset}}\n\n`,
      );
      res.end();
    });
    await new Promise<void>((resolve) => server.listen(0, resolve));
    const port = (server.address() as { port: number }).port;
    try {
      const c = new ActeonClient(`http://localhost:${port}`);
      const seen: string[] = [];
      let messageCount = 0;
      for await (const item of c.consumeBusSubscription("agent-A", {
        topic: "agents.demo.events",
        reconnect: { initialBackoffMs: 5, maxBackoffMs: 5, maxAttempts: 1 },
      })) {
        seen.push(item.kind);
        if (item.kind === "message") messageCount += 1;
        if (messageCount >= 2) break;
      }
      // Expect: message → reconnected → message.
      assert.ok(seen.includes("message"), `seen: ${seen.join(",")}`);
      assert.ok(seen.includes("reconnected"), `seen: ${seen.join(",")}`);
      assert.equal(connectionCount, 2);
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});
