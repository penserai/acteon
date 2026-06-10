package com.acteon.client;

import com.acteon.client.exceptions.ActeonException;
import com.acteon.client.exceptions.NonRetryableException;
import com.fasterxml.jackson.databind.JsonNode;

import java.time.Duration;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ThreadFactory;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Polling task-queue worker — mirrors the Go/Python/Node SDK Workers.
 *
 * <p>A Worker wraps the {@code /v1/queues} primitives on
 * {@link ActeonClient} into a long-running poll loop: lease tasks,
 * dispatch them to registered handlers with bounded concurrency, keep
 * leases alive with automatic heartbeats at half the lease interval,
 * and report complete/fail back to the server.
 *
 * <p>Example usage:</p>
 * <pre>{@code
 * ActeonClient client = new ActeonClient("http://localhost:8080");
 * try (Worker worker = Worker.builder(client, "jobs", "tenant-1", "emails")
 *         .maxConcurrent(4)
 *         .build()) {
 *     worker.register("send_email", payload -> {
 *         doSend(payload.get("to").asText());
 *         return Map.of("sent", true);
 *     });
 *     worker.run(); // blocks until worker.stop()
 * }
 * }</pre>
 *
 * <p>Failure convention: a plain exception from a handler fails the
 * task with {@code retryable=true} — transient breakage (network,
 * upstream 5xx) is the common case, and the server's
 * {@code max_attempts} bounds the blast radius. Throw
 * {@link NonRetryableException} to fail permanently. Tasks whose
 * action type has no registered handler fail retryable, so another
 * worker on the same queue may carry the handler.
 */
public final class Worker implements AutoCloseable {

    /**
     * Executes one task. Receives the task's JSON payload and returns
     * the result to report on completion (any JSON-serializable
     * value, or {@code null}). Throwing fails the task as retryable;
     * throw {@link NonRetryableException} to mark the failure
     * terminal.
     */
    @FunctionalInterface
    public interface TaskHandler {
        Object handle(JsonNode payload) throws Exception;
    }

    private final ActeonClient client;
    private final String namespace;
    private final String tenant;
    private final String queue;
    private final String workerId;
    private final Duration pollInterval;
    private final int leaseSeconds;
    private final int maxConcurrent;
    /**
     * Overrides the leaseSeconds/2 heartbeat cadence when non-null
     * (tests shrink it to keep slow-handler coverage fast).
     */
    private final Duration heartbeatInterval;

    private final Map<String, TaskHandler> handlers = new ConcurrentHashMap<>();
    private final ExecutorService executor;
    private final ScheduledExecutorService heartbeatScheduler;

    /** Guards the poll-loop sleep so {@link #stop} wakes it immediately. */
    private final Object stopLock = new Object();
    private volatile boolean stopped = false;

    private Worker(Builder builder) {
        this.client = builder.client;
        this.namespace = builder.namespace;
        this.tenant = builder.tenant;
        this.queue = builder.queue;
        this.workerId = builder.workerId != null
            ? builder.workerId
            : "worker-" + UUID.randomUUID().toString().replace("-", "").substring(0, 12);
        this.pollInterval = builder.pollInterval;
        this.leaseSeconds = builder.leaseSeconds;
        this.maxConcurrent = builder.maxConcurrent;
        this.heartbeatInterval = builder.heartbeatInterval;
        this.executor = Executors.newFixedThreadPool(
            maxConcurrent, daemonThreads("acteon-" + workerId));
        // One heartbeat thread per possible in-flight task, so one
        // slow heartbeat round-trip can't starve another task's lease.
        this.heartbeatScheduler = Executors.newScheduledThreadPool(
            maxConcurrent, daemonThreads("acteon-heartbeat-" + workerId));
    }

    private static ThreadFactory daemonThreads(String prefix) {
        AtomicInteger n = new AtomicInteger(0);
        return runnable -> {
            Thread t = new Thread(runnable, prefix + "-" + n.incrementAndGet());
            t.setDaemon(true);
            return t;
        };
    }

    /**
     * Creates a builder for a Worker bound to {@code client} and the
     * given scope.
     *
     * @param client the Acteon client used for every queue call
     * @param namespace namespace scoping every queue call (required)
     * @param tenant tenant scoping every queue call (required)
     * @param queue the queue to poll (required)
     * @throws IllegalArgumentException if a required argument is null
     *     or empty
     */
    public static Builder builder(ActeonClient client, String namespace, String tenant, String queue) {
        return new Builder(client, namespace, tenant, queue);
    }

    /** Builder for {@link Worker}. */
    public static final class Builder {
        private final ActeonClient client;
        private final String namespace;
        private final String tenant;
        private final String queue;
        private String workerId;
        private Duration pollInterval = Duration.ofSeconds(1);
        private int leaseSeconds = 60;
        private int maxConcurrent = 1;
        private Duration heartbeatInterval;

        private Builder(ActeonClient client, String namespace, String tenant, String queue) {
            if (client == null) {
                throw new IllegalArgumentException("client is required");
            }
            if (namespace == null || namespace.isEmpty()
                || tenant == null || tenant.isEmpty()
                || queue == null || queue.isEmpty()) {
                throw new IllegalArgumentException("namespace, tenant, and queue are required");
            }
            this.client = client;
            this.namespace = namespace;
            this.tenant = tenant;
            this.queue = queue;
        }

        /**
         * Stable worker identity sent on poll (for observability); a
         * random one is generated when omitted.
         */
        public Builder workerId(String workerId) {
            this.workerId = workerId;
            return this;
        }

        /**
         * How long to wait between polls when the queue is empty or a
         * poll fails. Defaults to 1 second.
         */
        public Builder pollInterval(Duration pollInterval) {
            if (pollInterval == null || pollInterval.isNegative() || pollInterval.isZero()) {
                throw new IllegalArgumentException("pollInterval must be positive");
            }
            this.pollInterval = pollInterval;
            return this;
        }

        /**
         * Lease duration requested on poll and re-requested on every
         * heartbeat. Heartbeats fire every leaseSeconds/2 while a
         * handler runs. Defaults to 60.
         */
        public Builder leaseSeconds(int leaseSeconds) {
            if (leaseSeconds <= 0) {
                throw new IllegalArgumentException("leaseSeconds must be positive");
            }
            this.leaseSeconds = leaseSeconds;
            return this;
        }

        /**
         * Bounds how many tasks run at once (and how many tasks each
         * poll leases). Defaults to 1.
         */
        public Builder maxConcurrent(int maxConcurrent) {
            if (maxConcurrent <= 0) {
                throw new IllegalArgumentException("maxConcurrent must be positive");
            }
            this.maxConcurrent = maxConcurrent;
            return this;
        }

        /**
         * Overrides the leaseSeconds/2 heartbeat cadence.
         * Package-private: tests shrink it to keep slow-handler
         * coverage fast; production code should size
         * {@link #leaseSeconds} instead.
         */
        Builder heartbeatInterval(Duration heartbeatInterval) {
            this.heartbeatInterval = heartbeatInterval;
            return this;
        }

        public Worker build() {
            return new Worker(this);
        }
    }

    /** The worker identity sent on every poll. */
    public String getWorkerId() {
        return workerId;
    }

    /**
     * Registers {@code handler} for tasks whose action type equals
     * {@code actionType}, replacing any previous handler for that
     * type. Safe to call concurrently with a running Worker.
     */
    public void register(String actionType, TaskHandler handler) {
        if (actionType == null || handler == null) {
            throw new IllegalArgumentException("actionType and handler are required");
        }
        handlers.put(actionType, handler);
    }

    /**
     * Polls the queue until {@link #stop} is called, dispatching each
     * leased task to its registered handler with at most
     * {@code maxConcurrent} tasks in flight. Poll errors do not stop
     * the loop — the Worker waits {@code pollInterval} and retries,
     * so transient server blips self-heal. In-flight handlers finish
     * (and report their outcome) before {@code run} returns.
     */
    public void run() {
        while (!stopped) {
            int n = 0;
            boolean polled = true;
            try {
                n = runOnce();
            } catch (ActeonException e) {
                // Poll failures self-heal: wait pollInterval and retry.
                polled = false;
            }
            if (stopped) {
                return;
            }
            // Poll again immediately while the queue keeps yielding
            // work; back off by pollInterval when it's empty or the
            // poll failed.
            if (polled && n > 0) {
                continue;
            }
            synchronized (stopLock) {
                if (stopped) {
                    return;
                }
                try {
                    stopLock.wait(Math.max(1L, pollInterval.toMillis()));
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return;
                }
            }
        }
    }

    /**
     * Performs a single poll-and-drain pass: lease up to
     * {@code maxConcurrent} tasks, run them concurrently to
     * completion, and report each outcome. Returns the number of
     * tasks leased. A zero count means the queue had nothing to
     * lease. Mainly useful for tests and cron-style invocations.
     *
     * @throws ActeonException if the poll itself fails; handler and
     *     settlement failures are reported to the server, not thrown
     */
    public int runOnce() throws ActeonException {
        java.util.List<Queues.WorkerTask> tasks = client.pollTasks(queue,
            new Queues.PollTasksRequest(namespace, tenant, maxConcurrent, leaseSeconds, workerId));
        if (tasks.isEmpty()) {
            return 0;
        }
        CountDownLatch done = new CountDownLatch(tasks.size());
        for (Queues.WorkerTask task : tasks) {
            executor.submit(() -> {
                try {
                    processTask(task);
                } finally {
                    done.countDown();
                }
            });
        }
        try {
            done.await();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        return tasks.size();
    }

    /**
     * Signals {@link #run} to exit. Safe to call from any thread
     * (e.g. a shutdown hook). The run loop stops polling immediately
     * and returns once in-flight tasks have settled. Stopping is
     * terminal — build a fresh Worker to poll again.
     */
    public void stop() {
        synchronized (stopLock) {
            stopped = true;
            stopLock.notifyAll();
        }
    }

    /** Stops the worker and shuts down its executors. */
    @Override
    public void close() {
        stop();
        executor.shutdown();
        heartbeatScheduler.shutdownNow();
        try {
            if (!executor.awaitTermination(5, TimeUnit.SECONDS)) {
                executor.shutdownNow();
            }
        } catch (InterruptedException e) {
            executor.shutdownNow();
            Thread.currentThread().interrupt();
        }
    }

    /**
     * Runs one leased task end to end: heartbeat schedule, handler
     * dispatch, then a complete or fail report. Reporting errors are
     * dropped — a lost report simply lets the lease expire and the
     * server re-queue the task.
     */
    private void processTask(Queues.WorkerTask task) {
        TaskHandler handler = handlers.get(task.actionType());
        if (handler == null) {
            // No handler on this worker. Fail retryable so the task
            // can be re-leased — another worker on the same queue may
            // carry the handler.
            reportFail(task,
                "no handler registered for action type \"" + task.actionType() + "\"",
                true);
            return;
        }

        // Auto-heartbeat at half the lease duration while the handler
        // runs, so slow handlers keep their lease. Heartbeat errors
        // are dropped: a lost lease surfaces as a conflict on the
        // final complete/fail report, and the server re-queues.
        long intervalMs = heartbeatInterval != null
            ? Math.max(1L, heartbeatInterval.toMillis())
            : leaseSeconds * 1000L / 2;
        ScheduledFuture<?> heartbeat = heartbeatScheduler.scheduleAtFixedRate(() -> {
            try {
                client.heartbeatTask(task.taskId(), new Queues.HeartbeatTaskRequest(
                    namespace, tenant, task.leaseToken(), leaseSeconds));
            } catch (ActeonException ignored) {
                // Surfaces on the final complete/fail report.
            }
        }, intervalMs, intervalMs, TimeUnit.MILLISECONDS);

        Object result = null;
        Exception failure = null;
        try {
            result = handler.handle(task.payload());
        } catch (Exception e) {
            failure = e;
        } finally {
            heartbeat.cancel(false);
        }

        if (failure != null) {
            reportFail(task, errorMessage(failure),
                !(failure instanceof NonRetryableException));
        } else {
            reportComplete(task, result);
        }
    }

    private static String errorMessage(Exception e) {
        return e.getMessage() != null ? e.getMessage() : e.toString();
    }

    private void reportComplete(Queues.WorkerTask task, Object result) {
        try {
            client.completeTask(task.taskId(), new Queues.CompleteTaskRequest(
                namespace, tenant, task.leaseToken(), result));
        } catch (ActeonException ignored) {
            // Lost report: the lease lapses and the server re-queues.
        }
    }

    private void reportFail(Queues.WorkerTask task, String error, boolean retryable) {
        try {
            client.failTask(task.taskId(), new Queues.FailTaskRequest(
                namespace, tenant, task.leaseToken(), error, retryable));
        } catch (ActeonException ignored) {
            // Lost report: the lease lapses and the server re-queues.
        }
    }
}
