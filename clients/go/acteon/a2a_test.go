package acteon

// A2A Go SDK — factory + URL/header smoke tests.
//
// Live HTTP tests would need a running Acteon instance with A2A
// enabled; these tests exercise the wire surface of the new
// `a2a.go` module via an `httptest.Server`. The contract under
// test: factories produce the dict shapes the server expects, URLs
// are spec-correct, and the `A2A-Version` header lands on every
// authenticated call.

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// ----------------------------------------------------------------------
// Factory helpers
// ----------------------------------------------------------------------

func TestMakePartText(t *testing.T) {
	got := MakePartText("hi")
	if got["text"] != "hi" {
		t.Errorf("text: got %v", got)
	}
}

func TestMakePartURL(t *testing.T) {
	got := MakePartURL("https://x/y")
	if got["url"] != "https://x/y" {
		t.Errorf("url: got %v", got)
	}
}

func TestMakePartDataDefaultsToJSON(t *testing.T) {
	got := MakePartData(map[string]any{"k": 1}, "")
	if got["mediaType"] != "application/json" {
		t.Errorf("mediaType default: got %v", got["mediaType"])
	}
}

func TestMakePartDataHonorsCustomMediaType(t *testing.T) {
	got := MakePartData(nil, "application/cloudevents+json")
	if got["mediaType"] != "application/cloudevents+json" {
		t.Errorf("mediaType: got %v", got["mediaType"])
	}
}

func TestMakeMessageMinimalOmitsTaskIDAndContextID(t *testing.T) {
	got := MakeMessage("m-1", "user", []map[string]any{MakePartText("hi")}, MakeMessageOptions{})
	if got["messageId"] != "m-1" || got["role"] != "user" {
		t.Errorf("identity fields: got %v", got)
	}
	// Absent vs. empty matters server-side; the helper must NOT
	// populate either key when omitted.
	if _, ok := got["taskId"]; ok {
		t.Errorf("taskId must be absent when not set: got %v", got)
	}
	if _, ok := got["contextId"]; ok {
		t.Errorf("contextId must be absent when not set: got %v", got)
	}
}

func TestMakeMessageThreadsTaskID(t *testing.T) {
	got := MakeMessage(
		"m-2", "user",
		[]map[string]any{MakePartText("yes")},
		MakeMessageOptions{TaskID: "task-alpha"},
	)
	if got["taskId"] != "task-alpha" {
		t.Errorf("taskId: got %v", got["taskId"])
	}
}

func TestMakePushConfigMinimal(t *testing.T) {
	got := MakePushConfig("https://hook/x", MakePushConfigOptions{})
	if len(got) != 1 || got["url"] != "https://hook/x" {
		t.Errorf("minimal: got %v", got)
	}
}

func TestMakePushConfigFull(t *testing.T) {
	got := MakePushConfig("https://hook/x", MakePushConfigOptions{
		ID:             "cfg-1",
		Token:          "t",
		Authentication: map[string]any{"schemes": []string{"api-key"}},
	})
	if got["id"] != "cfg-1" || got["token"] != "t" {
		t.Errorf("full: got %v", got)
	}
	if _, ok := got["authentication"]; !ok {
		t.Errorf("authentication must be present: got %v", got)
	}
}

// ----------------------------------------------------------------------
// Client URLs + headers via httptest.Server
// ----------------------------------------------------------------------

// capturedRequest records the bits of an inbound request the tests
// assert on (path, method, headers, body).
type capturedRequest struct {
	method  string
	path    string
	headers http.Header
	body    []byte
}

// newCapturingServer returns an httptest.Server that records every
// inbound request and responds with the supplied status + body.
// Returns the URL, a pointer to the latest captured request, and a
// teardown function (caller must `defer teardown()`).
//
// The captured `path` is the **raw** request path — preserves
// percent-escapes so a test can prove the client encoded reserved
// characters before sending. `r.URL.Path` would have decoded them
// back to the unescaped form.
func newCapturingServer(t *testing.T, status int, body any) (string, *capturedRequest, func()) {
	t.Helper()
	captured := &capturedRequest{}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		captured.method = r.Method
		// Prefer RawPath when set (the request carried
		// percent-escapes); fall back to Path otherwise.
		if r.URL.RawPath != "" {
			captured.path = r.URL.RawPath
		} else {
			captured.path = r.URL.Path
		}
		captured.headers = r.Header.Clone()
		b, _ := io.ReadAll(r.Body)
		captured.body = b
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		if body != nil {
			_ = json.NewEncoder(w).Encode(body)
		}
	}))
	return srv.URL, captured, srv.Close
}

func TestA2ASendMessageURLAndA2AVersionHeader(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, map[string]any{
		"id":     "task-1",
		"status": map[string]any{"state": "submitted"},
	})
	defer teardown()
	c := NewClient(url, WithAPIKey("k"))
	msg := MakeMessage("m-1", "user", []map[string]any{MakePartText("hi")}, MakeMessageOptions{})
	if _, err := c.A2ASendMessage(context.Background(), "ns", "tnt", msg); err != nil {
		t.Fatalf("send: %v", err)
	}
	if captured.method != "POST" {
		t.Errorf("method: got %s", captured.method)
	}
	if captured.path != "/a2a/ns/tnt/v1/message:send" {
		t.Errorf("path: got %s", captured.path)
	}
	if got := captured.headers.Get(A2AVersionHeader); got != A2AProtocolVersion {
		t.Errorf("A2A-Version header: got %q want %q", got, A2AProtocolVersion)
	}
	if got := captured.headers.Get("Authorization"); got != "Bearer k" {
		t.Errorf("Authorization: got %q", got)
	}
	// Body must wrap message in {"message": ...} per spec.
	var body map[string]any
	if err := json.Unmarshal(captured.body, &body); err != nil {
		t.Fatalf("body unmarshal: %v", err)
	}
	if _, ok := body["message"]; !ok {
		t.Errorf("body must wrap message: got %v", body)
	}
}

func TestA2ACancelTaskKeepsCancelVerbInSegment(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, map[string]any{
		"id":     "task-1",
		"status": map[string]any{"state": "canceled"},
	})
	defer teardown()
	c := NewClient(url)
	if _, err := c.A2ACancelTask(context.Background(), "ns", "tnt", "task-1"); err != nil {
		t.Fatalf("cancel: %v", err)
	}
	if captured.path != "/a2a/ns/tnt/v1/tasks/task-1:cancel" {
		t.Errorf("path: got %s", captured.path)
	}
	if captured.method != "POST" {
		t.Errorf("method: got %s", captured.method)
	}
}

func TestA2ADeletePushConfigURL(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, map[string]any{})
	defer teardown()
	c := NewClient(url)
	if err := c.A2ADeletePushConfig(context.Background(), "ns", "tnt", "task-1", "cfg-a"); err != nil {
		t.Fatalf("delete: %v", err)
	}
	want := "/a2a/ns/tnt/v1/tasks/task-1/pushNotificationConfigs/cfg-a"
	if captured.path != want {
		t.Errorf("path: got %s want %s", captured.path, want)
	}
	if captured.method != "DELETE" {
		t.Errorf("method: got %s", captured.method)
	}
}

func TestA2ADiscoverAgentIsUnauthenticated(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, map[string]any{
		"agent_id": "tenant",
	})
	defer teardown()
	// Configure an API key on the client — discovery must still go
	// out anonymous (A2A spec).
	c := NewClient(url, WithAPIKey("k"))
	if _, err := c.A2ADiscoverAgent(context.Background(), "ns", "tnt"); err != nil {
		t.Fatalf("discover: %v", err)
	}
	if captured.path != "/a2a/ns/tnt/.well-known/agent.json" {
		t.Errorf("path: got %s", captured.path)
	}
	if got := captured.headers.Get("Authorization"); got != "" {
		t.Errorf("Authorization must be absent on discovery: got %q", got)
	}
}

func TestA2AGetAuthenticatedExtendedCardUsesJSONRPCEnvelope(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, map[string]any{
		"jsonrpc": "2.0",
		"id":      1,
		"result":  map[string]any{"agent_id": "tenant", "capabilities": map[string]any{}},
	})
	defer teardown()
	c := NewClient(url, WithAPIKey("k"))
	card, err := c.A2AGetAuthenticatedExtendedCard(context.Background(), "ns", "tnt")
	if err != nil {
		t.Fatalf("extended card: %v", err)
	}
	if captured.path != "/a2a/ns/tnt" {
		t.Errorf("path: got %s", captured.path)
	}
	var body map[string]any
	if err := json.Unmarshal(captured.body, &body); err != nil {
		t.Fatalf("body unmarshal: %v", err)
	}
	if body["method"] != "agent/getAuthenticatedExtendedCard" {
		t.Errorf("method field: got %v", body["method"])
	}
	// The mixin unwraps the JSON-RPC envelope on the way out.
	if card["agent_id"] != "tenant" {
		t.Errorf("unwrapped result: got %v", card)
	}
}

func TestA2AGetAuthenticatedExtendedCardJSONRPCErrorSurfacesAsAPIError(t *testing.T) {
	url, _, teardown := newCapturingServer(t, 200, map[string]any{
		"jsonrpc": "2.0",
		"id":      1,
		"error":   map[string]any{"code": -32001, "message": "task not found"},
	})
	defer teardown()
	c := NewClient(url, WithAPIKey("k"))
	_, err := c.A2AGetAuthenticatedExtendedCard(context.Background(), "ns", "tnt")
	if err == nil {
		t.Fatalf("expected error on JSON-RPC error envelope")
	}
	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError, got %T: %v", err, err)
	}
	if !strings.Contains(apiErr.Message, "task not found") {
		t.Errorf("message: got %q", apiErr.Message)
	}
}

func TestA2APathSegmentsArePercentEncoded(t *testing.T) {
	url, captured, teardown := newCapturingServer(t, 200, map[string]any{})
	defer teardown()
	c := NewClient(url)
	// A tenant id with a slash must be percent-encoded so it
	// cannot leak into additional path components.
	if _, err := c.A2AGetTask(context.Background(), "ns/escape", "tnt", "t"); err != nil {
		t.Fatalf("get_task: %v", err)
	}
	if !strings.Contains(captured.path, "/ns%2Fescape/") {
		t.Errorf("path must percent-encode slash: got %s", captured.path)
	}
}
