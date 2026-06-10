/**
 * Worker task queue surface — wire types, request body builders, and
 * response parsers for the `/v1/queues` endpoints.
 *
 * External workers drive this API: they poll a named queue for leased
 * tasks, execute them, and settle each one via complete/fail (the
 * lease token from the poll response authenticates heartbeat /
 * complete / fail). Chain `worker` steps and workflow continuation
 * tasks flow through the same queues.
 *
 * The {@link ActeonClient} class carries the actual HTTP methods
 * (kept inline because TypeScript doesn't have Python-style mixins
 * and the class already aggregates every other surface); the
 * `Worker` class in `worker.ts` wraps them in a poll loop.
 */

/**
 * Lifecycle status of a worker task.
 */
export type WorkerTaskStatus =
  | "pending"
  | "leased"
  | "completed"
  | "failed"
  | "cancelled";

/**
 * A task on a worker queue.
 */
export interface WorkerTask {
  /** Unique task ID. */
  taskId: string;
  /** Queue the task is routed through. */
  queue: string;
  /** Action type for worker handler dispatch. */
  actionType: string;
  /** Task payload. */
  payload: unknown;
  /** Lifecycle status. */
  status: WorkerTaskStatus;
  /** Delivery attempt (1-based once leased). */
  attempt: number;
  /** Maximum delivery attempts. */
  maxAttempts: number;
  /** Lease token (present in poll responses; required for heartbeat /
   *  complete / fail). */
  leaseToken?: string;
  /** When the current lease expires. */
  leaseExpiresAt?: string;
  /** Result reported by the worker. */
  result?: unknown;
  /** Error reported on failure. */
  error?: string;
  /** Owning chain execution, for chain `worker` steps. */
  chainId?: string;
  /** Owning workflow execution, for workflow continuation tasks. */
  workflowExecutionId?: string;
  /** When the task was enqueued. */
  createdAt: string;
  /** When the task was last updated. */
  updatedAt: string;
}

/**
 * Parse a WorkerTask from API response.
 */
export function parseWorkerTask(data: Record<string, unknown>): WorkerTask {
  return {
    taskId: data.task_id as string,
    queue: data.queue as string,
    actionType: data.action_type as string,
    payload: data.payload,
    status: data.status as WorkerTaskStatus,
    attempt: data.attempt as number,
    maxAttempts: data.max_attempts as number,
    leaseToken: data.lease_token as string | undefined,
    leaseExpiresAt: data.lease_expires_at as string | undefined,
    result: data.result,
    error: data.error as string | undefined,
    chainId: data.chain_id as string | undefined,
    workflowExecutionId: data.workflow_execution_id as string | undefined,
    createdAt: data.created_at as string,
    updatedAt: data.updated_at as string,
  };
}

/**
 * Parse a `{ tasks: [...] }` envelope (poll + list responses).
 */
export function parseTaskListResponse(
  data: Record<string, unknown>,
): WorkerTask[] {
  const tasks = (data.tasks as Record<string, unknown>[]) ?? [];
  return tasks.map(parseWorkerTask);
}

/** Options accepted by `ActeonClient.pollTasks`. */
export interface PollTasksOptions {
  /** Maximum number of tasks to lease in one poll. */
  maxTasks?: number;
  /** Lease duration in seconds for the returned tasks. */
  leaseSeconds?: number;
  /** Stable worker identity recorded on the lease. */
  workerId?: string;
}

/** Build the `POST /v1/queues/{queue}/poll` request body. */
export function pollTasksBody(
  namespace: string,
  tenant: string,
  options: PollTasksOptions = {},
): Record<string, unknown> {
  const body: Record<string, unknown> = { namespace, tenant };
  if (options.maxTasks !== undefined) body.max_tasks = options.maxTasks;
  if (options.leaseSeconds !== undefined) {
    body.lease_seconds = options.leaseSeconds;
  }
  if (options.workerId !== undefined) body.worker_id = options.workerId;
  return body;
}

/** Options accepted by `ActeonClient.enqueueTask`. */
export interface EnqueueTaskOptions {
  /** Maximum delivery attempts before the task fails terminally. */
  maxAttempts?: number;
}

/** Build the `POST /v1/queues/{queue}/tasks` request body. */
export function enqueueTaskBody(
  namespace: string,
  tenant: string,
  actionType: string,
  payload: unknown,
  options: EnqueueTaskOptions = {},
): Record<string, unknown> {
  const body: Record<string, unknown> = {
    namespace,
    tenant,
    action_type: actionType,
    payload,
  };
  if (options.maxAttempts !== undefined) {
    body.max_attempts = options.maxAttempts;
  }
  return body;
}

/** Build the `POST /v1/queues/tasks/{task_id}/heartbeat` request body. */
export function heartbeatTaskBody(
  namespace: string,
  tenant: string,
  leaseToken: string,
  extendSeconds?: number,
): Record<string, unknown> {
  const body: Record<string, unknown> = {
    namespace,
    tenant,
    lease_token: leaseToken,
  };
  if (extendSeconds !== undefined) body.extend_seconds = extendSeconds;
  return body;
}

/** Build the `POST /v1/queues/tasks/{task_id}/complete` request body. */
export function completeTaskBody(
  namespace: string,
  tenant: string,
  leaseToken: string,
  result: unknown,
): Record<string, unknown> {
  return { namespace, tenant, lease_token: leaseToken, result };
}

/** Build the `POST /v1/queues/tasks/{task_id}/fail` request body. */
export function failTaskBody(
  namespace: string,
  tenant: string,
  leaseToken: string,
  error: string,
  retryable: boolean,
): Record<string, unknown> {
  return { namespace, tenant, lease_token: leaseToken, error, retryable };
}
