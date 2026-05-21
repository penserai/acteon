/**
 * A2A SDK — factory + URL/header smoke tests.
 *
 * Live HTTP tests would need a running Acteon server with A2A
 * enabled; these exercise the wire surface of the new `a2a.ts`
 * module + the `ActeonClient` methods via a captured `fetch`
 * stub. The contract under test: factories produce the dict
 * shapes the server expects, URLs are spec-correct, and the
 * `A2A-Version` header lands on every authenticated call.
 */

import { describe, it, expect } from "vitest";
import {
  A2A_PROTOCOL_VERSION,
  A2A_VERSION_HEADER,
  makeMessage,
  makePartData,
  makePartText,
  makePartUrl,
  makePushConfig,
} from "./a2a.js";
import { ActeonClient } from "./client.js";

// ---------------------------------------------------------------------
// Factory helpers
// ---------------------------------------------------------------------

describe("a2a factory helpers", () => {
  it("makePartText", () => {
    expect(makePartText("hi")).toEqual({ text: "hi" });
  });

  it("makePartUrl", () => {
    expect(makePartUrl("https://x/y")).toEqual({ url: "https://x/y" });
  });

  it("makePartData defaults mediaType to application/json", () => {
    const p = makePartData({ k: 1 });
    expect(p).toEqual({ data: { k: 1 }, mediaType: "application/json" });
  });

  it("makePartData honors custom mediaType", () => {
    const p = makePartData({ k: 1 }, "application/cloudevents+json");
    expect((p as Record<string, unknown>).mediaType).toEqual(
      "application/cloudevents+json",
    );
  });

  it("makeMessage minimal omits taskId and contextId", () => {
    const m = makeMessage("m-1", "user", [makePartText("hi")]);
    expect(m).toEqual({
      messageId: "m-1",
      role: "user",
      parts: [{ text: "hi" }],
    });
    // Absent vs. empty matters server-side; the helper must NOT
    // surface either key as `undefined` either.
    expect((m as Record<string, unknown>).taskId).toBeUndefined();
    expect((m as Record<string, unknown>).contextId).toBeUndefined();
  });

  it("makeMessage threads taskId into history", () => {
    const m = makeMessage("m-2", "user", [makePartText("yes")], {
      taskId: "task-alpha",
    });
    expect((m as Record<string, unknown>).taskId).toEqual("task-alpha");
  });

  it("makePushConfig minimal includes only url", () => {
    expect(makePushConfig("https://hook/x")).toEqual({ url: "https://hook/x" });
  });

  it("makePushConfig full carries id, token, authentication", () => {
    const cfg = makePushConfig("https://hook/x", {
      id: "cfg-1",
      token: "t",
      authentication: { schemes: ["api-key"] },
    });
    expect((cfg as Record<string, unknown>).id).toEqual("cfg-1");
    expect((cfg as Record<string, unknown>).token).toEqual("t");
    expect((cfg as Record<string, unknown>).authentication).toEqual({
      schemes: ["api-key"],
    });
  });
});

// ---------------------------------------------------------------------
// Client method URL + header behaviour via a captured `fetch`
// ---------------------------------------------------------------------

interface CapturedFetch {
  url: string;
  init: RequestInit | undefined;
}

/** Replace `globalThis.fetch` with a stub that records the call and
 *  returns the supplied response. Returns the captured-calls array
 *  plus a restore function. */
function captureFetch(
  status: number = 200,
  body: unknown = {},
): { calls: CapturedFetch[]; restore: () => void } {
  const calls: CapturedFetch[] = [];
  const original = globalThis.fetch;
  globalThis.fetch = (async (
    input: string | URL | Request,
    init?: RequestInit,
  ): Promise<Response> => {
    calls.push({ url: String(input), init });
    return new Response(JSON.stringify(body), {
      status,
      headers: { "Content-Type": "application/json" },
    });
  }) as typeof globalThis.fetch;
  return {
    calls,
    restore: () => {
      globalThis.fetch = original;
    },
  };
}

function headerValue(init: RequestInit | undefined, name: string): string | null {
  const h = init?.headers;
  if (!h) return null;
  if (h instanceof Headers) return h.get(name);
  if (Array.isArray(h)) {
    const entry = h.find(
      ([k]) => k.toLowerCase() === name.toLowerCase(),
    );
    return entry ? entry[1] : null;
  }
  // Record<string, string>
  const obj = h as Record<string, string>;
  for (const [k, v] of Object.entries(obj)) {
    if (k.toLowerCase() === name.toLowerCase()) return v;
  }
  return null;
}

describe("a2a client URLs and headers", () => {
  it("a2aSendMessage hits the right URL + carries A2A-Version", async () => {
    const { calls, restore } = captureFetch(200, {
      id: "task-1",
      status: { state: "submitted" },
    });
    try {
      const c = new ActeonClient("http://x", { apiKey: "k" });
      const msg = makeMessage("m-1", "user", [makePartText("hi")]);
      await c.a2aSendMessage("ns", "tnt", msg);
      expect(calls).toHaveLength(1);
      expect(calls[0].url).toEqual("http://x/a2a/ns/tnt/v1/message:send");
      expect(calls[0].init?.method).toEqual("POST");
      expect(headerValue(calls[0].init, A2A_VERSION_HEADER)).toEqual(
        A2A_PROTOCOL_VERSION,
      );
      expect(headerValue(calls[0].init, "Authorization")).toEqual("Bearer k");
      // Body must wrap message in { message: ... } per spec.
      const body = JSON.parse(String(calls[0].init?.body));
      expect(body).toEqual({ message: msg });
    } finally {
      restore();
    }
  });

  it("a2aCancelTask keeps the :cancel verb in the final segment", async () => {
    const { calls, restore } = captureFetch(200, {
      id: "task-1",
      status: { state: "canceled" },
    });
    try {
      const c = new ActeonClient("http://x");
      await c.a2aCancelTask("ns", "tnt", "task-1");
      expect(calls[0].url).toEqual("http://x/a2a/ns/tnt/v1/tasks/task-1:cancel");
      expect(calls[0].init?.method).toEqual("POST");
    } finally {
      restore();
    }
  });

  it("a2aDeletePushConfig builds the nested URL", async () => {
    // 204 No Content can't carry a body in the fetch Response
    // constructor — use 200 + empty object for the mock and let
    // the helper just shrug at the unused payload.
    const { calls, restore } = captureFetch(200, {});
    try {
      const c = new ActeonClient("http://x");
      await c.a2aDeletePushConfig("ns", "tnt", "task-1", "cfg-a");
      expect(calls[0].url).toEqual(
        "http://x/a2a/ns/tnt/v1/tasks/task-1/pushNotificationConfigs/cfg-a",
      );
      expect(calls[0].init?.method).toEqual("DELETE");
    } finally {
      restore();
    }
  });

  it("a2aDiscoverAgent is unauthenticated (no Authorization header)", async () => {
    const { calls, restore } = captureFetch(200, { agent_id: "tenant" });
    try {
      const c = new ActeonClient("http://x", { apiKey: "k" });
      await c.a2aDiscoverAgent("ns", "tnt");
      expect(calls[0].url).toEqual("http://x/a2a/ns/tnt/.well-known/agent.json");
      // Discovery is anonymous per A2A spec — even when an API key is
      // configured on the client, this call must NOT carry it.
      expect(headerValue(calls[0].init, "Authorization")).toBeNull();
    } finally {
      restore();
    }
  });

  it("a2aGetAuthenticatedExtendedCard uses the JSON-RPC envelope", async () => {
    const { calls, restore } = captureFetch(200, {
      jsonrpc: "2.0",
      id: 1,
      result: { agent_id: "tenant", capabilities: {} },
    });
    try {
      const c = new ActeonClient("http://x", { apiKey: "k" });
      const card = await c.a2aGetAuthenticatedExtendedCard("ns", "tnt");
      expect(calls[0].url).toEqual("http://x/a2a/ns/tnt");
      const body = JSON.parse(String(calls[0].init?.body));
      expect(body.jsonrpc).toEqual("2.0");
      expect(body.method).toEqual("agent/getAuthenticatedExtendedCard");
      // The mixin unwraps the envelope on the way out.
      expect(card).toEqual({ agent_id: "tenant", capabilities: {} });
    } finally {
      restore();
    }
  });

  it("path segments are percent-encoded", async () => {
    const { calls, restore } = captureFetch(200, {});
    try {
      const c = new ActeonClient("http://x");
      // A tenant id with a slash MUST be percent-encoded so it
      // cannot leak into additional path components.
      await c.a2aGetTask("ns/escape", "tnt", "t");
      expect(calls[0].url).toContain("/a2a/ns%2Fescape/tnt/v1/tasks/t");
    } finally {
      restore();
    }
  });
});

// ---------------------------------------------------------------------
// JSON-RPC error unwrap
// ---------------------------------------------------------------------

describe("a2a JSON-RPC error unwrap", () => {
  it("surfaces JSON-RPC errors as ApiError", async () => {
    const { restore } = captureFetch(200, {
      jsonrpc: "2.0",
      id: 1,
      error: { code: -32001, message: "task not found" },
    });
    try {
      const c = new ActeonClient("http://x", { apiKey: "k" });
      await expect(
        c.a2aGetAuthenticatedExtendedCard("ns", "tnt"),
      ).rejects.toThrow(/task not found/);
    } finally {
      restore();
    }
  });
});
