// Worker task-queue surface for the Go ActeonClient.
//
// External workers drive this API: they poll a named queue for
// tasks, execute them, and report results via complete/fail. Chain
// `worker` steps and workflow continuation tasks flow through the
// same queues. Method names match the existing Go SDK convention
// (exported PascalCase); wire payloads match the server's
// `/v1/queues` REST surface byte-for-byte.
//
// See `worker.go` for the higher-level polling `Worker` built on
// top of these primitives.

package acteon

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"time"
)

// Task lifecycle statuses reported in `WorkerTask.Status`.
const (
	// TaskStatusPending — enqueued, waiting to be leased.
	TaskStatusPending = "pending"
	// TaskStatusLeased — leased to a worker; the lease token gates
	// heartbeat / complete / fail.
	TaskStatusLeased = "leased"
	// TaskStatusCompleted — terminal success.
	TaskStatusCompleted = "completed"
	// TaskStatusFailed — terminal failure (non-retryable, or the
	// attempt budget is exhausted).
	TaskStatusFailed = "failed"
	// TaskStatusCancelled — cancelled before completion.
	TaskStatusCancelled = "cancelled"
)

// WorkerTask represents a task on a worker queue.
type WorkerTask struct {
	// TaskID is the unique task ID.
	TaskID string `json:"task_id"`
	// Queue is the queue the task is routed through.
	Queue string `json:"queue"`
	// ActionType drives worker handler dispatch.
	ActionType string `json:"action_type"`
	// Payload is the arbitrary JSON payload delivered to the worker.
	Payload json.RawMessage `json:"payload"`
	// Status is the lifecycle status (see the TaskStatus* constants).
	Status string `json:"status"`
	// Attempt is the delivery attempt (1-based once leased).
	Attempt int `json:"attempt"`
	// MaxAttempts is the maximum number of delivery attempts.
	MaxAttempts int `json:"max_attempts"`
	// LeaseToken is present in poll responses; required for
	// heartbeat / complete / fail.
	LeaseToken string `json:"lease_token,omitempty"`
	// LeaseExpiresAt is when the current lease expires.
	LeaseExpiresAt *time.Time `json:"lease_expires_at,omitempty"`
	// Result is the result reported by the worker on completion.
	Result json.RawMessage `json:"result,omitempty"`
	// Error is the error reported on failure.
	Error string `json:"error,omitempty"`
	// ChainID is the owning chain execution, for chain worker steps.
	ChainID string `json:"chain_id,omitempty"`
	// WorkflowExecutionID is the owning workflow execution, for
	// workflow continuation tasks.
	WorkflowExecutionID string `json:"workflow_execution_id,omitempty"`
	// CreatedAt is when the task was enqueued.
	CreatedAt time.Time `json:"created_at"`
	// UpdatedAt is when the task was last updated.
	UpdatedAt time.Time `json:"updated_at"`
}

// EnqueueTaskRequest is the request body for enqueueing a task.
type EnqueueTaskRequest struct {
	// Namespace scopes the task.
	Namespace string `json:"namespace"`
	// Tenant scopes the task.
	Tenant string `json:"tenant"`
	// ActionType drives worker handler dispatch.
	ActionType string `json:"action_type"`
	// Payload is the arbitrary JSON payload delivered to the worker.
	Payload any `json:"payload"`
	// MaxAttempts is the maximum number of delivery attempts
	// (server default 3 when nil).
	MaxAttempts *int `json:"max_attempts,omitempty"`
}

// PollTasksRequest is the request body for polling a queue.
type PollTasksRequest struct {
	// Namespace scopes the poll.
	Namespace string `json:"namespace"`
	// Tenant scopes the poll.
	Tenant string `json:"tenant"`
	// MaxTasks is the maximum number of tasks to lease in one poll
	// (server default 1 when nil).
	MaxTasks *int `json:"max_tasks,omitempty"`
	// LeaseSeconds is the lease duration in seconds (server default
	// 60, max 3600).
	LeaseSeconds *int `json:"lease_seconds,omitempty"`
	// WorkerID identifies the polling worker (for observability).
	WorkerID string `json:"worker_id,omitempty"`
}

// HeartbeatTaskRequest is the request body for extending a task lease.
type HeartbeatTaskRequest struct {
	// Namespace scopes the task.
	Namespace string `json:"namespace"`
	// Tenant scopes the task.
	Tenant string `json:"tenant"`
	// LeaseToken is the lease token returned by poll.
	LeaseToken string `json:"lease_token"`
	// ExtendSeconds is the new lease duration in seconds from now
	// (server default 60 when nil).
	ExtendSeconds *int `json:"extend_seconds,omitempty"`
}

// CompleteTaskRequest is the request body for completing a task.
type CompleteTaskRequest struct {
	// Namespace scopes the task.
	Namespace string `json:"namespace"`
	// Tenant scopes the task.
	Tenant string `json:"tenant"`
	// LeaseToken is the lease token returned by poll.
	LeaseToken string `json:"lease_token"`
	// Result is the task result. For chain worker steps this becomes
	// the step's response body; for workflow tasks it carries the
	// directive.
	Result any `json:"result"`
}

// FailTaskRequest is the request body for failing a task.
type FailTaskRequest struct {
	// Namespace scopes the task.
	Namespace string `json:"namespace"`
	// Tenant scopes the task.
	Tenant string `json:"tenant"`
	// LeaseToken is the lease token returned by poll.
	LeaseToken string `json:"lease_token"`
	// Error is the error message.
	Error string `json:"error"`
	// Retryable marks whether the failure is retryable. Retryable
	// failures within the attempt budget re-queue the task with
	// backoff; non-retryable failures are terminal.
	Retryable bool `json:"retryable"`
}

// pollTasksResponse is the wire envelope for poll and list replies.
type pollTasksResponse struct {
	Tasks []WorkerTask `json:"tasks"`
}

// queueSeg percent-encodes a single path segment so reserved
// characters like `/` don't slip into the URL grammar. Mirrors
// `busSeg` / `a2aSeg` — queue names and task ids are opaque strings.
func queueSeg(s string) string {
	return url.PathEscape(s)
}

// queueDoJSON is a thin helper around `doRequest` that reads the
// response body and surfaces structured Acteon errors as `*APIError`
// (or falls back to `*HTTPError` when the body isn't structured).
// Successful responses get unmarshalled into `out` (when non-nil).
// Mirrors `busDoJSON`, but marks transient HTTP statuses retryable —
// workers lean on `IsRetryable` to keep polling through blips.
func (c *Client) queueDoJSON(
	ctx context.Context,
	method, path string,
	body, out any,
) (*http.Response, error) {
	resp, err := c.doRequest(ctx, method, path, body)
	if err != nil {
		return nil, err
	}
	respBody, readErr := io.ReadAll(resp.Body)
	resp.Body.Close()
	if readErr != nil {
		return resp, &ConnectionError{Message: readErr.Error()}
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		var errResp struct {
			Code    string `json:"code"`
			Message string `json:"message"`
			Error   string `json:"error"`
		}
		if jsonErr := json.Unmarshal(respBody, &errResp); jsonErr == nil {
			msg := errResp.Error
			if msg == "" {
				msg = errResp.Message
			}
			if msg == "" {
				msg = fmt.Sprintf("queue error (status %d)", resp.StatusCode)
			}
			code := errResp.Code
			if code == "" {
				code = "QUEUE"
			}
			retryable := resp.StatusCode == http.StatusRequestTimeout ||
				resp.StatusCode == http.StatusTooManyRequests ||
				resp.StatusCode >= 500
			return resp, &APIError{Code: code, Message: msg, Retryable: retryable}
		}
		return resp, &HTTPError{Status: resp.StatusCode, Message: string(respBody)}
	}
	if out != nil && len(respBody) > 0 {
		if err := json.Unmarshal(respBody, out); err != nil {
			return resp, &ConnectionError{Message: err.Error()}
		}
	}
	return resp, nil
}

// EnqueueTask calls `POST /v1/queues/{queue}/tasks` to enqueue a
// task onto a worker queue. Returns the created task (status
// `pending`).
func (c *Client) EnqueueTask(ctx context.Context, queue string, req *EnqueueTaskRequest) (*WorkerTask, error) {
	path := fmt.Sprintf("/v1/queues/%s/tasks", queueSeg(queue))
	var out WorkerTask
	if _, err := c.queueDoJSON(ctx, http.MethodPost, path, req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// PollTasks calls `POST /v1/queues/{queue}/poll` to lease up to
// `req.MaxTasks` tasks from a queue. Returns the leased tasks —
// empty (not an error) when the queue has no leasable tasks. Each
// returned task carries the `LeaseToken` required for heartbeat /
// complete / fail.
func (c *Client) PollTasks(ctx context.Context, queue string, req *PollTasksRequest) ([]WorkerTask, error) {
	path := fmt.Sprintf("/v1/queues/%s/poll", queueSeg(queue))
	var out pollTasksResponse
	if _, err := c.queueDoJSON(ctx, http.MethodPost, path, req, &out); err != nil {
		return nil, err
	}
	return out.Tasks, nil
}

// HeartbeatTask calls `POST /v1/queues/tasks/{taskID}/heartbeat` to
// extend a leased task's lease. Returns the updated task with the
// new `LeaseExpiresAt`.
func (c *Client) HeartbeatTask(ctx context.Context, taskID string, req *HeartbeatTaskRequest) (*WorkerTask, error) {
	path := fmt.Sprintf("/v1/queues/tasks/%s/heartbeat", queueSeg(taskID))
	var out WorkerTask
	if _, err := c.queueDoJSON(ctx, http.MethodPost, path, req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// CompleteTask calls `POST /v1/queues/tasks/{taskID}/complete` to
// report a leased task as successfully completed with a result.
func (c *Client) CompleteTask(ctx context.Context, taskID string, req *CompleteTaskRequest) (*WorkerTask, error) {
	path := fmt.Sprintf("/v1/queues/tasks/%s/complete", queueSeg(taskID))
	var out WorkerTask
	if _, err := c.queueDoJSON(ctx, http.MethodPost, path, req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// FailTask calls `POST /v1/queues/tasks/{taskID}/fail` to report a
// leased task as failed. Retryable failures within the attempt
// budget re-queue the task with backoff; non-retryable failures are
// terminal.
func (c *Client) FailTask(ctx context.Context, taskID string, req *FailTaskRequest) (*WorkerTask, error) {
	path := fmt.Sprintf("/v1/queues/tasks/%s/fail", queueSeg(taskID))
	var out WorkerTask
	if _, err := c.queueDoJSON(ctx, http.MethodPost, path, req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// GetTask calls `GET /v1/queues/tasks/{taskID}` to fetch a single
// task. Returns (nil, nil) when the task does not exist, matching
// the GetRecurring convention.
func (c *Client) GetTask(ctx context.Context, taskID, namespace, tenant string) (*WorkerTask, error) {
	params := url.Values{}
	params.Set("namespace", namespace)
	params.Set("tenant", tenant)
	path := fmt.Sprintf("/v1/queues/tasks/%s?%s", queueSeg(taskID), params.Encode())

	var out WorkerTask
	resp, err := c.queueDoJSON(ctx, http.MethodGet, path, nil, &out)
	if resp != nil && resp.StatusCode == http.StatusNotFound {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &out, nil
}

// ListTasks calls `GET /v1/queues/{queue}/tasks` to list a queue's
// tasks. `status` optionally filters by lifecycle status (see the
// TaskStatus* constants); pass "" for all statuses.
func (c *Client) ListTasks(ctx context.Context, queue, namespace, tenant, status string) ([]WorkerTask, error) {
	params := url.Values{}
	params.Set("namespace", namespace)
	params.Set("tenant", tenant)
	if status != "" {
		params.Set("status", status)
	}
	path := fmt.Sprintf("/v1/queues/%s/tasks?%s", queueSeg(queue), params.Encode())

	var out pollTasksResponse
	if _, err := c.queueDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return out.Tasks, nil
}
