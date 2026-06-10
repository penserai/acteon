/**
 * Worker poll-loop tests.
 *
 * Live HTTP tests would need a running Acteon instance with task
 * queues enabled; these drive the `Worker` against a routed `fetch`
 * stub. The contract under test: poll → handle → settle round-trips
 * the lease protocol, handler outcomes map to complete/fail with the
 * right retryability, workflow continuation tasks route to the
 * registered workflow function, and the lease auto-heartbeats while
 * a handler runs.
 */

import { describe, it, expect, assert } from "vitest";
import { Worker } from "./worker.js";
import { ActeonClient } from "./client.js";
import { NonRetryableError } from "./errors.js";

interface RecordedCall {
  method: string;
  url: string;
  body: Record<string, unknown>;
}

type RouteResult = { status?: number; body?: unknown } | undefined;

/** Replace `globalThis.fetch` with a router stub. `route` receives
 *  each call and returns the response to send (defaults to 200 `{}`).
 *  Returns the recorded-calls array plus a restore function. */
function routeFetch(route: (call: RecordedCall) => RouteResult): {
  calls: RecordedCall[];
  restore: () => void;
} {
  const calls: RecordedCall[] = [];
  const original = globalThis.fetch;
  globalThis.fetch = (async (
    input: string | URL | Request,
    init?: RequestInit,
  ): Promise<Response> => {
    const call: RecordedCall = {
      method: init?.method ?? "GET",
      url: String(input),
      body: init?.body
        ? (JSON.parse(init.body as string) as Record<string, unknown>)
        : {},
    };
    calls.push(call);
    const result = route(call) ?? {};
    return new Response(JSON.stringify(result.body ?? {}), {
      status: result.status ?? 200,
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
    created_at: "2026-06-10T00:00:00Z",
    updated_at: "2026-06-10T00:00:00Z",
    ...overrides,
  };
}

/** Route serving the given tasks on the first poll and an empty
 *  queue afterwards; all settlement calls echo a leased task. */
function queueRoute(tasks: Record<string, unknown>[]): (
  call: RecordedCall,
) => RouteResult {
  let polled = false;
  return (call) => {
    if (call.url.endsWith("/poll")) {
      const body = polled ? { tasks: [] } : { tasks };
      polled = true;
      return { body };
    }
    return { body: wireTask() };
  };
}

function makeWorker(overrides: Partial<ConstructorParameters<typeof Worker>[0]> = {}): Worker {
  return new Worker({
    client: new ActeonClient("http://x"),
    namespace: "ns",
    tenant: "t1",
    queue: "billing",
    ...overrides,
  });
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

describe("Worker task handling", () => {
  it("polls, runs the handler, and completes with its result", async () => {
    const { calls, restore } = routeFetch(queueRoute([wireTask()]));
    try {
      const worker = makeWorker({ workerId: "w-1", leaseSeconds: 30 });
      const seen: unknown[] = [];
      worker.register("charge", (payload, task) => {
        seen.push(payload, task.taskId);
        return { charged: true };
      });

      const handled = await worker.runOnce();
      expect(handled).toEqual(1);
      expect(seen).toEqual([{ amount: 42 }, "task-1"]);

      expect(calls[0].url).toEqual("http://x/v1/queues/billing/poll");
      expect(calls[0].body).toEqual({
        namespace: "ns",
        tenant: "t1",
        max_tasks: 1,
        lease_seconds: 30,
        worker_id: "w-1",
      });
      expect(calls[1].url).toEqual("http://x/v1/queues/tasks/task-1/complete");
      expect(calls[1].body).toEqual({
        namespace: "ns",
        tenant: "t1",
        lease_token: "lease-1",
        result: { charged: true },
      });
    } finally {
      restore();
    }
  });

  it("a handler throw fails the task as retryable", async () => {
    const { calls, restore } = routeFetch(queueRoute([wireTask()]));
    try {
      const worker = makeWorker();
      worker.register("charge", () => {
        throw new Error("card declined");
      });
      await worker.runOnce();
      expect(calls[1].url).toEqual("http://x/v1/queues/tasks/task-1/fail");
      expect(calls[1].body).toEqual({
        namespace: "ns",
        tenant: "t1",
        lease_token: "lease-1",
        error: "card declined",
        retryable: true,
      });
    } finally {
      restore();
    }
  });

  it("NonRetryableError fails the task with retryable: false", async () => {
    const { calls, restore } = routeFetch(queueRoute([wireTask()]));
    try {
      const worker = makeWorker();
      worker.register("charge", () => {
        throw new NonRetryableError("unknown account");
      });
      await worker.runOnce();
      expect(calls[1].url).toEqual("http://x/v1/queues/tasks/task-1/fail");
      expect(calls[1].body.retryable).toEqual(false);
      expect(calls[1].body.error).toEqual("unknown account");
    } finally {
      restore();
    }
  });

  it("an unregistered action type fails the task as retryable", async () => {
    const { calls, restore } = routeFetch(queueRoute([wireTask({ action_type: "refund" })]));
    try {
      const worker = makeWorker();
      worker.register("charge", () => ({}));
      await worker.runOnce();
      expect(calls[1].url).toEqual("http://x/v1/queues/tasks/task-1/fail");
      expect(calls[1].body.retryable).toEqual(true);
      expect(calls[1].body.error).toEqual(
        "no handler registered for action type: refund",
      );
    } finally {
      restore();
    }
  });

  it("handles up to maxConcurrent tasks per poll", async () => {
    const { calls, restore } = routeFetch(
      queueRoute([
        wireTask(),
        wireTask({ task_id: "task-2", lease_token: "lease-2" }),
      ]),
    );
    try {
      const worker = makeWorker({ maxConcurrent: 2 });
      worker.register("charge", () => "ok");
      const handled = await worker.runOnce();
      expect(handled).toEqual(2);
      expect(calls[0].body.max_tasks).toEqual(2);
      const completes = calls.filter((call) => call.url.endsWith("/complete"));
      expect(completes.length).toEqual(2);
      expect(completes.map((call) => call.body.lease_token).sort()).toEqual([
        "lease-1",
        "lease-2",
      ]);
    } finally {
      restore();
    }
  });

  it("auto-heartbeats while a handler runs and adopts rotated tokens", async () => {
    const { calls, restore } = routeFetch((call) => {
      if (call.url.endsWith("/poll")) {
        return { body: { tasks: [wireTask()] } };
      }
      if (call.url.endsWith("/heartbeat")) {
        return { body: wireTask({ lease_token: "lease-2" }) };
      }
      return { body: wireTask() };
    });
    try {
      // leaseSeconds 0.04 → heartbeat every 20 ms; the handler runs
      // for ~80 ms, so at least one heartbeat must fire.
      const worker = makeWorker({ leaseSeconds: 0.04 });
      worker.register("charge", async () => {
        await sleep(80);
        return "ok";
      });
      await worker.runOnce();

      const heartbeats = calls.filter((call) => call.url.endsWith("/heartbeat"));
      assert.isAtLeast(heartbeats.length, 1);
      expect(heartbeats[0].url).toEqual(
        "http://x/v1/queues/tasks/task-1/heartbeat",
      );
      expect(heartbeats[0].body.lease_token).toEqual("lease-1");

      // The settle call uses the rotated token from the heartbeat.
      const complete = calls.find((call) => call.url.endsWith("/complete"));
      expect(complete?.body.lease_token).toEqual("lease-2");
    } finally {
      restore();
    }
  });
});

describe("Worker workflow routing", () => {
  it("routes __workflow__ tasks to the registered workflow", async () => {
    const workflowTask = wireTask({
      action_type: "__workflow__",
      payload: {
        execution_id: "exec-1",
        workflow: "onboarding",
        input: { u: 1 },
        checkpoints: [],
      },
    });
    const { calls, restore } = routeFetch((call) => {
      if (call.url.endsWith("/poll")) {
        return { body: { tasks: [workflowTask] } };
      }
      if (call.url.endsWith("/checkpoints")) {
        return { body: { name: call.body.name, seq: 1, data: call.body.data } };
      }
      return { body: wireTask() };
    });
    try {
      const worker = makeWorker();
      worker.registerWorkflow("onboarding", async (ctx, input) => {
        const user = await ctx.step("create-user", () => ({ id: 7, input }));
        await ctx.sleep(3600);
        return user;
      });
      await worker.runOnce();

      const recorded = calls.find((call) => call.url.endsWith("/checkpoints"));
      expect(recorded?.body.name).toEqual("step:create-user#0");

      // The continuation settles via complete with the suspension
      // directive as the result.
      const complete = calls.find((call) => call.url.endsWith("/complete"));
      expect(complete?.url).toEqual("http://x/v1/queues/tasks/task-1/complete");
      expect(complete?.body.result).toEqual({
        directive: "sleep",
        checkpoint: "sleep#0",
        seconds: 3600,
      });
    } finally {
      restore();
    }
  });

  it("an unregistered workflow fails the task as retryable", async () => {
    const workflowTask = wireTask({
      action_type: "__workflow__",
      payload: {
        execution_id: "exec-1",
        workflow: "unknown-wf",
        input: null,
        checkpoints: [],
      },
    });
    const { calls, restore } = routeFetch(queueRoute([workflowTask]));
    try {
      const worker = makeWorker();
      await worker.runOnce();
      expect(calls[1].url).toEqual("http://x/v1/queues/tasks/task-1/fail");
      expect(calls[1].body.error).toEqual("no workflow registered: unknown-wf");
      expect(calls[1].body.retryable).toEqual(true);
    } finally {
      restore();
    }
  });

  it("a workflow throw settles the task with a fail directive", async () => {
    const workflowTask = wireTask({
      action_type: "__workflow__",
      payload: {
        execution_id: "exec-1",
        workflow: "onboarding",
        input: null,
        checkpoints: [],
      },
    });
    const { calls, restore } = routeFetch(queueRoute([workflowTask]));
    try {
      const worker = makeWorker();
      worker.registerWorkflow("onboarding", () => {
        throw new Error("bad input");
      });
      await worker.runOnce();
      const complete = calls.find((call) => call.url.endsWith("/complete"));
      expect(complete?.body.result).toEqual({
        directive: "fail",
        error: "bad input",
      });
    } finally {
      restore();
    }
  });
});

describe("Worker run loop", () => {
  it("run() polls until stop(), settling tasks along the way", async () => {
    const { calls, restore } = routeFetch(queueRoute([wireTask()]));
    try {
      const worker = makeWorker({ pollIntervalMs: 5 });
      worker.register("charge", () => {
        worker.stop();
        return "ok";
      });
      await worker.run();
      const completes = calls.filter((call) => call.url.endsWith("/complete"));
      expect(completes.length).toEqual(1);
    } finally {
      restore();
    }
  });

  it("stop() interrupts the idle poll backoff promptly", async () => {
    const { restore } = routeFetch(() => ({ body: { tasks: [] } }));
    try {
      const worker = makeWorker({ pollIntervalMs: 60_000 });
      const running = worker.run();
      await sleep(10);
      worker.stop();
      // Resolves without waiting out the 60 s interval.
      await running;
    } finally {
      restore();
    }
  });

  it("run() rejects when already running", async () => {
    const { restore } = routeFetch(() => ({ body: { tasks: [] } }));
    try {
      const worker = makeWorker({ pollIntervalMs: 5 });
      const running = worker.run();
      await expect(worker.run()).rejects.toThrow(/already running/);
      worker.stop();
      await running;
    } finally {
      restore();
    }
  });

  it("run() survives transport errors and keeps polling", async () => {
    let failures = 0;
    let completed = false;
    const { restore } = routeFetch((call) => {
      if (call.url.endsWith("/poll")) {
        if (failures < 1) {
          failures += 1;
          return { status: 500, body: { error: "backend unavailable" } };
        }
        return { body: completed ? { tasks: [] } : { tasks: [wireTask()] } };
      }
      completed = true;
      return { body: wireTask() };
    });
    try {
      const worker = makeWorker({ pollIntervalMs: 5 });
      worker.register("charge", () => {
        worker.stop();
        return "ok";
      });
      await worker.run();
      expect(failures).toEqual(1);
      expect(completed).toEqual(true);
    } finally {
      restore();
    }
  });
});
