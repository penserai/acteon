package com.acteon.client;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.databind.JsonNode;

import java.util.List;

/**
 * Container class for the Acteon worker task-queue surface
 * ({@code /v1/queues}).
 *
 * <p>External workers drive this API: they poll a named queue for
 * tasks, execute them, and report results via complete/fail. Chain
 * {@code worker} steps and workflow continuation tasks flow through
 * the same queues. The DTOs live as nested records to keep the queue
 * types in one place, mirroring {@link Bus}. Wire fields use
 * snake_case via {@code @JsonProperty}; Java field names stay
 * camelCase to match the SDK convention. Optional request fields use
 * boxed types ({@code Integer}) so callers can pass {@code null} to
 * mean "unset" — Jackson + {@code @JsonInclude(NON_NULL)} drops
 * nulls from the wire form.
 *
 * <p>Queue methods themselves live as members of {@link ActeonClient};
 * see {@link Worker} for the higher-level polling worker built on
 * top of them.
 */
public final class Queues {
    private Queues() {}

    // =========================================================================
    // Task lifecycle statuses (WorkerTask.status)
    // =========================================================================

    /** Enqueued, waiting to be leased. */
    public static final String TASK_STATUS_PENDING = "pending";
    /** Leased to a worker; the lease token gates heartbeat / complete / fail. */
    public static final String TASK_STATUS_LEASED = "leased";
    /** Terminal success. */
    public static final String TASK_STATUS_COMPLETED = "completed";
    /** Terminal failure (non-retryable, or the attempt budget is exhausted). */
    public static final String TASK_STATUS_FAILED = "failed";
    /** Cancelled before completion. */
    public static final String TASK_STATUS_CANCELLED = "cancelled";

    // =========================================================================
    // Task
    // =========================================================================

    /**
     * A task on a worker queue.
     *
     * @param taskId unique task ID
     * @param queue queue the task is routed through
     * @param actionType drives worker handler dispatch
     * @param payload arbitrary JSON payload delivered to the worker
     * @param status lifecycle status (see the {@code TASK_STATUS_*} constants)
     * @param attempt delivery attempt (1-based once leased)
     * @param maxAttempts maximum number of delivery attempts
     * @param leaseToken present in poll responses; required for
     *     heartbeat / complete / fail
     * @param leaseExpiresAt when the current lease expires (RFC 3339)
     * @param result result reported by the worker on completion
     * @param error error reported on failure
     * @param chainId owning chain execution, for chain worker steps
     * @param workflowExecutionId owning workflow execution, for
     *     workflow continuation tasks
     * @param createdAt when the task was enqueued (RFC 3339)
     * @param updatedAt when the task was last updated (RFC 3339)
     */
    public record WorkerTask(
        @JsonProperty("task_id") String taskId,
        String queue,
        @JsonProperty("action_type") String actionType,
        JsonNode payload,
        String status,
        int attempt,
        @JsonProperty("max_attempts") int maxAttempts,
        @JsonProperty("lease_token") String leaseToken,
        @JsonProperty("lease_expires_at") String leaseExpiresAt,
        JsonNode result,
        String error,
        @JsonProperty("chain_id") String chainId,
        @JsonProperty("workflow_execution_id") String workflowExecutionId,
        @JsonProperty("created_at") String createdAt,
        @JsonProperty("updated_at") String updatedAt
    ) {}

    // =========================================================================
    // Request bodies
    // =========================================================================

    /**
     * Request body for enqueueing a task
     * ({@code POST /v1/queues/{queue}/tasks}).
     *
     * @param namespace namespace scoping the task
     * @param tenant tenant scoping the task
     * @param actionType drives worker handler dispatch
     * @param payload arbitrary JSON payload delivered to the worker
     * @param maxAttempts maximum number of delivery attempts (server
     *     default 3 when null)
     */
    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record EnqueueTaskRequest(
        String namespace,
        String tenant,
        @JsonProperty("action_type") String actionType,
        Object payload,
        @JsonProperty("max_attempts") Integer maxAttempts
    ) {
        public EnqueueTaskRequest(String namespace, String tenant, String actionType, Object payload) {
            this(namespace, tenant, actionType, payload, null);
        }
    }

    /**
     * Request body for polling a queue
     * ({@code POST /v1/queues/{queue}/poll}).
     *
     * @param namespace namespace scoping the poll
     * @param tenant tenant scoping the poll
     * @param maxTasks maximum number of tasks to lease in one poll
     *     (server default 1 when null)
     * @param leaseSeconds lease duration in seconds (server default
     *     60, max 3600)
     * @param workerId identifies the polling worker (for observability)
     */
    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record PollTasksRequest(
        String namespace,
        String tenant,
        @JsonProperty("max_tasks") Integer maxTasks,
        @JsonProperty("lease_seconds") Integer leaseSeconds,
        @JsonProperty("worker_id") String workerId
    ) {
        public PollTasksRequest(String namespace, String tenant) {
            this(namespace, tenant, null, null, null);
        }
    }

    /**
     * Request body for extending a task lease
     * ({@code POST /v1/queues/tasks/{taskId}/heartbeat}).
     *
     * @param namespace namespace scoping the task
     * @param tenant tenant scoping the task
     * @param leaseToken lease token returned by poll
     * @param extendSeconds new lease duration in seconds from now
     *     (server default 60 when null)
     */
    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record HeartbeatTaskRequest(
        String namespace,
        String tenant,
        @JsonProperty("lease_token") String leaseToken,
        @JsonProperty("extend_seconds") Integer extendSeconds
    ) {
        public HeartbeatTaskRequest(String namespace, String tenant, String leaseToken) {
            this(namespace, tenant, leaseToken, null);
        }
    }

    /**
     * Request body for completing a task
     * ({@code POST /v1/queues/tasks/{taskId}/complete}).
     *
     * @param namespace namespace scoping the task
     * @param tenant tenant scoping the task
     * @param leaseToken lease token returned by poll
     * @param result task result. For chain worker steps this becomes
     *     the step's response body; for workflow tasks it carries the
     *     directive. {@code null} is dropped from the wire form and
     *     the server defaults the result to JSON {@code null}.
     */
    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record CompleteTaskRequest(
        String namespace,
        String tenant,
        @JsonProperty("lease_token") String leaseToken,
        Object result
    ) {}

    /**
     * Request body for failing a task
     * ({@code POST /v1/queues/tasks/{taskId}/fail}).
     *
     * @param namespace namespace scoping the task
     * @param tenant tenant scoping the task
     * @param leaseToken lease token returned by poll
     * @param error error message
     * @param retryable whether the failure is retryable. Retryable
     *     failures within the attempt budget re-queue the task with
     *     backoff; non-retryable failures are terminal.
     */
    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record FailTaskRequest(
        String namespace,
        String tenant,
        @JsonProperty("lease_token") String leaseToken,
        String error,
        boolean retryable
    ) {}

    // =========================================================================
    // Response envelopes
    // =========================================================================

    /** Wire envelope for poll and list replies. */
    public record TaskListResponse(
        List<WorkerTask> tasks
    ) {}
}
