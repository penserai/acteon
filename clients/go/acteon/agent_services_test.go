package acteon

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
)

func TestAgentServiceReceiptsPreserveHeadersAndIdentity(t *testing.T) {
	var fixture struct {
		Jobs []struct {
			Source string         `json:"source_context"`
			Task   map[string]any `json:"task"`
			Stop   map[string]any `json:"stop_response"`
		} `json:"jobs"`
	}
	data, _ := os.ReadFile("../../contract-fixtures/agent-services.json")
	if err := json.Unmarshal(data, &fixture); err != nil {
		t.Fatal(err)
	}
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		if r.Method == "GET" && strings.HasSuffix(r.URL.Path, "/peers") {
			if r.URL.Query().Get("skill") != "diagnose" {
				t.Errorf("unexpected discovery query: %s", r.URL.RawQuery)
			}
			if r.Header.Get(AgentSourceContextHeader) != "" || r.Header.Get(AgentExecutionContextHeader) != "" || r.Header.Get("x-acteon-execution-permits") != "" {
				t.Error("authority fields leaked into peer discovery")
			}
			w.Header().Set(A2AVersionHeader, A2AProtocolVersion)
			json.NewEncoder(w).Encode(map[string]any{"peers": []any{map[string]any{
				"agent_id": "resolver", "skill": "diagnose", "description_untrusted": "Investigates incidents",
				"card_version": "v1", "binding_digest": strings.Repeat("a", 64), "checked_at_ms": 42,
			}}})
			return
		}
		if r.Header.Get("Authorization") != "Bearer caller-key" || r.Header.Get(A2AVersionHeader) != "1.0" {
			t.Error("original authentication/version missing")
		}
		index := 0
		if r.Method == "POST" && !strings.HasSuffix(r.URL.Path, "/stop") {
			var body struct {
				Message map[string]any `json:"message"`
			}
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Error(err)
			}
			if body.Message["messageId"] == "m2" {
				index = 1
			}
			if r.Header.Get(AgentSourceContextHeader) != "" {
				t.Error("source context leaked to admission")
			}
		} else {
			if strings.HasSuffix(r.URL.Path, "job-2") || strings.HasSuffix(r.URL.Path, "job-2/stop") {
				index = 1
			}
			if r.Header.Get(AgentSourceContextHeader) != fixture.Jobs[index].Source {
				t.Error("job contexts mixed")
			}
		}
		w.Header().Set(A2AVersionHeader, "1.0")
		w.Header().Set(AgentSourceContextHeader, fixture.Jobs[index].Source)
		if strings.HasSuffix(r.URL.Path, "/stop") {
			json.NewEncoder(w).Encode(fixture.Jobs[index].Stop)
		} else {
			json.NewEncoder(w).Encode(fixture.Jobs[index].Task)
		}
	}))
	defer server.Close()
	client := NewClient(server.URL, WithAPIKey("caller-key"))
	receipts := make([]*AgentServiceReceipt, 2)
	for i, id := range []string{"m1", "m2"} {
		var err error
		receipts[i], err = client.AgentServiceSendMessage(context.Background(), "prod", "acme", "notifier", map[string]any{"messageId": id})
		if err != nil {
			t.Fatal(err)
		}
	}
	receipts[0].Task["id"] = "tampered-model-id"
	saved, _ := json.Marshal(receipts[0])
	var restored AgentServiceReceipt
	if err := json.Unmarshal(saved, &restored); err != nil {
		t.Fatal(err)
	}
	receipts[0] = &restored
	var jobs sync.WaitGroup
	for i, receipt := range receipts {
		jobs.Add(1)
		go func(i int, receipt *AgentServiceReceipt) {
			defer jobs.Done()
			task, err := client.AgentServiceGetTask(context.Background(), receipt)
			if err != nil {
				t.Error(err)
				return
			}
			if task["id"] != fixture.Jobs[i].Task["id"] {
				t.Error("task identity changed")
			}
		}(i, receipt)
	}
	jobs.Wait()
	for i, receipt := range receipts {
		stopped, err := client.AgentServiceStopTask(context.Background(), receipt)
		if err != nil {
			t.Fatal(err)
		}
		if !stopped.FutureStartsBlocked || stopped.Task["id"] != receipt.TaskID {
			t.Fatal("stop mixed original jobs")
		}
		expected := []string{"restricted_only", "uncertain"}[i]
		if stopped.ProviderAbort == nil || stopped.ProviderAbort.State != expected {
			t.Fatalf("provider abort status mismatch: %#v", stopped.ProviderAbort)
		}
	}
	if calls.Load() != 6 {
		t.Fatalf("unexpected request count: %d", calls.Load())
	}
}

func TestAgentServiceParentCarriesContextAndPermits(t *testing.T) {
	var fixture struct {
		Parent struct {
			ExecutionContext string            `json:"execution_context"`
			Permits          []PermitReference `json:"permits"`
		} `json:"parent"`
		Jobs []struct {
			Source string         `json:"source_context"`
			Task   map[string]any `json:"task"`
		} `json:"jobs"`
	}
	data, _ := os.ReadFile("../../contract-fixtures/agent-services.json")
	if err := json.Unmarshal(data, &fixture); err != nil {
		t.Fatal(err)
	}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get(AgentExecutionContextHeader) != fixture.Parent.ExecutionContext {
			t.Error("parent context missing")
		}
		var permits []PermitReference
		if err := json.Unmarshal([]byte(r.Header.Get("x-acteon-execution-permits")), &permits); err != nil {
			t.Error(err)
		}
		if len(permits) != 1 || permits[0] != fixture.Parent.Permits[0] {
			t.Error("parent permits changed")
		}
		if r.Header.Get(AgentSourceContextHeader) != "" {
			t.Error("source context leaked into admission")
		}
		w.Header().Set(A2AVersionHeader, "1.0")
		w.Header().Set(AgentSourceContextHeader, fixture.Jobs[0].Source)
		json.NewEncoder(w).Encode(fixture.Jobs[0].Task)
	}))
	defer server.Close()
	parent := &AgentServiceParent{ExecutionContext: fixture.Parent.ExecutionContext, Permits: fixture.Parent.Permits}
	if _, err := NewClient(server.URL).AgentServiceSendMessageWithParent(context.Background(), "prod", "acme", "notifier", map[string]any{}, parent); err != nil {
		t.Fatal(err)
	}
}

func TestAgentServicePeerToolCarriesNoAuthorityFields(t *testing.T) {
	source := &AgentServiceReceipt{Namespace: "prod", Tenant: "acme", Agent: "notifier", TaskID: "job-1", SourceContext: "host-only"}
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		if r.Method == http.MethodGet && strings.HasSuffix(r.URL.Path, "/peers") {
			if r.URL.Query().Get("skill") != "diagnose" {
				t.Errorf("unexpected discovery query: %s", r.URL.RawQuery)
			}
			if r.Header.Get(AgentSourceContextHeader) != "" ||
				r.Header.Get(AgentExecutionContextHeader) != "" ||
				r.Header.Get("x-acteon-execution-permits") != "" {
				t.Error("authority fields leaked into peer discovery")
			}
			w.Header().Set(A2AVersionHeader, A2AProtocolVersion)
			json.NewEncoder(w).Encode(map[string]any{"peers": []any{map[string]any{
				"agent_id":              "resolver",
				"skill":                 "diagnose",
				"description_untrusted": "Investigates incidents",
				"card_version":          "v1",
				"binding_digest":        strings.Repeat("a", 64),
				"checked_at_ms":         42,
			}}})
			return
		}
		if !strings.Contains(r.URL.Path, "/tasks/job-1/peers/team/resolver/diagnose/") &&
			!strings.Contains(r.URL.RawPath, "/tasks/job-1/peers/team%2Fresolver/diagnose/") {
			t.Errorf("unexpected peer path: %s", r.URL.Path)
		}
		if r.Header.Get(AgentSourceContextHeader) != "" || r.Header.Get(AgentExecutionContextHeader) != "" || r.Header.Get("x-acteon-execution-permits") != "" {
			t.Error("authority fields leaked into peer tool request")
		}
		w.Header().Set(A2AVersionHeader, A2AProtocolVersion)
		if strings.HasSuffix(r.URL.Path, ":cancel") {
			json.NewEncoder(w).Encode(map[string]any{
				"submission_id":   "f47ac10b-58cc-5372-a567-0e02b2c3d479",
				"cancellation_id": "67e55044-10b1-526f-9247-bb680e5fe0c8",
				"status": map[string]any{"state": "restricted", "task": map[string]any{
					"id": "remote-1", "namespace": "prod", "tenant": "acme", "status": map[string]any{"state": "submitted"},
				}},
			})
			return
		}
		json.NewEncoder(w).Encode(map[string]any{
			"submission_id": "f47ac10b-58cc-5372-a567-0e02b2c3d479",
			"status":        map[string]any{"state": "accepted", "task": map[string]any{"id": "remote-1", "namespace": "prod", "tenant": "acme"}},
		})
	}))
	defer server.Close()
	peers, err := NewClient(server.URL).AgentServiceDiscoverPeers(context.Background(), source, "diagnose")
	if err != nil || len(peers) != 1 || peers[0].AgentID != "resolver" || calls.Load() != 1 {
		t.Fatalf("peer discovery mismatch: %#v %v", peers, err)
	}
	receipt, err := NewClient(server.URL).AgentServiceSendPeer(context.Background(), source, "team/resolver", "diagnose", map[string]any{"messageId": "peer-1"})
	if err != nil || receipt.Status.State != "accepted" || calls.Load() != 2 {
		t.Fatalf("peer receipt mismatch: %#v %v", receipt, err)
	}
	refreshed, err := NewClient(server.URL).AgentServiceRefreshPeer(context.Background(), source, "team/resolver", "diagnose", receipt)
	if err != nil || refreshed.SubmissionID != receipt.SubmissionID || calls.Load() != 3 {
		t.Fatalf("peer refresh mismatch: %#v %v", refreshed, err)
	}
	canceled, err := NewClient(server.URL).AgentServiceCancelPeer(context.Background(), source, "team/resolver", "diagnose", receipt)
	if err != nil || canceled.Status.State != "restricted" || calls.Load() != 4 {
		t.Fatalf("peer cancellation mismatch: %#v %v", canceled, err)
	}
}

func TestAgentServiceErrorsMissingHeaderAndRedirectsDoNotRetry(t *testing.T) {
	for _, status := range []int{200, 403, 404, 409, 429, 503, 307} {
		t.Run(http.StatusText(status), func(t *testing.T) {
			var calls atomic.Int32
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				calls.Add(1)
				w.Header().Set(A2AVersionHeader, "1.0")
				w.Header().Set("Location", "/redirected")
				w.WriteHeader(status)
				w.Write([]byte(`{"id":"job-1","namespace":"prod","tenant":"acme","metadata":{"source_context":"forged"}}`))
			}))
			defer server.Close()
			_, err := NewClient(server.URL).AgentServiceSendMessage(context.Background(), "prod", "acme", "notifier", map[string]any{})
			if err == nil {
				t.Fatal("missing header or failed response accepted")
			}
			if status != 200 {
				if httpErr, ok := err.(*HTTPError); !ok || httpErr.Status != status {
					t.Fatalf("status lost: %v", err)
				}
			}
			if calls.Load() != 1 {
				t.Fatalf("retried or redirected: %d", calls.Load())
			}
		})
	}
}

func TestAgentServiceStopRejectsFalseAcknowledgementsAndFailuresWithoutRetry(t *testing.T) {
	task := map[string]any{"id": "job-1", "namespace": "prod", "tenant": "acme"}
	receipt := &AgentServiceReceipt{Namespace: "prod", Tenant: "acme", Agent: "notifier", TaskID: "job-1", SourceContext: "opaque", Task: task}
	payloads := []any{nil, map[string]any{}, map[string]any{"task": task, "future_starts_blocked": false}, map[string]any{"task": task, "future_starts_blocked": "true"}, map[string]any{"task": map[string]any{"id": "foreign", "namespace": "prod", "tenant": "acme"}, "future_starts_blocked": true}, map[string]any{"task": task, "future_starts_blocked": true, "provider_abort": map[string]any{"state": "uncertain", "attempt_id": "bad"}}}
	for _, payload := range payloads {
		var calls atomic.Int32
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			calls.Add(1)
			w.Header().Set(A2AVersionHeader, "1.0")
			json.NewEncoder(w).Encode(payload)
		}))
		_, err := NewClient(server.URL).AgentServiceStopTask(context.Background(), receipt)
		server.Close()
		if err == nil || calls.Load() != 1 {
			t.Fatalf("invalid acknowledgement accepted or retried: %v", err)
		}
	}
	for _, status := range []int{403, 404, 409, 429, 503, 307} {
		var calls atomic.Int32
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			calls.Add(1)
			w.Header().Set("Location", "/redirected")
			w.WriteHeader(status)
		}))
		_, err := NewClient(server.URL).AgentServiceStopTask(context.Background(), receipt)
		server.Close()
		failure, ok := err.(*HTTPError)
		if !ok || failure.Status != status || calls.Load() != 1 {
			t.Fatalf("failed stop retried or status lost: %v", err)
		}
	}
}

func TestAgentProviderAbortRequiresCanonicalUUIDv5(t *testing.T) {
	for _, id := range []string{"F47AC10B-58CC-5372-A567-0E02B2C3D479", "f47ac10b-58cc-4372-a567-0e02b2c3d479"} {
		if _, err := agentProviderAbort(map[string]any{"state": "uncertain", "attempt_id": id}); err == nil {
			t.Fatalf("accepted non-canonical provider abort attempt %q", id)
		}
	}
}
