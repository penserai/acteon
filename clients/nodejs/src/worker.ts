/**
 * Task-queue worker: a poll → handle → settle loop over the
 * `/v1/queues` lease protocol.
 *
 * Register plain task handlers with {@link Worker.register} and
 * workflow functions with {@link Worker.registerWorkflow}; then call
 * {@link Worker.run} (long-lived poll loop, stop with
 * {@link Worker.stop}) or {@link Worker.runOnce} (single poll, handy
 * in tests). While a handler runs, the worker auto-heartbeats the
 * task's lease every `leaseSeconds / 2` so slow handlers don't lose
 * the lease to redelivery.
 *
 * Handler outcomes:
 * - return value → task completed with the value as `result`
 * - throw → task failed with `retryable: true`
 * - throw {@link NonRetryableError} → task failed with
 *   `retryable: false` (no redelivery)
 *
 * Workflow continuation tasks (`action_type === "__workflow__"`) are
 * routed to the workflow function registered under
 * `payload.workflow`; the resulting {@link WorkflowDirective} settles
 * the task via the complete endpoint.
 */

import type { ActeonClient } from "./client.js";
import { ActeonError, NonRetryableError } from "./errors.js";
import { WorkerTask } from "./queues.js";
import {
  WORKFLOW_TASK_ACTION_TYPE,
  WorkflowDirective,
  WorkflowExecutionNotFoundError,
  WorkflowFn,
  WorkflowTaskPayload,
  runWorkflowTask,
} from "./workflows.js";

/**
 * A registered task handler. Receives the task payload (plus the full
 * task for attempt counts, IDs, etc.); the resolved value becomes the
 * task result.
 */
export type TaskHandler = (
  payload: unknown,
  task: WorkerTask,
) => Promise<unknown> | unknown;

/**
 * Configuration options for {@link Worker}.
 */
export interface WorkerOptions {
  /** Client used for polling, heartbeats, and settlement. */
  client: ActeonClient;
  /** Namespace the worker operates in. */
  namespace: string;
  /** Tenant the worker operates in. */
  tenant: string;
  /** Queue to poll. */
  queue: string;
  /** Stable worker identity recorded on leases. */
  workerId?: string;
  /** Delay between empty polls in milliseconds. Default: 1000. */
  pollIntervalMs?: number;
  /** Lease duration requested per poll, in seconds. Default: 60. */
  leaseSeconds?: number;
  /** Maximum tasks leased and handled concurrently per poll. Default: 1. */
  maxConcurrent?: number;
}

/**
 * Task-queue worker.
 *
 * @example
 * ```typescript
 * const worker = new Worker({
 *   client,
 *   namespace: "jobs",
 *   tenant: "tenant-1",
 *   queue: "billing",
 * });
 * worker.register("charge", async (payload) => {
 *   return { charged: true };
 * });
 * worker.registerWorkflow("onboarding", async (ctx, input) => {
 *   const user = await ctx.step("create-user", () => createUser(input));
 *   await ctx.sleep(3600);
 *   return user;
 * });
 * await worker.run();
 * ```
 */
export class Worker {
  private readonly client: ActeonClient;
  private readonly namespace: string;
  private readonly tenant: string;
  private readonly queue: string;
  private readonly workerId?: string;
  private readonly pollIntervalMs: number;
  private readonly leaseSeconds: number;
  private readonly maxConcurrent: number;

  private readonly handlers = new Map<string, TaskHandler>();
  private readonly workflows = new Map<string, WorkflowFn>();

  private running = false;
  private stopRequested = false;
  private wake?: () => void;

  constructor(options: WorkerOptions) {
    this.client = options.client;
    this.namespace = options.namespace;
    this.tenant = options.tenant;
    this.queue = options.queue;
    this.workerId = options.workerId;
    this.pollIntervalMs = options.pollIntervalMs ?? 1000;
    this.leaseSeconds = options.leaseSeconds ?? 60;
    this.maxConcurrent = options.maxConcurrent ?? 1;
  }

  /**
   * Register a handler for an action type. Replaces any handler
   * previously registered for the same action type.
   */
  register(actionType: string, handler: TaskHandler): this {
    this.handlers.set(actionType, handler);
    return this;
  }

  /**
   * Register a workflow function under a workflow name. Continuation
   * tasks (`action_type === "__workflow__"`) are routed to it by
   * `payload.workflow`.
   */
  registerWorkflow(name: string, fn: WorkflowFn): this {
    this.workflows.set(name, fn);
    return this;
  }

  /**
   * Poll once and handle the leased tasks (up to `maxConcurrent`,
   * concurrently). Returns the number of tasks handled. Exposed for
   * tests and for callers driving their own scheduling loop.
   */
  async runOnce(): Promise<number> {
    const tasks = await this.client.pollTasks(
      this.queue,
      this.namespace,
      this.tenant,
      {
        maxTasks: this.maxConcurrent,
        leaseSeconds: this.leaseSeconds,
        workerId: this.workerId,
      },
    );
    if (tasks.length === 0) {
      return 0;
    }
    const settled = await Promise.allSettled(
      tasks.map((task) => this.handleTask(task)),
    );
    for (const outcome of settled) {
      if (outcome.status === "rejected") {
        throw outcome.reason;
      }
    }
    return tasks.length;
  }

  /**
   * Run the poll loop until {@link stop} is called. Transport and
   * settlement errors are swallowed and retried after
   * `pollIntervalMs` — the loop only exits on `stop()`.
   */
  async run(): Promise<void> {
    if (this.running) {
      throw new ActeonError("worker is already running");
    }
    this.running = true;
    this.stopRequested = false;
    try {
      while (!this.stopRequested) {
        let handled = 0;
        try {
          handled = await this.runOnce();
        } catch {
          // Poll/settle failure: back off below and retry.
        }
        if (this.stopRequested) {
          break;
        }
        if (handled === 0) {
          await this.sleep(this.pollIntervalMs);
        }
      }
    } finally {
      this.running = false;
    }
  }

  /**
   * Request a graceful stop: in-flight handlers finish and settle,
   * then the {@link run} promise resolves. Idempotent.
   */
  stop(): void {
    this.stopRequested = true;
    this.wake?.();
  }

  /** Sleep that {@link stop} can interrupt. */
  private sleep(ms: number): Promise<void> {
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.wake = undefined;
        resolve();
      }, ms);
      this.wake = () => {
        clearTimeout(timer);
        this.wake = undefined;
        resolve();
      };
    });
  }

  /**
   * Handle one leased task: route to the registered handler (or the
   * workflow runner), auto-heartbeating the lease while it runs, then
   * settle via complete/fail.
   */
  private async handleTask(task: WorkerTask): Promise<void> {
    if (!task.leaseToken) {
      // Without a lease token the task cannot be settled; the lease
      // will expire server-side and the task will be redelivered.
      return;
    }
    let leaseToken = task.leaseToken;
    const heartbeat = setInterval(() => {
      void this.client
        .heartbeatTask(
          task.taskId,
          this.namespace,
          this.tenant,
          leaseToken,
          this.leaseSeconds,
        )
        .then((updated) => {
          if (updated.leaseToken) {
            leaseToken = updated.leaseToken;
          }
        })
        .catch(() => {
          // Heartbeat failures are non-fatal: the settle call decides.
        });
    }, this.leaseSeconds * 500);
    // Don't let the heartbeat timer keep the process alive on its own.
    heartbeat.unref?.();

    try {
      if (task.actionType === WORKFLOW_TASK_ACTION_TYPE) {
        await this.handleWorkflowTask(task, () => leaseToken);
        return;
      }

      const handler = this.handlers.get(task.actionType);
      if (!handler) {
        await this.client.failTask(
          task.taskId,
          this.namespace,
          this.tenant,
          leaseToken,
          `no handler registered for action type: ${task.actionType}`,
          true,
        );
        return;
      }

      let result: unknown;
      try {
        result = await handler(task.payload, task);
      } catch (error) {
        const message =
          error instanceof Error ? error.message : String(error);
        await this.client.failTask(
          task.taskId,
          this.namespace,
          this.tenant,
          leaseToken,
          message,
          !(error instanceof NonRetryableError),
        );
        return;
      }
      await this.client.completeTask(
        task.taskId,
        this.namespace,
        this.tenant,
        leaseToken,
        result ?? null,
      );
    } finally {
      clearInterval(heartbeat);
    }
  }

  /**
   * Handle one workflow continuation task: re-run the registered
   * workflow function and settle the task with the resulting
   * directive.
   */
  private async handleWorkflowTask(
    task: WorkerTask,
    leaseToken: () => string,
  ): Promise<void> {
    const payload = task.payload as WorkflowTaskPayload;
    const fn =
      typeof payload?.workflow === "string"
        ? this.workflows.get(payload.workflow)
        : undefined;
    if (!fn) {
      await this.client.failTask(
        task.taskId,
        this.namespace,
        this.tenant,
        leaseToken(),
        `no workflow registered: ${payload?.workflow ?? "<missing>"}`,
        true,
      );
      return;
    }
    let directive: WorkflowDirective;
    try {
      directive = await runWorkflowTask(
        this.client,
        this.namespace,
        this.tenant,
        fn,
        payload,
      );
    } catch (error) {
      // The runner only throws when the execution state could not be
      // resolved (the workflow function itself never ran): permanently
      // for a missing execution, retryably for transport failures.
      const message = error instanceof Error ? error.message : String(error);
      await this.client.failTask(
        task.taskId,
        this.namespace,
        this.tenant,
        leaseToken(),
        message,
        !(error instanceof WorkflowExecutionNotFoundError),
      );
      return;
    }
    await this.client.completeTask(
      task.taskId,
      this.namespace,
      this.tenant,
      leaseToken(),
      directive,
    );
  }
}
