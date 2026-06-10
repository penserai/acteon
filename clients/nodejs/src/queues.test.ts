/**
 * Worker task queue surface tests.
 *
 * Live HTTP tests would need a running Acteon instance with task
 * queues enabled; these exercise serde (request body builders +
 * response parsers) and the `ActeonClient` queue methods via a
 * captured `fetch` stub. The contract under test: every wire field
 * round-trips, optional fields drop cleanly, and URLs/bodies match
 * the `/v1/queues` lease protocol.
 */

import { describe, it, expect, assert } from "vitest";
import {
  completeTaskBody,
  enqueueTaskBody,
  failTaskBody,
  heartbeatTaskBody,
  parseTaskListResponse,
  parseWorkerTask,
  pollTasksBody,
} from "./queues.js";
import { ActeonClient } from "./client.js";
import { HttpError } from "./errors.js";

// ---------------------------------------------------------------------
// Request body builders
// ---------------------------------------------------------------------

describe("queue request body builders", () => {
  it("poll body minimal", () => {
    assert.deepEqual(pollTasksBody("ns", "t1"), { namespace: "ns", tenant: "t1" });
  });

  it("poll body snake-cases optional fields", () => {
    const body = pollTasksBody("ns", "t1", {
      maxTasks: 5,
      leaseSeconds: 120,
      workerId: "worker-7",
    });
    assert.equal(body.max_tasks, 5);
    assert.equal(body.lease_seconds, 120);
    assert.equal(body.worker_id, "worker-7");
  });

  it("enqueue body — payload required, max_attempts optional", () => {
    const minimal = enqueueTaskBody("ns", "t1", "send_email", { to: "x" });
    assert.deepEqual(minimal, {
      namespace: "ns",
      tenant: "t1",
      action_type: "send_email",
      payload: { to: "x" },
    });

    const full = enqueueTaskBody("ns", "t1", "send_email", {}, { maxAttempts: 3 });
    assert.equal(full.max_attempts, 3);
  });

  it("heartbeat body — extend_seconds optional", () => {
    const minimal = heartbeatTaskBody("ns", "t1", "lease-1");
    assert.deepEqual(minimal, {
      namespace: "ns",
      tenant: "t1",
      lease_token: "lease-1",
    });

    const full = heartbeatTaskBody("ns", "t1", "lease-1", 90);
    assert.equal(full.extend_seconds, 90);
  });

  it("complete body carries result verbatim", () => {
    const body = completeTaskBody("ns", "t1", "lease-1", { ok: true });
    assert.deepEqual(body, {
      namespace: "ns",
      tenant: "t1",
      lease_token: "lease-1",
      result: { ok: true },
    });
  });

  it("fail body carries error + retryable", () => {
    const body = failTaskBody("ns", "t1", "lease-1", "boom", false);
    assert.deepEqual(body, {
      namespace: "ns",
      tenant: "t1",
      lease_token: "lease-1",
      error: "boom",
      retryable: false,
    });
  });
});

// ---------------------------------------------------------------------
// Response parsers
// ---------------------------------------------------------------------

function wireTask(
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    task_id: "task-1",
    queue: "billing",
    action_type: "charge",
    payload: { amount: 42 },
    status: "leased",
    attempt: 1,
    max_attempts: 5,
    lease_token: "lease-1",
    lease_expires_at: "2026-06-10T00:01:00Z",
    chain_id: "chain-1",
    workflow_execution_id: "exec-1",
    created_at: "2026-06-10T00:00:00Z",
    updated_at: "2026-06-10T00:00:30Z",
    ...overrides,
  };
}

describe("parseWorkerTask", () => {
  it("parses all fields", () => {
    const task = parseWorkerTask(wireTask({ result: { ok: true }, error: "e" }));
    assert.equal(task.taskId, "task-1");
    assert.equal(task.queue, "billing");
    assert.equal(task.actionType, "charge");
    assert.deepEqual(task.payload, { amount: 42 });
    assert.equal(task.status, "leased");
    assert.equal(task.attempt, 1);
    assert.equal(task.maxAttempts, 5);
    assert.equal(task.leaseToken, "lease-1");
    assert.equal(task.leaseExpiresAt, "2026-06-10T00:01:00Z");
    assert.deepEqual(task.result, { ok: true });
    assert.equal(task.error, "e");
    assert.equal(task.chainId, "chain-1");
    assert.equal(task.workflowExecutionId, "exec-1");
    assert.equal(task.createdAt, "2026-06-10T00:00:00Z");
    assert.equal(task.updatedAt, "2026-06-10T00:00:30Z");
  });

  it("optional fields drop cleanly", () => {
    const task = parseWorkerTask({
      task_id: "task-2",
      queue: "q",
      action_type: "a",
      payload: null,
      status: "pending",
      attempt: 0,
      max_attempts: 1,
      created_at: "2026-06-10T00:00:00Z",
      updated_at: "2026-06-10T00:00:00Z",
    });
    assert.equal(task.leaseToken, undefined);
    assert.equal(task.leaseExpiresAt, undefined);
    assert.equal(task.result, undefined);
    assert.equal(task.error, undefined);
    assert.equal(task.chainId, undefined);
    assert.equal(task.workflowExecutionId, undefined);
  });

  it("parseTaskListResponse unwraps the tasks envelope", () => {
    const tasks = parseTaskListResponse({ tasks: [wireTask(), wireTask({ task_id: "task-2" })] });
    assert.equal(tasks.length, 2);
    assert.equal(tasks[1].taskId, "task-2");

    assert.deepEqual(parseTaskListResponse({}), []);
  });
});

// ---------------------------------------------------------------------
// Client method URL + body behaviour via a captured `fetch`
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

function sentBody(call: CapturedFetch): Record<string, unknown> {
  return JSON.parse(call.init?.body as string) as Record<string, unknown>;
}

describe("queue client URLs and bodies", () => {
  it("pollTasks posts the poll body and unwraps tasks", async () => {
    const { calls, restore } = captureFetch(200, { tasks: [wireTask()] });
    try {
      const c = new ActeonClient("http://x");
      const tasks = await c.pollTasks("billing", "ns", "t1", {
        maxTasks: 2,
        leaseSeconds: 30,
        workerId: "w-1",
      });
      expect(calls[0].url).toEqual("http://x/v1/queues/billing/poll");
      expect(calls[0].init?.method).toEqual("POST");
      expect(sentBody(calls[0])).toEqual({
        namespace: "ns",
        tenant: "t1",
        max_tasks: 2,
        lease_seconds: 30,
        worker_id: "w-1",
      });
      expect(tasks.length).toEqual(1);
      expect(tasks[0].taskId).toEqual("task-1");
    } finally {
      restore();
    }
  });

  it("enqueueTask hits the queue tasks URL and parses a 201", async () => {
    const { calls, restore } = captureFetch(201, wireTask({ status: "pending" }));
    try {
      const c = new ActeonClient("http://x");
      const task = await c.enqueueTask("billing", "ns", "t1", "charge", { amount: 42 }, { maxAttempts: 3 });
      expect(calls[0].url).toEqual("http://x/v1/queues/billing/tasks");
      expect(sentBody(calls[0]).max_attempts).toEqual(3);
      expect(task.status).toEqual("pending");
    } finally {
      restore();
    }
  });

  it("heartbeatTask / completeTask / failTask hit the task verb URLs", async () => {
    const { calls, restore } = captureFetch(200, wireTask());
    try {
      const c = new ActeonClient("http://x");
      await c.heartbeatTask("task-1", "ns", "t1", "lease-1", 45);
      await c.completeTask("task-1", "ns", "t1", "lease-1", { ok: true });
      await c.failTask("task-1", "ns", "t1", "lease-1", "boom");
      expect(calls[0].url).toEqual("http://x/v1/queues/tasks/task-1/heartbeat");
      expect(sentBody(calls[0]).extend_seconds).toEqual(45);
      expect(calls[1].url).toEqual("http://x/v1/queues/tasks/task-1/complete");
      expect(sentBody(calls[1]).result).toEqual({ ok: true });
      expect(calls[2].url).toEqual("http://x/v1/queues/tasks/task-1/fail");
      // retryable defaults to true.
      expect(sentBody(calls[2]).retryable).toEqual(true);
    } finally {
      restore();
    }
  });

  it("getTask returns null on 404", async () => {
    const { calls, restore } = captureFetch(404, { error: "task not found" });
    try {
      const c = new ActeonClient("http://x");
      const task = await c.getTask("task-9", "ns", "t1");
      expect(calls[0].url).toEqual(
        "http://x/v1/queues/tasks/task-9?namespace=ns&tenant=t1",
      );
      expect(task).toBeNull();
    } finally {
      restore();
    }
  });

  it("listTasks carries namespace, tenant, and status filters", async () => {
    const { calls, restore } = captureFetch(200, { tasks: [] });
    try {
      const c = new ActeonClient("http://x");
      const tasks = await c.listTasks("billing", "ns", "t1", "failed");
      expect(calls[0].url).toEqual(
        "http://x/v1/queues/billing/tasks?namespace=ns&tenant=t1&status=failed",
      );
      expect(tasks).toEqual([]);
    } finally {
      restore();
    }
  });

  it("surfaces the server error envelope as an HttpError", async () => {
    const { restore } = captureFetch(409, { error: "lease token mismatch" });
    try {
      const c = new ActeonClient("http://x");
      await expect(
        c.completeTask("task-1", "ns", "t1", "stale", null),
      ).rejects.toThrowError(HttpError);
      await expect(
        c.completeTask("task-1", "ns", "t1", "stale", null),
      ).rejects.toThrow(/lease token mismatch/);
    } finally {
      restore();
    }
  });
});
