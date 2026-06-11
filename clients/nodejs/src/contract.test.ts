/**
 * Node.js side of the cross-SDK workflow contract.
 *
 * Drives the public SDK surface (`WorkflowContext` + `runWorkflowTask`)
 * against the shared fixtures in
 * `clients/contract-fixtures/workflow-contract.json` — the same file
 * consumed by the Python SDK tests and by the Rust server tests — so
 * checkpoint-key derivation, directive wire shapes, and the
 * integer-seconds coercions stay identical across languages. A workflow
 * execution can migrate between Python and Node workers mid-flight;
 * these keys and shapes are the entire compatibility surface.
 */

import { readFileSync } from "node:fs";
import { describe, it, expect } from "vitest";
import type { ActeonClient } from "./client.js";
import {
  CHILD_RESULT_SIGNAL_PREFIX,
  WORKFLOW_TASK_ACTION_TYPE,
  WorkflowContext,
  WorkflowSuspend,
  runWorkflowTask,
  type WorkflowCheckpoint,
  type WorkflowDirective,
} from "./workflows.js";

interface Fixtures {
  constants: {
    workflow_task_action_type: string;
    child_result_signal_prefix: string;
    timed_out_marker: Record<string, unknown>;
  };
  directives: { name: string; json: Record<string, unknown> }[];
  checkpoint_key_scenarios: {
    name: string;
    ops: Record<string, unknown>[];
    expected_keys: string[];
  }[];
  sleep_coercions: { input_seconds: number; expected_seconds: number }[];
  signal_timeout_coercions: {
    input_seconds: number;
    expected_seconds: number;
  }[];
}

const FIXTURES: Fixtures = JSON.parse(
  readFileSync(
    new URL("../../contract-fixtures/workflow-contract.json", import.meta.url),
    "utf8",
  ),
) as Fixtures;

function directiveFixture(name: string): Record<string, unknown> {
  const found = FIXTURES.directives.find((d) => d.name === name);
  if (!found) throw new Error(`no directive fixture: ${name}`);
  return found.json;
}

function toCheckpoints(record: Map<string, unknown>): WorkflowCheckpoint[] {
  return [...record.entries()].map(([name, data], i) => ({
    seq: i + 1,
    name,
    data,
    recordedAt: "2026-06-11T00:00:00Z",
  }));
}

/** Stub client recording checkpoint keys in first-recorded order. */
function scenarioClient(
  checkpoints: Map<string, unknown>,
  keys: string[],
): ActeonClient {
  let children = 0;
  return {
    async recordWorkflowCheckpoint(
      _executionId: string,
      _namespace: string,
      _tenant: string,
      name: string,
      data: unknown,
    ) {
      if (!checkpoints.has(name)) {
        keys.push(name);
        checkpoints.set(name, data);
      }
      return { name, seq: keys.length, data: checkpoints.get(name) };
    },
    async startChildWorkflow(
      _executionId: string,
      _namespace: string,
      _tenant: string,
      checkpoint: string,
    ) {
      if (!checkpoints.has(checkpoint)) {
        keys.push(checkpoint);
        children += 1;
        checkpoints.set(checkpoint, { child_id: `child-${children}` });
      }
      return (checkpoints.get(checkpoint) as { child_id: string }).child_id;
    },
  } as unknown as ActeonClient;
}

/**
 * Run an op sequence continuation-style, collecting checkpoint keys.
 * Mirrors a real execution: run from the top, let the first
 * un-checkpointed suspension unwind, record its checkpoint (as the
 * server would on resume), and re-run.
 */
async function runScenario(
  ops: Record<string, unknown>[],
): Promise<string[]> {
  const checkpoints = new Map<string, unknown>();
  const keys: string[] = [];

  const workflow = async (ctx: WorkflowContext): Promise<void> => {
    for (const op of ops) {
      switch (op.op) {
        case "step":
          await ctx.step(op.name as string, () => ({ r: 1 }));
          break;
        case "sleep":
          await ctx.sleep((op.seconds as number) ?? 1);
          break;
        case "wait_for_signal":
          await ctx.waitForSignal(op.name as string);
          break;
        case "start_child":
          await ctx.startChild(op.workflow as string, {});
          break;
        case "wait_for_child":
          await ctx.waitForChild(op.child_id as string);
          break;
        default:
          throw new Error(`unknown op: ${JSON.stringify(op)}`);
      }
    }
  };

  for (let round = 0; round < 100; round++) {
    const ctx = new WorkflowContext(
      scenarioClient(checkpoints, keys),
      "ns",
      "t1",
      "ex-1",
      {},
      toCheckpoints(checkpoints),
    );
    try {
      await workflow(ctx);
      return keys;
    } catch (error) {
      if (!(error instanceof WorkflowSuspend)) throw error;
      const directive = error.directive as { checkpoint: string };
      expect(checkpoints.has(directive.checkpoint)).toBe(false);
      keys.push(directive.checkpoint);
      checkpoints.set(
        directive.checkpoint,
        error.directive.directive === "sleep" ? {} : { payload: true },
      );
    }
  }
  throw new Error("scenario did not settle in 100 continuations");
}

/** Capture the directive thrown by a context operation. */
async function suspensionDirective(
  run: (ctx: WorkflowContext) => Promise<unknown>,
): Promise<WorkflowDirective> {
  const ctx = new WorkflowContext(
    undefined as unknown as ActeonClient,
    "ns",
    "t1",
    "ex-1",
    {},
    [],
  );
  try {
    await run(ctx);
  } catch (error) {
    if (error instanceof WorkflowSuspend) return error.directive;
    throw error;
  }
  throw new Error("operation did not suspend");
}

describe("contract: constants", () => {
  it("match the shared fixtures", () => {
    expect(WORKFLOW_TASK_ACTION_TYPE).toEqual(
      FIXTURES.constants.workflow_task_action_type,
    );
    expect(CHILD_RESULT_SIGNAL_PREFIX).toEqual(
      FIXTURES.constants.child_result_signal_prefix,
    );
  });
});

describe("contract: checkpoint keys", () => {
  for (const scenario of FIXTURES.checkpoint_key_scenarios) {
    it(scenario.name, async () => {
      expect(await runScenario(scenario.ops)).toEqual(scenario.expected_keys);
    });
  }
});

describe("contract: directive shapes", () => {
  it("sleep", async () => {
    const directive = await suspensionDirective((ctx) => ctx.sleep(30));
    expect(directive).toEqual(directiveFixture("sleep"));
  });

  it("await_signal with timeout", async () => {
    const directive = await suspensionDirective((ctx) =>
      ctx.waitForSignal("approved", 300),
    );
    expect(directive).toEqual(directiveFixture("await_signal"));
  });

  it("await_signal without timeout", async () => {
    const directive = await suspensionDirective((ctx) =>
      ctx.waitForSignal("go"),
    );
    expect(directive).toEqual(directiveFixture("await_signal_no_timeout"));
  });

  it("complete and fail via the runner", async () => {
    const payload = {
      execution_id: "ex-1",
      workflow: "wf",
      input: {},
      checkpoints: [],
    };
    const client = undefined as unknown as ActeonClient;

    const complete = await runWorkflowTask(
      client,
      "ns",
      "t1",
      async () => ({ ok: true, count: 3 }),
      payload,
    );
    expect(complete).toEqual(directiveFixture("complete"));

    const fail = await runWorkflowTask(
      client,
      "ns",
      "t1",
      async () => {
        throw new Error("provisioning broke");
      },
      payload,
    );
    expect(fail).toEqual(directiveFixture("fail"));
  });
});

describe("contract: integer-seconds coercions", () => {
  it("sleep seconds", async () => {
    for (const c of FIXTURES.sleep_coercions) {
      const directive = (await suspensionDirective((ctx) =>
        ctx.sleep(c.input_seconds),
      )) as { seconds: number };
      expect(directive.seconds).toEqual(c.expected_seconds);
      expect(Number.isInteger(directive.seconds)).toBe(true);
    }
  });

  it("signal timeout seconds", async () => {
    for (const c of FIXTURES.signal_timeout_coercions) {
      const directive = (await suspensionDirective((ctx) =>
        ctx.waitForSignal("x", c.input_seconds),
      )) as { timeout_seconds?: number };
      expect(directive.timeout_seconds).toEqual(c.expected_seconds);
      expect(Number.isInteger(directive.timeout_seconds)).toBe(true);
    }
  });
});

describe("contract: timed-out marker", () => {
  it("replays as null", async () => {
    const ctx = new WorkflowContext(
      undefined as unknown as ActeonClient,
      "ns",
      "t1",
      "ex-1",
      {},
      [
        {
          seq: 1,
          name: "signal:x#0",
          data: FIXTURES.constants.timed_out_marker,
          recordedAt: "2026-06-11T00:00:00Z",
        },
      ],
    );
    expect(await ctx.waitForSignal("x")).toBeNull();
  });
});
