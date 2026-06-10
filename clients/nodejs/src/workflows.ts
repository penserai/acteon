/**
 * Checkpoint-based workflow authoring surface — wire types, response
 * parsers, the {@link WorkflowContext} handed to workflow functions,
 * and the runner that turns one continuation task into a settlement
 * directive.
 *
 * A workflow execution runs customer code on external workers via the
 * worker task queue. The server never executes workflow logic — it
 * persists *checkpoints* (named, recorded results of completed steps)
 * and schedules continuation tasks. On every resume the worker re-runs
 * the workflow function from the top; the context replays recorded
 * checkpoints instantly (returning the stored result instead of
 * re-executing), so the function deterministically reaches the first
 * un-checkpointed operation and continues from there. Unlike
 * replay-based engines there is no determinism sandbox: only the order
 * and names of checkpointed operations must be stable across resumes.
 *
 * Suspension points (durable timers, signal waits) are expressed as
 * *directives* the worker returns when completing a workflow task —
 * see {@link WorkflowDirective}.
 *
 * The {@link ActeonClient} class carries the actual HTTP methods
 * (kept inline because TypeScript doesn't have Python-style mixins
 * and the class already aggregates every other surface).
 */

import type { ActeonClient } from "./client.js";

/** Action type used for workflow continuation tasks on worker queues. */
export const WORKFLOW_TASK_ACTION_TYPE = "__workflow__";

/** Signal-name prefix used to deliver child-workflow results to
 *  parents. `ctx.waitForChild(childId)` awaits the signal
 *  `__child:{childId}`. */
export const CHILD_RESULT_SIGNAL_PREFIX = "__child:";

/**
 * Lifecycle status of a workflow execution.
 */
export type WorkflowStatus =
  | "running"
  | "waiting_timer"
  | "waiting_signal"
  | "completed"
  | "failed"
  | "cancelled";

/**
 * A recorded checkpoint: the durable result of one completed operation
 * (step, fired timer, received signal, started child).
 */
export interface WorkflowCheckpoint {
  /** Dense 1-based sequence number. */
  seq: number;
  /** Unique checkpoint name within the execution. */
  name: string;
  /** Recorded payload, returned verbatim on replay. */
  data: unknown;
  /** When the checkpoint was recorded. */
  recordedAt: string;
}

/**
 * Parse a WorkflowCheckpoint from API response.
 */
export function parseWorkflowCheckpoint(
  data: Record<string, unknown>,
): WorkflowCheckpoint {
  return {
    seq: data.seq as number,
    name: data.name as string,
    data: data.data,
    recordedAt: data.recorded_at as string,
  };
}

/**
 * A workflow execution snapshot.
 */
export interface WorkflowExecution {
  /** Unique execution ID. */
  executionId: string;
  /** Workflow name. */
  workflow: string;
  /** Worker queue continuation tasks are routed through. */
  queue: string;
  /** Lifecycle status. */
  status: WorkflowStatus;
  /** Input the execution started with. */
  input: unknown;
  /** Result (when completed). */
  result?: unknown;
  /** Error (when failed/cancelled). */
  error?: string;
  /** Recorded checkpoints. */
  checkpoints: WorkflowCheckpoint[];
  /** What the execution is waiting on, when suspended. */
  awaiting?: Record<string, unknown>;
  /** Parent execution ID for child workflows. */
  parentId?: string;
  /** IDs of child executions. */
  children: string[];
  /** User-defined search attributes. */
  searchAttributes: Record<string, unknown>;
  /** When the execution started. */
  createdAt: string;
  /** When the execution was last updated. */
  updatedAt: string;
}

/**
 * Parse a WorkflowExecution from API response.
 */
export function parseWorkflowExecution(
  data: Record<string, unknown>,
): WorkflowExecution {
  const checkpoints = (data.checkpoints as Record<string, unknown>[]) ?? [];
  return {
    executionId: data.execution_id as string,
    workflow: data.workflow as string,
    queue: data.queue as string,
    status: data.status as WorkflowStatus,
    input: data.input,
    result: data.result,
    error: data.error as string | undefined,
    checkpoints: checkpoints.map(parseWorkflowCheckpoint),
    awaiting: data.awaiting as Record<string, unknown> | undefined,
    parentId: data.parent_id as string | undefined,
    children: (data.children as string[]) ?? [],
    searchAttributes:
      (data.search_attributes as Record<string, unknown>) ?? {},
    createdAt: data.created_at as string,
    updatedAt: data.updated_at as string,
  };
}

/**
 * Parse a `{ executions: [...] }` envelope (list response).
 */
export function parseWorkflowExecutionListResponse(
  data: Record<string, unknown>,
): WorkflowExecution[] {
  const executions = (data.executions as Record<string, unknown>[]) ?? [];
  return executions.map(parseWorkflowExecution);
}

/**
 * Response from recording a checkpoint. Recording is idempotent by
 * name: replays return the originally-recorded data.
 */
export interface RecordCheckpointResponse {
  /** Checkpoint name. */
  name: string;
  /** Dense 1-based sequence number. */
  seq: number;
  /** Recorded payload (the original data on replay). */
  data: unknown;
}

/**
 * Parse a RecordCheckpointResponse from API response.
 */
export function parseRecordCheckpointResponse(
  data: Record<string, unknown>,
): RecordCheckpointResponse {
  return {
    name: data.name as string,
    seq: data.seq as number,
    data: data.data,
  };
}

/**
 * Ordered event history of an execution.
 */
export interface ExecutionHistory {
  /** Execution ID the events belong to. */
  executionId: string;
  /** Ordered history events. */
  events: Record<string, unknown>[];
}

/**
 * Parse an ExecutionHistory from API response.
 */
export function parseExecutionHistory(
  data: Record<string, unknown>,
): ExecutionHistory {
  return {
    executionId: data.execution_id as string,
    events: (data.events as Record<string, unknown>[]) ?? [],
  };
}

/** Options accepted by `ActeonClient.startWorkflow`. */
export interface StartWorkflowOptions {
  /** User-defined search attributes for list filtering. */
  searchAttributes?: Record<string, unknown>;
}

/** Build the `POST /v1/workflows/start` request body. */
export function startWorkflowBody(
  namespace: string,
  tenant: string,
  workflow: string,
  queue: string,
  input: unknown,
  options: StartWorkflowOptions = {},
): Record<string, unknown> {
  const body: Record<string, unknown> = {
    namespace,
    tenant,
    workflow,
    queue,
    input,
  };
  if (options.searchAttributes !== undefined) {
    body.search_attributes = options.searchAttributes;
  }
  return body;
}

/** What happens to running children when a parent workflow closes. */
export type ParentClosePolicy = "abandon" | "cancel";

/** Options accepted by {@link WorkflowContext.startChild} and
 *  `ActeonClient.startChildWorkflow`. */
export interface StartChildOptions {
  /** Worker queue for the child (defaults to the parent's queue). */
  queue?: string;
  /** What happens to the child when the parent closes. */
  parentClosePolicy?: ParentClosePolicy;
}

/**
 * Directive returned by a worker when settling a workflow continuation
 * task: either the workflow finished (complete/fail) or it suspends
 * until a timer or signal resolves.
 *
 * Sent verbatim as the worker task's `result` payload, so the fields
 * use the snake_case wire shape.
 */
export type WorkflowDirective =
  | { directive: "complete"; result: unknown }
  | { directive: "fail"; error: string }
  | { directive: "sleep"; checkpoint: string; seconds: number }
  | {
      directive: "await_signal";
      checkpoint: string;
      name: string;
      timeout_seconds?: number;
    };

/**
 * Internal control-flow signal thrown by {@link WorkflowContext} at an
 * un-checkpointed suspension point (`sleep`, `waitForSignal`). The
 * workflow runner catches it and settles the continuation task with
 * the carried directive. Never surfaces to workflow authors unless
 * they catch-and-swallow inside the workflow function — don't.
 */
export class WorkflowSuspend extends Error {
  /** The directive to settle the continuation task with. */
  readonly directive: WorkflowDirective;

  constructor(directive: WorkflowDirective) {
    super(`workflow suspended: ${directive.directive}`);
    this.name = "WorkflowSuspend";
    this.directive = directive;
  }
}

/** Recorded checkpoint data for a signal wait that timed out. */
function isTimedOutMarker(data: unknown): boolean {
  return (
    typeof data === "object" &&
    data !== null &&
    (data as Record<string, unknown>).timed_out === true
  );
}

/**
 * Deterministic execution context handed to workflow functions.
 *
 * Checkpoint keys are derived from the operation name plus a per-name
 * occurrence counter (`step:{name}#{k}`, `sleep#{k}`,
 * `signal:{name}#{k}`, `child:{workflow}#{k}` with `k` starting at
 * 0), so the same code produces the same keys on every re-run. Keep
 * the order and names of checkpointed operations stable across
 * resumes; everything else (logging, local computation) is
 * unconstrained.
 */
export class WorkflowContext {
  /** Execution ID of the running workflow. */
  readonly executionId: string;
  /** Input the execution started with. */
  readonly input: unknown;

  private readonly client: ActeonClient;
  private readonly namespace: string;
  private readonly tenant: string;
  private readonly checkpoints: Map<string, unknown>;
  private readonly counters = new Map<string, number>();

  constructor(
    client: ActeonClient,
    namespace: string,
    tenant: string,
    executionId: string,
    input: unknown,
    checkpoints: WorkflowCheckpoint[],
  ) {
    this.client = client;
    this.namespace = namespace;
    this.tenant = tenant;
    this.executionId = executionId;
    this.input = input;
    this.checkpoints = new Map(checkpoints.map((c) => [c.name, c.data]));
  }

  /** Derive the next checkpoint key for `prefix`: `{prefix}#{k}` with
   *  `k` the 0-based occurrence count of `prefix` so far in this run.
   *  Matches the Python SDK so executions can migrate between
   *  workers written in different languages. */
  private nextKey(prefix: string): string {
    const k = this.counters.get(prefix) ?? 0;
    this.counters.set(prefix, k + 1);
    return `${prefix}#${k}`;
  }

  /**
   * Run a named step exactly once. On replay the recorded result is
   * returned without re-executing `fn`; otherwise `fn` runs, its
   * result is recorded as checkpoint `step:{name}#{k}`, and the
   * recorded data is returned (the original data if a previous
   * attempt already recorded the checkpoint).
   */
  async step<T>(name: string, fn: () => T | Promise<T>): Promise<T> {
    const key = this.nextKey(`step:${name}`);
    if (this.checkpoints.has(key)) {
      return this.checkpoints.get(key) as T;
    }
    const result = await fn();
    const recorded = await this.client.recordWorkflowCheckpoint(
      this.executionId,
      this.namespace,
      this.tenant,
      key,
      result ?? null,
    );
    this.checkpoints.set(key, recorded.data);
    return recorded.data as T;
  }

  /**
   * Suspend on a durable timer for `seconds`. Replays return
   * immediately once the timer-fire checkpoint (`sleep#{k}`) is
   * recorded.
   */
  async sleep(seconds: number): Promise<void> {
    const key = this.nextKey("sleep");
    if (this.checkpoints.has(key)) {
      return;
    }
    // The server's directive schema requires integer seconds; a float
    // would fail deserialization and fail the execution.
    throw new WorkflowSuspend({
      directive: "sleep",
      checkpoint: key,
      seconds: Math.max(1, Math.floor(seconds)),
    });
  }

  /**
   * Suspend until the external signal `name` arrives, optionally with
   * a timeout. Returns the signal payload; returns `null` when the
   * wait timed out (the server records `{"timed_out": true}` as the
   * `signal:{name}#{k}` checkpoint data).
   */
  async waitForSignal(
    name: string,
    timeoutSeconds?: number,
  ): Promise<unknown> {
    const key = this.nextKey(`signal:${name}`);
    if (this.checkpoints.has(key)) {
      const data = this.checkpoints.get(key);
      return isTimedOutMarker(data) ? null : data;
    }
    const directive: WorkflowDirective = {
      directive: "await_signal",
      checkpoint: key,
      name,
    };
    if (timeoutSeconds !== undefined) {
      // Integer seconds required by the server's directive schema.
      directive.timeout_seconds = Math.max(1, Math.floor(timeoutSeconds));
    }
    throw new WorkflowSuspend(directive);
  }

  /**
   * Start a child workflow, idempotently keyed by checkpoint
   * `child:{workflow}#{k}`. Returns the child execution ID (the
   * recorded one on replay). Await the child's terminal result with
   * {@link waitForChild}.
   */
  async startChild(
    workflow: string,
    input: unknown,
    options?: StartChildOptions,
  ): Promise<string> {
    const key = this.nextKey(`child:${workflow}`);
    if (this.checkpoints.has(key)) {
      const data = this.checkpoints.get(key) as Record<string, unknown>;
      return data.child_id as string;
    }
    const childId = await this.client.startChildWorkflow(
      this.executionId,
      this.namespace,
      this.tenant,
      key,
      workflow,
      input,
      options,
    );
    this.checkpoints.set(key, { child_id: childId });
    return childId;
  }

  /**
   * Suspend until the child execution reaches a terminal state. The
   * server delivers child completion as the well-known signal
   * `__child:{childId}` carrying a `{"status", "result"?/"error"?}`
   * payload; returns that payload, or `null` when `timeoutSeconds`
   * elapsed first.
   */
  async waitForChild(
    childId: string,
    timeoutSeconds?: number,
  ): Promise<unknown> {
    return this.waitForSignal(
      `${CHILD_RESULT_SIGNAL_PREFIX}${childId}`,
      timeoutSeconds,
    );
  }
}

/**
 * A registered workflow function. Re-run from the top on every
 * continuation; must derive all effects through `ctx` so replays are
 * deterministic.
 */
export type WorkflowFn = (
  ctx: WorkflowContext,
  input: unknown,
) => Promise<unknown> | unknown;

/**
 * Payload carried by a workflow continuation task
 * (`action_type === "__workflow__"`), in wire shape.
 */
export interface WorkflowTaskPayload {
  execution_id: string;
  workflow: string;
  input: unknown;
  checkpoints: Record<string, unknown>[];
}

/**
 * Run one workflow continuation: build a {@link WorkflowContext} from
 * the task payload, invoke `fn`, and translate the outcome into the
 * directive to settle the task with — `complete` when the function
 * returns, the suspension's own directive on {@link WorkflowSuspend},
 * and `fail` on any other throw.
 */
export async function runWorkflowTask(
  client: ActeonClient,
  namespace: string,
  tenant: string,
  fn: WorkflowFn,
  payload: WorkflowTaskPayload,
): Promise<WorkflowDirective> {
  const checkpoints = (payload.checkpoints ?? []).map(parseWorkflowCheckpoint);
  const ctx = new WorkflowContext(
    client,
    namespace,
    tenant,
    payload.execution_id,
    payload.input,
    checkpoints,
  );
  try {
    const result = await fn(ctx, payload.input);
    return { directive: "complete", result: result ?? null };
  } catch (error) {
    if (error instanceof WorkflowSuspend) {
      return error.directive;
    }
    const message =
      error instanceof Error ? error.message : String(error);
    return { directive: "fail", error: message };
  }
}
