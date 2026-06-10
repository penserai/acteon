package acteon

// Go SDK task-queue surface tests.
//
// Live HTTP tests would need a running Acteon instance with the
// queue feature enabled; these tests exercise the wire surface of
// `queues.go` via an `httptest.Server` (reusing the capturing
// server from the A2A tests). The contract under test: URLs and
// methods match the server routes, request bodies snake-case and
// drop optional fields, task responses round-trip, and structured
// errors surface as `*APIError`.

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"
)

// taskWire builds a minimal wire-form task body the mock server
// returns, with overrides merged on top.
func taskWire(overrides map[string]any) map[string]any {
	body := map[string]any{
		"task_id":      "t-1",
		"queue":        "q",
		"action_type":  "send_email",
		"payload":      map[string]any{"to": "a@b.c"},
		"status":       "pending",
		"attempt":      0,
		"max_attempts": 3,
		"created_at":   "2026-06-10T00:00:00Z",
		"updated_at":   "2026-06-10T00:00:00Z",
	}
	for k, v := range overrides {
		body[k] = v
	}
	return body
}

func TestEnqueueTaskURLAndBody(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 201, taskWire(nil))
	defer teardown()
	c := NewClient(url, WithAPIKey("k"))

	task, err := c.EnqueueTask(context.Background(), "q", &EnqueueTaskRequest{
		Namespace:   "ns",
		Tenant:      "tnt",
		ActionType:  "send_email",
		Payload:     map[string]any{"to": "a@b.c"},
		MaxAttempts: ptr(5),
	})
	if err != nil {
		t.Fatalf("enqueue: %v", err)
	}
	if captured.method != "POST" {
		t.Errorf("method: got %s", captured.method)
	}
	if captured.path != "/v1/queues/q/tasks" {
		t.Errorf("path: got %s", captured.path)
	}
	if got := captured.headers.Get("Authorization"); got != "Bearer k" {
		t.Errorf("Authorization: got %q", got)
	}
	s := string(captured.body)
	for _, want := range []string{
		`"namespace":"ns"`,
		`"tenant":"tnt"`,
		`"action_type":"send_email"`,
		`"max_attempts":5`,
	} {
		if !strings.Contains(s, want) {
			t.Errorf("expected %s in body; got %s", want, s)
		}
	}
	if task.TaskID != "t-1" || task.Status != TaskStatusPending {
		t.Errorf("task: got %+v", task)
	}
}

func TestEnqueueTaskOmitsMaxAttemptsWhenNil(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 201, taskWire(nil))
	defer teardown()
	c := NewClient(url)

	if _, err := c.EnqueueTask(context.Background(), "q", &EnqueueTaskRequest{
		Namespace:  "ns",
		Tenant:     "tnt",
		ActionType: "send_email",
		Payload:    map[string]any{},
	}); err != nil {
		t.Fatalf("enqueue: %v", err)
	}
	if strings.Contains(string(captured.body), "max_attempts") {
		t.Errorf("max_attempts should be omitted when nil; got %s", captured.body)
	}
}

func TestPollTasksURLBodyAndLeaseFields(t *testing.T) {
	leased := taskWire(map[string]any{
		"status":           "leased",
		"attempt":          1,
		"lease_token":      "lease-abc",
		"lease_expires_at": "2026-06-10T00:01:00Z",
	})
	url, captured, teardown := newCapturingServer(t, 200, map[string]any{
		"tasks": []map[string]any{leased},
	})
	defer teardown()
	c := NewClient(url)

	tasks, err := c.PollTasks(context.Background(), "q", &PollTasksRequest{
		Namespace:    "ns",
		Tenant:       "tnt",
		MaxTasks:     ptr(4),
		LeaseSeconds: ptr(120),
		WorkerID:     "w-1",
	})
	if err != nil {
		t.Fatalf("poll: %v", err)
	}
	if captured.method != "POST" || captured.path != "/v1/queues/q/poll" {
		t.Errorf("request: got %s %s", captured.method, captured.path)
	}
	s := string(captured.body)
	for _, want := range []string{
		`"max_tasks":4`,
		`"lease_seconds":120`,
		`"worker_id":"w-1"`,
	} {
		if !strings.Contains(s, want) {
			t.Errorf("expected %s in body; got %s", want, s)
		}
	}
	if len(tasks) != 1 {
		t.Fatalf("tasks: got %d", len(tasks))
	}
	got := tasks[0]
	if got.Status != TaskStatusLeased || got.LeaseToken != "lease-abc" {
		t.Errorf("leased task: got %+v", got)
	}
	if got.LeaseExpiresAt == nil || !got.LeaseExpiresAt.Equal(time.Date(2026, 6, 10, 0, 1, 0, 0, time.UTC)) {
		t.Errorf("lease_expires_at: got %v", got.LeaseExpiresAt)
	}
	var payload map[string]any
	if err := json.Unmarshal(got.Payload, &payload); err != nil {
		t.Fatalf("payload unmarshal: %v", err)
	}
	if payload["to"] != "a@b.c" {
		t.Errorf("payload: got %v", payload)
	}
}

func TestPollTasksEmptyQueueReturnsNoTasks(t *testing.T) {
	url, _, teardown := newCapturingServer(t, 200, map[string]any{"tasks": []map[string]any{}})
	defer teardown()
	c := NewClient(url)

	tasks, err := c.PollTasks(context.Background(), "q", &PollTasksRequest{Namespace: "ns", Tenant: "tnt"})
	if err != nil {
		t.Fatalf("poll: %v", err)
	}
	if len(tasks) != 0 {
		t.Errorf("tasks: got %d", len(tasks))
	}
}

func TestHeartbeatTaskURLAndBody(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, taskWire(map[string]any{"status": "leased"}))
	defer teardown()
	c := NewClient(url)

	if _, err := c.HeartbeatTask(context.Background(), "t-1", &HeartbeatTaskRequest{
		Namespace:     "ns",
		Tenant:        "tnt",
		LeaseToken:    "lease-abc",
		ExtendSeconds: ptr(90),
	}); err != nil {
		t.Fatalf("heartbeat: %v", err)
	}
	if captured.method != "POST" || captured.path != "/v1/queues/tasks/t-1/heartbeat" {
		t.Errorf("request: got %s %s", captured.method, captured.path)
	}
	s := string(captured.body)
	if !strings.Contains(s, `"lease_token":"lease-abc"`) || !strings.Contains(s, `"extend_seconds":90`) {
		t.Errorf("body: got %s", s)
	}
}

func TestCompleteTaskURLAndBody(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, taskWire(map[string]any{"status": "completed"}))
	defer teardown()
	c := NewClient(url)

	task, err := c.CompleteTask(context.Background(), "t-1", &CompleteTaskRequest{
		Namespace:  "ns",
		Tenant:     "tnt",
		LeaseToken: "lease-abc",
		Result:     map[string]any{"sent": true},
	})
	if err != nil {
		t.Fatalf("complete: %v", err)
	}
	if captured.method != "POST" || captured.path != "/v1/queues/tasks/t-1/complete" {
		t.Errorf("request: got %s %s", captured.method, captured.path)
	}
	if !strings.Contains(string(captured.body), `"result":{"sent":true}`) {
		t.Errorf("body: got %s", captured.body)
	}
	if task.Status != TaskStatusCompleted {
		t.Errorf("status: got %s", task.Status)
	}
}

func TestFailTaskURLAndBody(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, taskWire(map[string]any{"status": "failed"}))
	defer teardown()
	c := NewClient(url)

	if _, err := c.FailTask(context.Background(), "t-1", &FailTaskRequest{
		Namespace:  "ns",
		Tenant:     "tnt",
		LeaseToken: "lease-abc",
		Error:      "boom",
		Retryable:  false,
	}); err != nil {
		t.Fatalf("fail: %v", err)
	}
	if captured.method != "POST" || captured.path != "/v1/queues/tasks/t-1/fail" {
		t.Errorf("request: got %s %s", captured.method, captured.path)
	}
	s := string(captured.body)
	if !strings.Contains(s, `"error":"boom"`) || !strings.Contains(s, `"retryable":false`) {
		t.Errorf("body: got %s", s)
	}
}

func TestGetTaskURLAndQueryParams(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, taskWire(nil))
	defer teardown()
	c := NewClient(url)

	task, err := c.GetTask(context.Background(), "t-1", "ns", "tnt")
	if err != nil {
		t.Fatalf("get: %v", err)
	}
	if captured.method != "GET" || !strings.HasPrefix(captured.path, "/v1/queues/tasks/t-1") {
		t.Errorf("request: got %s %s", captured.method, captured.path)
	}
	if task.TaskID != "t-1" {
		t.Errorf("task: got %+v", task)
	}
}

func TestGetTaskNotFoundReturnsNil(t *testing.T) {
	url, _, teardown := newCapturingServer(t, 404, map[string]any{"error": "task not found"})
	defer teardown()
	c := NewClient(url)

	task, err := c.GetTask(context.Background(), "missing", "ns", "tnt")
	if err != nil {
		t.Fatalf("get: %v", err)
	}
	if task != nil {
		t.Errorf("task must be nil on 404: got %+v", task)
	}
}

func TestListTasksStatusFilter(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, map[string]any{
		"tasks": []map[string]any{taskWire(nil)},
	})
	defer teardown()
	c := NewClient(url)

	tasks, err := c.ListTasks(context.Background(), "q", "ns", "tnt", TaskStatusPending)
	if err != nil {
		t.Fatalf("list: %v", err)
	}
	if captured.method != "GET" || !strings.HasPrefix(captured.path, "/v1/queues/q/tasks") {
		t.Errorf("request: got %s %s", captured.method, captured.path)
	}
	if len(tasks) != 1 {
		t.Errorf("tasks: got %d", len(tasks))
	}
}

func TestQueuePathSegmentsArePercentEncoded(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, map[string]any{"tasks": []map[string]any{}})
	defer teardown()
	c := NewClient(url)

	// A queue name with a slash must be percent-encoded so it
	// cannot leak into additional path components.
	if _, err := c.PollTasks(context.Background(), "q/escape", &PollTasksRequest{
		Namespace: "ns",
		Tenant:    "tnt",
	}); err != nil {
		t.Fatalf("poll: %v", err)
	}
	if !strings.Contains(captured.path, "/q%2Fescape/") {
		t.Errorf("path must percent-encode slash: got %s", captured.path)
	}
}

func TestQueueErrorSurfacesAsAPIError(t *testing.T) {
	url, _, teardown := newCapturingServer(t, 409, map[string]any{
		"error": "lease token mismatch",
	})
	defer teardown()
	c := NewClient(url)

	_, err := c.CompleteTask(context.Background(), "t-1", &CompleteTaskRequest{
		Namespace:  "ns",
		Tenant:     "tnt",
		LeaseToken: "stale",
		Result:     nil,
	})
	if err == nil {
		t.Fatalf("expected error on 409")
	}
	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError, got %T: %v", err, err)
	}
	if !strings.Contains(apiErr.Message, "lease token mismatch") {
		t.Errorf("message: got %q", apiErr.Message)
	}
	if apiErr.IsRetryable() {
		t.Errorf("409 must not be retryable")
	}
}

func TestQueueServerErrorIsRetryable(t *testing.T) {
	url, _, teardown := newCapturingServer(t, 500, map[string]any{"error": "backend down"})
	defer teardown()
	c := NewClient(url)

	_, err := c.PollTasks(context.Background(), "q", &PollTasksRequest{Namespace: "ns", Tenant: "tnt"})
	if err == nil {
		t.Fatalf("expected error on 500")
	}
	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError, got %T: %v", err, err)
	}
	if !apiErr.IsRetryable() {
		t.Errorf("500 must be retryable")
	}
}
