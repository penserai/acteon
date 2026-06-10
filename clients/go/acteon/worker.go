// Polling task-queue Worker for the Go ActeonClient.
//
// A Worker wraps the `/v1/queues` primitives in `queues.go` into a
// long-running poll loop: lease tasks, dispatch them to registered
// handlers with bounded concurrency, keep leases alive with
// automatic heartbeats, and report complete/fail back to the
// server. There is no workflow authoring layer in the Go SDK — the
// Worker is the execution side only.

package acteon

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"sync"
	"time"
)

// TaskHandler executes one task. It receives the task's raw JSON
// payload and returns the result to report on completion (any
// JSON-serializable value, or nil). Returning an error fails the
// task as retryable; wrap the error with NonRetryable to mark the
// failure terminal.
type TaskHandler func(ctx context.Context, payload json.RawMessage) (any, error)

// nonRetryableError marks a handler failure as terminal. Built via
// NonRetryable, detected via errors.As so wrapping with fmt.Errorf
// ("%w") along the way still works.
type nonRetryableError struct {
	err error
}

func (e *nonRetryableError) Error() string {
	return e.err.Error()
}

func (e *nonRetryableError) Unwrap() error {
	return e.err
}

// NonRetryable wraps err to mark a task failure as terminal: the
// Worker reports it with retryable=false so the server fails the
// task immediately instead of re-queueing it with backoff.
// Returns nil when err is nil.
func NonRetryable(err error) error {
	if err == nil {
		return nil
	}
	return &nonRetryableError{err: err}
}

// IsNonRetryable reports whether err (or any error it wraps) was
// marked terminal via NonRetryable.
func IsNonRetryable(err error) bool {
	var nr *nonRetryableError
	return errors.As(err, &nr)
}

// WorkerConfig configures a Worker.
type WorkerConfig struct {
	// Namespace scopes every queue call. Required.
	Namespace string
	// Tenant scopes every queue call. Required.
	Tenant string
	// Queue is the queue to poll. Required.
	Queue string
	// WorkerID identifies this worker in poll requests (for
	// observability). Optional.
	WorkerID string
	// PollInterval is how long to wait between polls when the queue
	// is empty or a poll fails. Defaults to 1 second.
	PollInterval time.Duration
	// LeaseSeconds is the lease duration requested on poll and
	// re-requested on every heartbeat. Heartbeats fire every
	// LeaseSeconds/2 while a handler runs. Defaults to 60.
	LeaseSeconds int
	// MaxConcurrent bounds how many tasks run at once (and how many
	// tasks each poll leases). Defaults to 1.
	MaxConcurrent int
}

// Worker is a polling task-queue worker. Register handlers per
// action type, then call Run (long-running loop) or RunOnce (a
// single poll-and-drain pass, mainly for tests).
type Worker struct {
	client *Client
	cfg    WorkerConfig

	// heartbeatInterval overrides the LeaseSeconds/2 cadence when
	// non-zero (tests shrink it to keep slow-handler coverage fast).
	heartbeatInterval time.Duration

	mu       sync.RWMutex
	handlers map[string]TaskHandler
}

// NewWorker creates a Worker bound to client and cfg, applying the
// documented defaults for PollInterval, LeaseSeconds, and
// MaxConcurrent.
func NewWorker(client *Client, cfg WorkerConfig) *Worker {
	if cfg.PollInterval <= 0 {
		cfg.PollInterval = time.Second
	}
	if cfg.LeaseSeconds <= 0 {
		cfg.LeaseSeconds = 60
	}
	if cfg.MaxConcurrent <= 0 {
		cfg.MaxConcurrent = 1
	}
	return &Worker{
		client:   client,
		cfg:      cfg,
		handlers: make(map[string]TaskHandler),
	}
}

// Register installs handler for tasks whose ActionType equals
// actionType, replacing any previous handler for that type. Safe to
// call concurrently with a running Worker.
func (w *Worker) Register(actionType string, handler TaskHandler) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.handlers[actionType] = handler
}

// validate checks the required config fields. Run and RunOnce both
// refuse to poll with an unscoped or unnamed queue — the server
// would reject the calls anyway, but failing fast keeps the error
// out of the poll loop.
func (w *Worker) validate() error {
	if w.cfg.Namespace == "" || w.cfg.Tenant == "" || w.cfg.Queue == "" {
		return fmt.Errorf("acteon worker: namespace, tenant, and queue are required")
	}
	return nil
}

// Run polls the queue until ctx is cancelled, dispatching each
// leased task to its registered handler with at most
// MaxConcurrent tasks in flight. Poll errors do not stop the loop —
// the Worker waits PollInterval and retries, so transient server
// blips self-heal. Returns ctx.Err() once cancelled; in-flight
// handlers finish (and report their outcome) before Run returns.
func (w *Worker) Run(ctx context.Context) error {
	if err := w.validate(); err != nil {
		return err
	}
	for {
		n, err := w.RunOnce(ctx)
		if ctx.Err() != nil {
			return ctx.Err()
		}
		// Poll again immediately while the queue keeps yielding
		// work; back off by PollInterval when it's empty or the
		// poll failed.
		if err == nil && n > 0 {
			continue
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(w.cfg.PollInterval):
		}
	}
}

// RunOnce performs a single poll-and-drain pass: lease up to
// MaxConcurrent tasks, run them concurrently to completion, and
// report each outcome. Returns the number of tasks leased. A zero
// count with a nil error means the queue had nothing to lease.
// Mainly useful for tests and cron-style invocations.
func (w *Worker) RunOnce(ctx context.Context) (int, error) {
	if err := w.validate(); err != nil {
		return 0, err
	}
	maxTasks := w.cfg.MaxConcurrent
	leaseSeconds := w.cfg.LeaseSeconds
	tasks, err := w.client.PollTasks(ctx, w.cfg.Queue, &PollTasksRequest{
		Namespace:    w.cfg.Namespace,
		Tenant:       w.cfg.Tenant,
		MaxTasks:     &maxTasks,
		LeaseSeconds: &leaseSeconds,
		WorkerID:     w.cfg.WorkerID,
	})
	if err != nil {
		return 0, err
	}

	var wg sync.WaitGroup
	for i := range tasks {
		task := &tasks[i]
		wg.Add(1)
		go func() {
			defer wg.Done()
			w.processTask(ctx, task)
		}()
	}
	wg.Wait()
	return len(tasks), nil
}

// processTask runs one leased task end to end: heartbeat goroutine,
// handler dispatch, then a complete or fail report. Outcome reports
// use a cancellation-detached context so a handler that finished
// just as the Worker shuts down still lands its result; reporting
// errors are dropped — a lost report simply lets the lease expire
// and the server re-queue the task.
func (w *Worker) processTask(ctx context.Context, task *WorkerTask) {
	reportCtx := context.WithoutCancel(ctx)

	w.mu.RLock()
	handler, ok := w.handlers[task.ActionType]
	w.mu.RUnlock()
	if !ok {
		// No handler on this worker. Fail retryable so the task can
		// be re-leased — another worker on the same queue may carry
		// the handler.
		_, _ = w.client.FailTask(reportCtx, task.TaskID, &FailTaskRequest{
			Namespace:  w.cfg.Namespace,
			Tenant:     w.cfg.Tenant,
			LeaseToken: task.LeaseToken,
			Error:      fmt.Sprintf("no handler registered for action type %q", task.ActionType),
			Retryable:  true,
		})
		return
	}

	// Auto-heartbeat at half the lease duration while the handler
	// runs, so slow handlers keep their lease.
	heartbeatCtx, stopHeartbeat := context.WithCancel(ctx)
	heartbeatDone := make(chan struct{})
	go func() {
		defer close(heartbeatDone)
		w.heartbeatLoop(heartbeatCtx, task)
	}()

	result, err := handler(ctx, task.Payload)
	stopHeartbeat()
	<-heartbeatDone

	if err != nil {
		_, _ = w.client.FailTask(reportCtx, task.TaskID, &FailTaskRequest{
			Namespace:  w.cfg.Namespace,
			Tenant:     w.cfg.Tenant,
			LeaseToken: task.LeaseToken,
			Error:      err.Error(),
			Retryable:  !IsNonRetryable(err),
		})
		return
	}
	_, _ = w.client.CompleteTask(reportCtx, task.TaskID, &CompleteTaskRequest{
		Namespace:  w.cfg.Namespace,
		Tenant:     w.cfg.Tenant,
		LeaseToken: task.LeaseToken,
		Result:     result,
	})
}

// heartbeatLoop extends the task's lease every LeaseSeconds/2 (or
// the test override) until ctx is cancelled. Heartbeat errors are
// dropped: a lost lease surfaces as a conflict on the final
// complete/fail report, and the server re-queues the task.
func (w *Worker) heartbeatLoop(ctx context.Context, task *WorkerTask) {
	interval := w.heartbeatInterval
	if interval <= 0 {
		interval = time.Duration(w.cfg.LeaseSeconds) * time.Second / 2
	}
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			extendSeconds := w.cfg.LeaseSeconds
			_, _ = w.client.HeartbeatTask(ctx, task.TaskID, &HeartbeatTaskRequest{
				Namespace:     w.cfg.Namespace,
				Tenant:        w.cfg.Tenant,
				LeaseToken:    task.LeaseToken,
				ExtendSeconds: &extendSeconds,
			})
		}
	}
}
