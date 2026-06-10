package acteon

// Go SDK polling Worker tests.
//
// These tests drive the Worker against a stateful httptest mock of
// the `/v1/queues` surface. The contract under test: poll → handle
// → complete on success; handler errors fail with retryable=true;
// NonRetryable-wrapped errors fail with retryable=false; slow
// handlers get automatic heartbeats; Run honors ctx cancellation.

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"
)

// queueMockState records every worker-side report the mock queue
// server receives. All access goes through the mutex — the Worker
// hits the server from multiple goroutines.
type queueMockState struct {
	mu         sync.Mutex
	pollCount  int
	pollBodies []map[string]any
	heartbeats []map[string]any
	completes  []map[string]any
	fails      []map[string]any
}

func (s *queueMockState) snapshot() (heartbeats, completes, fails []map[string]any) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]map[string]any{}, s.heartbeats...),
		append([]map[string]any{}, s.completes...),
		append([]map[string]any{}, s.fails...)
}

// newQueueMockServer returns a mock queue server that serves `tasks`
// on the first poll and an empty batch on every poll after, and
// records heartbeat / complete / fail bodies. Caller must
// `defer teardown()`.
func newQueueMockServer(t *testing.T, tasks []map[string]any) (string, *queueMockState, func()) {
	t.Helper()
	state := &queueMockState{}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body map[string]any
		_ = json.NewDecoder(r.Body).Decode(&body)

		state.mu.Lock()
		defer state.mu.Unlock()
		w.Header().Set("Content-Type", "application/json")

		switch {
		case strings.HasSuffix(r.URL.Path, "/poll"):
			state.pollCount++
			state.pollBodies = append(state.pollBodies, body)
			batch := []map[string]any{}
			if state.pollCount == 1 {
				batch = tasks
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"tasks": batch})
		case strings.HasSuffix(r.URL.Path, "/heartbeat"):
			state.heartbeats = append(state.heartbeats, body)
			_ = json.NewEncoder(w).Encode(taskWire(map[string]any{"status": "leased"}))
		case strings.HasSuffix(r.URL.Path, "/complete"):
			state.completes = append(state.completes, body)
			_ = json.NewEncoder(w).Encode(taskWire(map[string]any{"status": "completed"}))
		case strings.HasSuffix(r.URL.Path, "/fail"):
			state.fails = append(state.fails, body)
			_ = json.NewEncoder(w).Encode(taskWire(map[string]any{"status": "failed"}))
		default:
			w.WriteHeader(http.StatusNotFound)
			_ = json.NewEncoder(w).Encode(map[string]any{"error": "not found"})
		}
	}))
	return srv.URL, state, srv.Close
}

// leasedTaskWire builds a leased wire-form task for the mock poll
// response.
func leasedTaskWire(taskID, actionType, leaseToken string, payload map[string]any) map[string]any {
	return taskWire(map[string]any{
		"task_id":          taskID,
		"action_type":      actionType,
		"payload":          payload,
		"status":           "leased",
		"attempt":          1,
		"lease_token":      leaseToken,
		"lease_expires_at": time.Now().UTC().Add(time.Minute).Format(time.RFC3339),
	})
}

func newTestWorker(serverURL string) *Worker {
	c := NewClient(serverURL, WithAPIKey("k"))
	return NewWorker(c, WorkerConfig{
		Namespace: "ns",
		Tenant:    "tnt",
		Queue:     "q",
		WorkerID:  "w-1",
	})
}

func TestWorkerConfigDefaults(t *testing.T) {
	w := NewWorker(NewClient("http://localhost"), WorkerConfig{
		Namespace: "ns",
		Tenant:    "tnt",
		Queue:     "q",
	})
	if w.cfg.PollInterval != time.Second {
		t.Errorf("PollInterval default: got %v", w.cfg.PollInterval)
	}
	if w.cfg.LeaseSeconds != 60 {
		t.Errorf("LeaseSeconds default: got %d", w.cfg.LeaseSeconds)
	}
	if w.cfg.MaxConcurrent != 1 {
		t.Errorf("MaxConcurrent default: got %d", w.cfg.MaxConcurrent)
	}
}

func TestWorkerRunOnceRequiresScope(t *testing.T) {
	w := NewWorker(NewClient("http://localhost"), WorkerConfig{})
	if _, err := w.RunOnce(context.Background()); err == nil {
		t.Fatalf("expected error for missing namespace/tenant/queue")
	}
}

func TestWorkerHappyPathCompletesWithHandlerResult(t *testing.T) {
	url, state, teardown := newQueueMockServer(t, []map[string]any{
		leasedTaskWire("t-1", "send_email", "lease-1", map[string]any{"to": "a@b.c"}),
	})
	defer teardown()

	w := newTestWorker(url)
	var gotPayload map[string]any
	w.Register("send_email", func(_ context.Context, payload json.RawMessage) (any, error) {
		if err := json.Unmarshal(payload, &gotPayload); err != nil {
			return nil, err
		}
		return map[string]any{"sent": true}, nil
	})

	n, err := w.RunOnce(context.Background())
	if err != nil {
		t.Fatalf("run once: %v", err)
	}
	if n != 1 {
		t.Errorf("tasks handled: got %d", n)
	}
	if gotPayload["to"] != "a@b.c" {
		t.Errorf("handler payload: got %v", gotPayload)
	}

	_, completes, fails := state.snapshot()
	if len(fails) != 0 {
		t.Errorf("unexpected fail reports: %v", fails)
	}
	if len(completes) != 1 {
		t.Fatalf("complete reports: got %d", len(completes))
	}
	complete := completes[0]
	if complete["lease_token"] != "lease-1" {
		t.Errorf("complete lease_token: got %v", complete["lease_token"])
	}
	result, ok := complete["result"].(map[string]any)
	if !ok || result["sent"] != true {
		t.Errorf("complete result: got %v", complete["result"])
	}

	// Poll body must carry the worker's scope + identity.
	state.mu.Lock()
	poll := state.pollBodies[0]
	state.mu.Unlock()
	if poll["namespace"] != "ns" || poll["tenant"] != "tnt" || poll["worker_id"] != "w-1" {
		t.Errorf("poll body: got %v", poll)
	}
}

func TestWorkerHandlerErrorFailsRetryable(t *testing.T) {
	url, state, teardown := newQueueMockServer(t, []map[string]any{
		leasedTaskWire("t-1", "send_email", "lease-1", map[string]any{}),
	})
	defer teardown()

	w := newTestWorker(url)
	w.Register("send_email", func(context.Context, json.RawMessage) (any, error) {
		return nil, errors.New("smtp timeout")
	})

	if _, err := w.RunOnce(context.Background()); err != nil {
		t.Fatalf("run once: %v", err)
	}

	_, completes, fails := state.snapshot()
	if len(completes) != 0 {
		t.Errorf("unexpected complete reports: %v", completes)
	}
	if len(fails) != 1 {
		t.Fatalf("fail reports: got %d", len(fails))
	}
	fail := fails[0]
	if fail["error"] != "smtp timeout" || fail["lease_token"] != "lease-1" {
		t.Errorf("fail body: got %v", fail)
	}
	if fail["retryable"] != true {
		t.Errorf("plain handler errors must fail retryable: got %v", fail["retryable"])
	}
}

func TestWorkerNonRetryableErrorFailsTerminal(t *testing.T) {
	url, state, teardown := newQueueMockServer(t, []map[string]any{
		leasedTaskWire("t-1", "send_email", "lease-1", map[string]any{}),
	})
	defer teardown()

	w := newTestWorker(url)
	w.Register("send_email", func(context.Context, json.RawMessage) (any, error) {
		// Wrapping with %w along the way must not hide the marker.
		return nil, fmt.Errorf("handler: %w", NonRetryable(errors.New("bad address")))
	})

	if _, err := w.RunOnce(context.Background()); err != nil {
		t.Fatalf("run once: %v", err)
	}

	_, _, fails := state.snapshot()
	if len(fails) != 1 {
		t.Fatalf("fail reports: got %d", len(fails))
	}
	if fails[0]["retryable"] != false {
		t.Errorf("NonRetryable errors must fail terminal: got %v", fails[0]["retryable"])
	}
	if !strings.Contains(fails[0]["error"].(string), "bad address") {
		t.Errorf("fail error: got %v", fails[0]["error"])
	}
}

func TestWorkerUnregisteredActionTypeFailsRetryable(t *testing.T) {
	url, state, teardown := newQueueMockServer(t, []map[string]any{
		leasedTaskWire("t-1", "unknown_type", "lease-1", map[string]any{}),
	})
	defer teardown()

	w := newTestWorker(url)
	w.Register("send_email", func(context.Context, json.RawMessage) (any, error) {
		return nil, nil
	})

	if _, err := w.RunOnce(context.Background()); err != nil {
		t.Fatalf("run once: %v", err)
	}

	_, _, fails := state.snapshot()
	if len(fails) != 1 {
		t.Fatalf("fail reports: got %d", len(fails))
	}
	if fails[0]["retryable"] != true {
		t.Errorf("missing-handler failures must stay retryable: got %v", fails[0]["retryable"])
	}
	if !strings.Contains(fails[0]["error"].(string), "unknown_type") {
		t.Errorf("fail error must name the action type: got %v", fails[0]["error"])
	}
}

func TestWorkerHeartbeatsSlowHandler(t *testing.T) {
	url, state, teardown := newQueueMockServer(t, []map[string]any{
		leasedTaskWire("t-1", "slow", "lease-1", map[string]any{}),
	})
	defer teardown()

	w := newTestWorker(url)
	// Shrink the heartbeat cadence so the test stays fast; the
	// production cadence is LeaseSeconds/2.
	w.heartbeatInterval = 20 * time.Millisecond
	w.Register("slow", func(ctx context.Context, _ json.RawMessage) (any, error) {
		select {
		case <-time.After(150 * time.Millisecond):
		case <-ctx.Done():
			return nil, ctx.Err()
		}
		return map[string]any{"ok": true}, nil
	})

	if _, err := w.RunOnce(context.Background()); err != nil {
		t.Fatalf("run once: %v", err)
	}

	heartbeats, completes, fails := state.snapshot()
	if len(fails) != 0 || len(completes) != 1 {
		t.Fatalf("reports: %d completes, %d fails", len(completes), len(fails))
	}
	if len(heartbeats) == 0 {
		t.Fatalf("expected at least one heartbeat for a slow handler")
	}
	hb := heartbeats[0]
	if hb["lease_token"] != "lease-1" {
		t.Errorf("heartbeat lease_token: got %v", hb["lease_token"])
	}
	// extend_seconds must re-request the configured lease (default 60).
	if hb["extend_seconds"] != float64(60) {
		t.Errorf("heartbeat extend_seconds: got %v", hb["extend_seconds"])
	}
}

func TestWorkerFastHandlerSendsNoHeartbeat(t *testing.T) {
	url, state, teardown := newQueueMockServer(t, []map[string]any{
		leasedTaskWire("t-1", "fast", "lease-1", map[string]any{}),
	})
	defer teardown()

	w := newTestWorker(url)
	w.Register("fast", func(context.Context, json.RawMessage) (any, error) {
		return nil, nil
	})

	if _, err := w.RunOnce(context.Background()); err != nil {
		t.Fatalf("run once: %v", err)
	}

	heartbeats, _, _ := state.snapshot()
	// LeaseSeconds defaults to 60 → first heartbeat at 30s; a fast
	// handler must finish well before the ticker ever fires.
	if len(heartbeats) != 0 {
		t.Errorf("unexpected heartbeats: %v", heartbeats)
	}
}

func TestWorkerRunHonorsContextCancellation(t *testing.T) {
	url, state, teardown := newQueueMockServer(t, []map[string]any{
		leasedTaskWire("t-1", "send_email", "lease-1", map[string]any{}),
	})
	defer teardown()

	w := newTestWorker(url)
	w.cfg.PollInterval = 10 * time.Millisecond
	w.Register("send_email", func(context.Context, json.RawMessage) (any, error) {
		return map[string]any{"sent": true}, nil
	})

	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- w.Run(ctx) }()

	// Let the loop drain the first batch and idle-poll a few times.
	time.Sleep(100 * time.Millisecond)
	cancel()

	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Errorf("Run must return ctx.Err(): got %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatalf("Run did not return after cancellation")
	}

	_, completes, _ := state.snapshot()
	if len(completes) != 1 {
		t.Errorf("complete reports: got %d", len(completes))
	}
	state.mu.Lock()
	polls := state.pollCount
	state.mu.Unlock()
	if polls < 2 {
		t.Errorf("Run must keep polling after an empty batch: got %d polls", polls)
	}
}

func TestNonRetryableNilStaysNil(t *testing.T) {
	if NonRetryable(nil) != nil {
		t.Errorf("NonRetryable(nil) must be nil")
	}
	if IsNonRetryable(errors.New("plain")) {
		t.Errorf("plain errors must not read as non-retryable")
	}
	if !IsNonRetryable(NonRetryable(errors.New("x"))) {
		t.Errorf("wrapped errors must read as non-retryable")
	}
}
