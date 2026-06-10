/**
 * Workflow authoring surface tests.
 *
 * Live HTTP tests would need a running Acteon instance with the
 * workflow engine enabled; these exercise serde (body builders +
 * response parsers), the `ActeonClient` workflow methods via a
 * captured `fetch` stub, and the checkpoint-replay semantics of
 * `WorkflowContext` + `runWorkflowTask`. The contract under test:
 * checkpoint keys are stable across re-runs, replays skip executed
 * operations, and suspension points produce the right directives.
 */

import { describe, it, expect, assert } from "vitest";
import {
  CHILD_RESULT_SIGNAL_PREFIX,
  WORKFLOW_TASK_ACTION_TYPE,
  WorkflowContext,
  WorkflowSuspend,
  parseExecutionHistory,
  parseRecordCheckpointResponse,
  parseWorkflowCheckpoint,
  parseWorkflowExecution,
  parseWorkflowExecutionListResponse,
  runWorkflowTask,
  startWorkflowBody,
  type WorkflowCheckpoint,
  type WorkflowTaskPayload,
} from "./workflows.js";
import { ActeonClient } from "./client.js";

// ---------------------------------------------------------------------
// Constants + body builders + parsers
// ---------------------------------------------------------------------

describe("workflow constants", () => {
  it("match the server wire protocol", () => {
    expect(WORKFLOW_TASK_ACTION_TYPE).toEqual("__workflow__");
    expect(CHILD_RESULT_SIGNAL_PREFIX).toEqual("__child:");
  });
});

describe("startWorkflowBody", () => {
  it("minimal omits search_attributes", () => {
    const body = startWorkflowBody("ns", "t1", "onboarding", "q", { u: 1 });
    assert.deepEqual(body, {
      namespace: "ns",
      tenant: "t1",
      workflow: "onboarding",
      queue: "q",
      input: { u: 1 },
    });
  });

  it("snake-cases search attributes", () => {
    const body = startWorkflowBody("ns", "t1", "wf", "q", null, {
      searchAttributes: { region: "eu" },
    });
    assert.deepEqual(body.search_attributes, { region: "eu" });
  });
});

function wireExecution(
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    execution_id: "exec-1",
    workflow: "onboarding",
    queue: "q",
    status: "running",
    input: { u: 1 },
    checkpoints: [
      { seq: 1, name: "step:a#0", data: { v: 1 }, recorded_at: "2026-06-10T00:00:00Z" },
    ],
    search_attributes: { region: "eu" },
    created_at: "2026-06-10T00:00:00Z",
    updated_at: "2026-06-10T00:00:30Z",
    ...overrides,
  };
}

describe("workflow parsers", () => {
  it("parseWorkflowExecution parses all fields", () => {
    const exec = parseWorkflowExecution(
      wireExecution({
        result: { done: true },
        error: "e",
        awaiting: { kind: "timer" },
        parent_id: "exec-0",
        children: ["exec-2"],
      }),
    );
    assert.equal(exec.executionId, "exec-1");
    assert.equal(exec.workflow, "onboarding");
    assert.equal(exec.queue, "q");
    assert.equal(exec.status, "running");
    assert.deepEqual(exec.input, { u: 1 });
    assert.deepEqual(exec.result, { done: true });
    assert.equal(exec.error, "e");
    assert.equal(exec.checkpoints.length, 1);
    assert.equal(exec.checkpoints[0].name, "step:a#0");
    assert.deepEqual(exec.awaiting, { kind: "timer" });
    assert.equal(exec.parentId, "exec-0");
    assert.deepEqual(exec.children, ["exec-2"]);
    assert.deepEqual(exec.searchAttributes, { region: "eu" });
  });

  it("optional fields drop cleanly", () => {
    const exec = parseWorkflowExecution(
      wireExecution({ checkpoints: undefined, children: undefined, search_attributes: undefined }),
    );
    assert.deepEqual(exec.checkpoints, []);
    assert.deepEqual(exec.children, []);
    assert.deepEqual(exec.searchAttributes, {});
    assert.equal(exec.parentId, undefined);
    assert.equal(exec.awaiting, undefined);
  });

  it("parseWorkflowCheckpoint round-trips the wire shape", () => {
    const checkpoint = parseWorkflowCheckpoint({
      seq: 2,
      name: "sleep#0",
      data: { fired: true },
      recorded_at: "2026-06-10T00:00:00Z",
    });
    assert.deepEqual(checkpoint, {
      seq: 2,
      name: "sleep#0",
      data: { fired: true },
      recordedAt: "2026-06-10T00:00:00Z",
    });
  });

  it("parseWorkflowExecutionListResponse unwraps the envelope", () => {
    const executions = parseWorkflowExecutionListResponse({
      executions: [wireExecution(), wireExecution({ execution_id: "exec-2" })],
    });
    assert.equal(executions.length, 2);
    assert.equal(executions[1].executionId, "exec-2");
  });

  it("parseRecordCheckpointResponse + parseExecutionHistory", () => {
    const recorded = parseRecordCheckpointResponse({
      name: "step:a#0",
      seq: 1,
      data: { v: 1 },
    });
    assert.deepEqual(recorded, { name: "step:a#0", seq: 1, data: { v: 1 } });

    const history = parseExecutionHistory({
      execution_id: "exec-1",
      events: [{ event_type: "execution_started" }],
    });
    assert.equal(history.executionId, "exec-1");
    assert.equal(history.events.length, 1);
  });
});

// ---------------------------------------------------------------------
// Routed `fetch` stub: records calls and answers per-URL
// ---------------------------------------------------------------------

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

/** Route that answers checkpoint POSTs by echoing the recorded data
 *  (the server's idempotent-record behaviour for fresh names) and
 *  children POSTs with a fixed child id. */
function workflowRoute(call: RecordedCall): RouteResult {
  if (call.url.endsWith("/checkpoints")) {
    return { body: { name: call.body.name, seq: 1, data: call.body.data } };
  }
  if (call.url.endsWith("/children")) {
    return { status: 201, body: { child_execution_id: "child-1" } };
  }
  return undefined;
}

function checkpoint(name: string, data: unknown): WorkflowCheckpoint {
  return { seq: 1, name, data, recordedAt: "2026-06-10T00:00:00Z" };
}

function taskPayload(
  checkpoints: { name: string; data: unknown }[] = [],
): WorkflowTaskPayload {
  return {
    execution_id: "exec-1",
    workflow: "onboarding",
    input: { u: 1 },
    checkpoints: checkpoints.map((c, i) => ({
      seq: i + 1,
      name: c.name,
      data: c.data,
      recorded_at: "2026-06-10T00:00:00Z",
    })),
  };
}

// ---------------------------------------------------------------------
// WorkflowContext + runWorkflowTask
// ---------------------------------------------------------------------

describe("WorkflowContext", () => {
  it("steps execute once, record checkpoints, and complete", async () => {
    const { calls, restore } = routeFetch(workflowRoute);
    try {
      const c = new ActeonClient("http://x");
      const ran: string[] = [];
      const directive = await runWorkflowTask(c, "ns", "t1", async (ctx) => {
        const a = await ctx.step("a", () => {
          ran.push("a");
          return { v: 1 };
        });
        const b = await ctx.step("b", () => {
          ran.push("b");
          return { v: 2 };
        });
        return { a, b };
      }, taskPayload());

      expect(ran).toEqual(["a", "b"]);
      expect(directive).toEqual({
        directive: "complete",
        result: { a: { v: 1 }, b: { v: 2 } },
      });
      const recorded = calls.filter((call) => call.url.endsWith("/checkpoints"));
      expect(recorded.length).toEqual(2);
      expect(recorded[0].url).toEqual(
        "http://x/v1/workflows/executions/exec-1/checkpoints",
      );
      expect(recorded[0].body).toEqual({
        namespace: "ns",
        tenant: "t1",
        name: "step:a#0",
        data: { v: 1 },
      });
      expect(recorded[1].body.name).toEqual("step:b#0");
    } finally {
      restore();
    }
  });

  it("replay skips executed steps and only records new ones", async () => {
    const { calls, restore } = routeFetch(workflowRoute);
    try {
      const c = new ActeonClient("http://x");
      const ran: string[] = [];
      const directive = await runWorkflowTask(c, "ns", "t1", async (ctx) => {
        const a = await ctx.step("a", () => {
          ran.push("a");
          return { v: 999 };
        });
        const b = await ctx.step("b", () => {
          ran.push("b");
          return { v: 2 };
        });
        return { a, b };
      }, taskPayload([{ name: "step:a#0", data: { v: 1 } }]));

      // Step a replays the recorded data without running its fn.
      expect(ran).toEqual(["b"]);
      expect(directive).toEqual({
        directive: "complete",
        result: { a: { v: 1 }, b: { v: 2 } },
      });
      const recorded = calls.filter((call) => call.url.endsWith("/checkpoints"));
      expect(recorded.length).toEqual(1);
      expect(recorded[0].body.name).toEqual("step:b#0");
    } finally {
      restore();
    }
  });

  it("sleep suspends with a sleep directive and replays through", async () => {
    const { restore } = routeFetch(workflowRoute);
    try {
      const c = new ActeonClient("http://x");
      const fn = async (ctx: WorkflowContext) => {
        await ctx.step("a", () => ({ v: 1 }));
        await ctx.sleep(3600);
        return "done";
      };

      // First continuation: suspends at the timer.
      const first = await runWorkflowTask(c, "ns", "t1", fn, taskPayload());
      expect(first).toEqual({
        directive: "sleep",
        checkpoint: "sleep#0",
        seconds: 3600,
      });

      // Second continuation: timer checkpoint recorded — runs to completion.
      const second = await runWorkflowTask(
        c,
        "ns",
        "t1",
        fn,
        taskPayload([
          { name: "step:a#0", data: { v: 1 } },
          { name: "sleep#0", data: { fired: true } },
        ]),
      );
      expect(second).toEqual({ directive: "complete", result: "done" });
    } finally {
      restore();
    }
  });

  it("waitForSignal suspends with an await_signal directive", async () => {
    const { restore } = routeFetch(workflowRoute);
    try {
      const c = new ActeonClient("http://x");
      const directive = await runWorkflowTask(c, "ns", "t1", async (ctx) => {
        return ctx.waitForSignal("approved", 600);
      }, taskPayload());
      expect(directive).toEqual({
        directive: "await_signal",
        checkpoint: "signal:approved#0",
        name: "approved",
        timeout_seconds: 600,
      });

      // Without a timeout the field is omitted entirely.
      const noTimeout = await runWorkflowTask(c, "ns", "t1", async (ctx) => {
        return ctx.waitForSignal("approved");
      }, taskPayload());
      expect(noTimeout).toEqual({
        directive: "await_signal",
        checkpoint: "signal:approved#0",
        name: "approved",
      });
      assert.equal("timeout_seconds" in noTimeout, false);
    } finally {
      restore();
    }
  });

  it("waitForSignal replay returns the payload, or null on timeout", async () => {
    const { restore } = routeFetch(workflowRoute);
    try {
      const c = new ActeonClient("http://x");
      const fn = async (ctx: WorkflowContext) => {
        const payload = await ctx.waitForSignal("approved", 600);
        return { payload };
      };

      const delivered = await runWorkflowTask(
        c,
        "ns",
        "t1",
        fn,
        taskPayload([{ name: "signal:approved#0", data: { by: "renzo" } }]),
      );
      expect(delivered).toEqual({
        directive: "complete",
        result: { payload: { by: "renzo" } },
      });

      const timedOut = await runWorkflowTask(
        c,
        "ns",
        "t1",
        fn,
        taskPayload([{ name: "signal:approved#0", data: { timed_out: true } }]),
      );
      expect(timedOut).toEqual({
        directive: "complete",
        result: { payload: null },
      });
    } finally {
      restore();
    }
  });

  it("checkpoint keys are stable across re-runs and count per name", async () => {
    const { restore } = routeFetch(workflowRoute);
    try {
      const c = new ActeonClient("http://x");
      const keysSeen: string[][] = [];
      const fn = async (ctx: WorkflowContext) => {
        const keys: string[] = [];
        await ctx.step("x", () => "first");
        keys.push("step:x#0");
        await ctx.step("x", () => "second");
        keys.push("step:x#1");
        await ctx.step("y", () => "third");
        keys.push("step:y#0");
        keysSeen.push(keys);
        return "done";
      };

      // First run records all three; the re-run replays them from the
      // recorded checkpoints under the exact same keys.
      const recorded = [
        { name: "step:x#0", data: "first" },
        { name: "step:x#1", data: "second" },
        { name: "step:y#0", data: "third" },
      ];
      const first = await runWorkflowTask(c, "ns", "t1", fn, taskPayload());
      const rerun = await runWorkflowTask(c, "ns", "t1", fn, taskPayload(recorded));
      expect(first).toEqual(rerun);
      expect(keysSeen[0]).toEqual(keysSeen[1]);
    } finally {
      restore();
    }
  });

  it("startChild posts the children endpoint and replays the child id", async () => {
    const { calls, restore } = routeFetch(workflowRoute);
    try {
      const c = new ActeonClient("http://x");
      const fn = async (ctx: WorkflowContext) => {
        const childId = await ctx.startChild("sub", { n: 1 }, {
          queue: "sub-q",
          parentClosePolicy: "cancel",
        });
        return { childId };
      };

      const fresh = await runWorkflowTask(c, "ns", "t1", fn, taskPayload());
      expect(fresh).toEqual({
        directive: "complete",
        result: { childId: "child-1" },
      });
      const started = calls.filter((call) => call.url.endsWith("/children"));
      expect(started.length).toEqual(1);
      expect(started[0].url).toEqual(
        "http://x/v1/workflows/executions/exec-1/children",
      );
      expect(started[0].body).toEqual({
        namespace: "ns",
        tenant: "t1",
        checkpoint: "child:sub#0",
        workflow: "sub",
        input: { n: 1 },
        queue: "sub-q",
        parent_close_policy: "cancel",
      });

      // Replay: the recorded checkpoint resolves without another POST.
      const replayed = await runWorkflowTask(
        c,
        "ns",
        "t1",
        fn,
        taskPayload([{ name: "child:sub#0", data: { child_id: "child-7" } }]),
      );
      expect(replayed).toEqual({
        directive: "complete",
        result: { childId: "child-7" },
      });
      expect(calls.filter((call) => call.url.endsWith("/children")).length).toEqual(1);
    } finally {
      restore();
    }
  });

  it("waitForChild awaits the __child: signal", async () => {
    const { restore } = routeFetch(workflowRoute);
    try {
      const c = new ActeonClient("http://x");
      const directive = await runWorkflowTask(c, "ns", "t1", async (ctx) => {
        return ctx.waitForChild("child-1", 60);
      }, taskPayload([{ name: "child:sub#0", data: { child_id: "child-1" } }]));
      expect(directive).toEqual({
        directive: "await_signal",
        checkpoint: "signal:__child:child-1#0",
        name: "__child:child-1",
        timeout_seconds: 60,
      });
    } finally {
      restore();
    }
  });

  it("a workflow throw becomes a fail directive", async () => {
    const { restore } = routeFetch(workflowRoute);
    try {
      const c = new ActeonClient("http://x");
      const directive = await runWorkflowTask(c, "ns", "t1", async () => {
        throw new Error("payment rejected");
      }, taskPayload());
      expect(directive).toEqual({
        directive: "fail",
        error: "payment rejected",
      });
    } finally {
      restore();
    }
  });

  it("exposes executionId and input; WorkflowSuspend carries its directive", async () => {
    const ctx = new WorkflowContext(
      new ActeonClient("http://x"),
      "ns",
      "t1",
      "exec-1",
      { u: 1 },
      [checkpoint("sleep#0", { fired: true })],
    );
    expect(ctx.executionId).toEqual("exec-1");
    expect(ctx.input).toEqual({ u: 1 });
    // Recorded timer replays straight through.
    await ctx.sleep(10);
    // The second occurrence suspends under the next key.
    try {
      await ctx.sleep(10);
      assert.fail("expected WorkflowSuspend");
    } catch (e) {
      assert.instanceOf(e, WorkflowSuspend);
      expect((e as WorkflowSuspend).directive).toEqual({
        directive: "sleep",
        checkpoint: "sleep#1",
        seconds: 10,
      });
    }
  });
});

// ---------------------------------------------------------------------
// Client method URL + body behaviour
// ---------------------------------------------------------------------

describe("workflow client URLs and bodies", () => {
  it("startWorkflow posts /v1/workflows/start and parses a 201", async () => {
    const { calls, restore } = routeFetch(() => ({
      status: 201,
      body: wireExecution(),
    }));
    try {
      const c = new ActeonClient("http://x");
      const exec = await c.startWorkflow("ns", "t1", "onboarding", "q", { u: 1 }, {
        searchAttributes: { region: "eu" },
      });
      expect(calls[0].url).toEqual("http://x/v1/workflows/start");
      expect(calls[0].body.search_attributes).toEqual({ region: "eu" });
      expect(exec.executionId).toEqual("exec-1");
    } finally {
      restore();
    }
  });

  it("listWorkflowExecutions carries the filters", async () => {
    const { calls, restore } = routeFetch(() => ({ body: { executions: [] } }));
    try {
      const c = new ActeonClient("http://x");
      const executions = await c.listWorkflowExecutions("ns", "t1", {
        workflow: "onboarding",
        status: "running",
        limit: 10,
      });
      expect(calls[0].url).toEqual(
        "http://x/v1/workflows/executions?namespace=ns&tenant=t1&workflow=onboarding&status=running&limit=10",
      );
      expect(executions).toEqual([]);
    } finally {
      restore();
    }
  });

  it("getWorkflowExecution returns null on 404", async () => {
    const { calls, restore } = routeFetch(() => ({
      status: 404,
      body: { error: "not found" },
    }));
    try {
      const c = new ActeonClient("http://x");
      const exec = await c.getWorkflowExecution("exec-9", "ns", "t1");
      expect(calls[0].url).toEqual(
        "http://x/v1/workflows/executions/exec-9?namespace=ns&tenant=t1",
      );
      expect(exec).toBeNull();
    } finally {
      restore();
    }
  });

  it("signalWorkflow and cancelWorkflow hit the verb URLs", async () => {
    const { calls, restore } = routeFetch(() => undefined);
    try {
      const c = new ActeonClient("http://x");
      await c.signalWorkflow("exec-1", "approved", "ns", "t1", { by: "renzo" });
      await c.cancelWorkflow("exec-1", "ns", "t1", "superseded");
      expect(calls[0].url).toEqual(
        "http://x/v1/workflows/executions/exec-1/signal/approved",
      );
      expect(calls[0].body).toEqual({
        namespace: "ns",
        tenant: "t1",
        payload: { by: "renzo" },
      });
      expect(calls[1].url).toEqual(
        "http://x/v1/workflows/executions/exec-1/cancel",
      );
      expect(calls[1].body.reason).toEqual("superseded");
    } finally {
      restore();
    }
  });

  it("getExecutionHistory hits /v1/executions/{id}/history", async () => {
    const { calls, restore } = routeFetch(() => ({
      body: { execution_id: "exec-1", events: [] },
    }));
    try {
      const c = new ActeonClient("http://x");
      const history = await c.getExecutionHistory("exec-1", "ns", "t1");
      expect(calls[0].url).toEqual(
        "http://x/v1/executions/exec-1/history?namespace=ns&tenant=t1",
      );
      expect(history.executionId).toEqual("exec-1");
    } finally {
      restore();
    }
  });
});
