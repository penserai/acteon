package acteon

// Phase 8c: Go SDK bus surface tests.
//
// Live HTTP tests would need a running Acteon instance with the
// bus feature enabled; these tests exercise wire-level serde and
// URL encoding. The contract under test: every wire field
// round-trips, optional fields drop cleanly, and path segments
// are properly percent-encoded.

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func ptr[T any](v T) *T { return &v }

func TestCreateBusTopicSerde(t *testing.T) {
	t.Run("minimal — drops every optional field", func(t *testing.T) {
		req := CreateBusTopic{Name: "t", Namespace: "n", Tenant: "te"}
		raw, err := json.Marshal(req)
		if err != nil {
			t.Fatalf("marshal: %v", err)
		}
		var got map[string]any
		if err := json.Unmarshal(raw, &got); err != nil {
			t.Fatalf("unmarshal: %v", err)
		}
		// `omitempty` on the optional pointer fields means they
		// don't appear in the wire form when nil.
		if _, ok := got["partitions"]; ok {
			t.Errorf("partitions should be omitted; got %v", got)
		}
		if _, ok := got["replication_factor"]; ok {
			t.Errorf("replication_factor should be omitted; got %v", got)
		}
		if _, ok := got["labels"]; ok {
			t.Errorf("empty labels should be omitted; got %v", got)
		}
	})

	t.Run("full — snake-cases all field names", func(t *testing.T) {
		req := CreateBusTopic{
			Name:              "t",
			Namespace:         "n",
			Tenant:            "te",
			Partitions:        ptr(4),
			ReplicationFactor: ptr(2),
			RetentionMs:       ptr[int64](86_400_000),
			Description:       ptr("demo"),
			Labels:            map[string]string{"env": "prod"},
		}
		raw, err := json.Marshal(req)
		if err != nil {
			t.Fatalf("marshal: %v", err)
		}
		s := string(raw)
		for _, want := range []string{
			`"replication_factor":2`,
			`"retention_ms":86400000`,
			`"labels":{"env":"prod"}`,
		} {
			if !strings.Contains(s, want) {
				t.Errorf("expected %s in body; got %s", want, s)
			}
		}
	})
}

func TestPostBusToolCallSerde(t *testing.T) {
	t.Run("basic — no approval gate fields", func(t *testing.T) {
		req := PostBusToolCall{
			CallID:    "call-1",
			Tool:      "calendar.list",
			Arguments: map[string]any{},
		}
		raw, _ := json.Marshal(req)
		s := string(raw)
		if strings.Contains(s, "require_approval") {
			t.Errorf("require_approval should be omitted when false; got %s", s)
		}
	})

	t.Run("Phase 6c gate — emits all approval fields", func(t *testing.T) {
		req := PostBusToolCall{
			CallID:          "call-1",
			Tool:            "billing.charge",
			Arguments:       map[string]any{"usd": 42},
			Sender:          ptr("planner-1"),
			RequireApproval: true,
			ApprovalReason:  ptr("paid action"),
			ApprovalTtlMs:   ptr[uint64](600_000),
		}
		raw, _ := json.Marshal(req)
		s := string(raw)
		for _, want := range []string{
			`"require_approval":true`,
			`"approval_reason":"paid action"`,
			`"approval_ttl_ms":600000`,
			`"call_id":"call-1"`,
		} {
			if !strings.Contains(s, want) {
				t.Errorf("expected %s in body; got %s", want, s)
			}
		}
	})
}

func TestPostBusToolResultSerde(t *testing.T) {
	req := PostBusToolResult{
		CallID:       "call-1",
		Status:       "error",
		Output:       map[string]any{},
		ErrorMessage: ptr("upstream gave up"),
		Sender:       ptr("calendar-svc"),
	}
	raw, _ := json.Marshal(req)
	s := string(raw)
	for _, want := range []string{
		`"status":"error"`,
		`"error_message":"upstream gave up"`,
		`"sender":"calendar-svc"`,
	} {
		if !strings.Contains(s, want) {
			t.Errorf("expected %s in body; got %s", want, s)
		}
	}
}

func TestPostBusStreamChunkSerde(t *testing.T) {
	req := PostBusStreamChunk{
		StreamID: "s1",
		ChunkSeq: 0,
		Body:     map[string]any{"token": "Once "},
	}
	raw, _ := json.Marshal(req)
	s := string(raw)
	if !strings.Contains(s, `"stream_id":"s1"`) {
		t.Errorf("expected stream_id in body; got %s", s)
	}
	if !strings.Contains(s, `"chunk_seq":0`) {
		t.Errorf("expected chunk_seq in body; got %s", s)
	}
	if !strings.Contains(s, `"body":{"token":"Once "}`) {
		t.Errorf("expected body wrapping the token; got %s", s)
	}
}

func TestBusApprovalDecisionSerde(t *testing.T) {
	d := BusApprovalDecision{
		DecidedBy:    "ops-1",
		DecisionNote: ptr("verified PO"),
	}
	raw, _ := json.Marshal(d)
	s := string(raw)
	if !strings.Contains(s, `"decided_by":"ops-1"`) {
		t.Errorf("expected decided_by; got %s", s)
	}
	if !strings.Contains(s, `"decision_note":"verified PO"`) {
		t.Errorf("expected decision_note; got %s", s)
	}
}

func TestBusTopicResponseRoundTrip(t *testing.T) {
	body := []byte(`{
		"name": "t", "namespace": "n", "tenant": "te",
		"kafka_name": "n.te.t", "partitions": 4, "replication_factor": 2,
		"created_at": "2026-01-01T00:00:00Z",
		"updated_at": "2026-01-01T00:00:00Z"
	}`)
	var topic BusTopic
	if err := json.Unmarshal(body, &topic); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if topic.KafkaName != "n.te.t" {
		t.Errorf("kafka_name mismatch")
	}
	if topic.SchemaSubject != nil {
		t.Errorf("schema_subject should be nil when omitted; got %v", topic.SchemaSubject)
	}
}

func TestBusLagResponseRoundTrip(t *testing.T) {
	body := []byte(`{
		"subscription_id": "s1", "topic": "n.te.t",
		"partitions": [
			{"partition": 0, "committed": 10, "high_water_mark": 12, "lag": 2},
			{"partition": 1, "committed": 0, "high_water_mark": 0, "lag": 0}
		],
		"total_lag": 2
	}`)
	var lag BusLag
	if err := json.Unmarshal(body, &lag); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if lag.TotalLag != 2 {
		t.Errorf("total_lag mismatch: %d", lag.TotalLag)
	}
	if len(lag.Partitions) != 2 {
		t.Errorf("expected 2 partitions; got %d", len(lag.Partitions))
	}
	if lag.Partitions[0].HighWaterMark != 12 {
		t.Errorf("high_water_mark mismatch: %d", lag.Partitions[0].HighWaterMark)
	}
}

func TestBusApprovalViewRoundTrip(t *testing.T) {
	body := []byte(`{
		"approval_id": "appr-1", "namespace": "n", "tenant": "te",
		"conversation_id": "c1", "correlation_token": "call-1",
		"envelope_kind": "tool_call", "status": "pending",
		"created_at": "2026-01-01T00:00:00Z",
		"expires_at": "2026-01-02T00:00:00Z",
		"envelope": {"kind": "tool_call"}
	}`)
	var v BusApprovalView
	if err := json.Unmarshal(body, &v); err != nil {
		t.Fatalf("unmarshal: %v", err)
	}
	if v.Status != "pending" {
		t.Errorf("status mismatch: %s", v.Status)
	}
	if v.DecidedBy != nil {
		t.Errorf("decided_by should be nil before decision; got %v", v.DecidedBy)
	}
	if v.ProducedOffset != nil {
		t.Errorf("produced_offset should be nil before approve; got %v", v.ProducedOffset)
	}
}

func TestBusApprovalDecisionResponseRoundTrip(t *testing.T) {
	t.Run("approved with receipt", func(t *testing.T) {
		body := []byte(`{
			"approval": {
				"approval_id": "appr-1", "namespace": "n", "tenant": "te",
				"conversation_id": "c1", "correlation_token": "call-1",
				"envelope_kind": "tool_call", "status": "approved",
				"created_at": "2026-01-01T00:00:00Z",
				"expires_at": "2026-01-02T00:00:00Z",
				"envelope": {},
				"decided_by": "ops-1"
			},
			"receipt": {
				"events_topic": "n.te.events",
				"conversation_id": "c1", "call_id": "call-1",
				"partition": 0, "offset": 99,
				"produced_at": "2026-01-01T00:00:01Z",
				"cursor": "xx"
			}
		}`)
		var r BusApprovalDecisionResponse
		if err := json.Unmarshal(body, &r); err != nil {
			t.Fatalf("unmarshal: %v", err)
		}
		if r.Approval.Status != "approved" {
			t.Errorf("status mismatch")
		}
		if r.Receipt == nil || r.Receipt.Offset != 99 {
			t.Errorf("expected receipt with offset 99; got %v", r.Receipt)
		}
	})

	t.Run("rejected — receipt nil", func(t *testing.T) {
		body := []byte(`{
			"approval": {
				"approval_id": "appr-1", "namespace": "n", "tenant": "te",
				"conversation_id": "c1", "correlation_token": "call-1",
				"envelope_kind": "tool_call", "status": "rejected",
				"created_at": "2026-01-01T00:00:00Z",
				"expires_at": "2026-01-02T00:00:00Z",
				"envelope": {},
				"decided_by": "ops-1",
				"decision_note": "scope too broad"
			},
			"receipt": null
		}`)
		var r BusApprovalDecisionResponse
		if err := json.Unmarshal(body, &r); err != nil {
			t.Fatalf("unmarshal: %v", err)
		}
		if r.Receipt != nil {
			t.Errorf("expected nil receipt on reject; got %v", r.Receipt)
		}
	})
}

func TestBusStreamConsumeURL(t *testing.T) {
	c := NewClient("http://localhost:3000")

	t.Run("simple segments", func(t *testing.T) {
		got := c.BusStreamConsumeURL("agents", "demo", "thread-1", "stream-1")
		want := "http://localhost:3000/v1/bus/streams/agents/demo/thread-1/stream-1"
		if got != want {
			t.Errorf("simple URL mismatch:\n  want: %s\n   got: %s", want, got)
		}
	})

	t.Run("encodes slashes and spaces", func(t *testing.T) {
		got := c.BusStreamConsumeURL("agents/x", "demo", "thread/with/slashes", "story 1")
		// Embedded slashes inside segments must be percent-encoded
		// so they don't escape into URL grammar; spaces become %20.
		for _, want := range []string{"agents%2Fx", "thread%2Fwith%2Fslashes", "story%201"} {
			if !strings.Contains(got, want) {
				t.Errorf("expected %s in URL; got %s", want, got)
			}
		}
	})
}

func TestBusToolCallOutcome(t *testing.T) {
	t.Run("produced", func(t *testing.T) {
		o := PostBusToolCallOutcome{
			Produced: &BusToolEnvelopeReceipt{CallID: "call-1", Offset: 42},
		}
		if o.WasParked() {
			t.Errorf("WasParked should be false for produced outcome")
		}
	})
	t.Run("parked", func(t *testing.T) {
		o := PostBusToolCallOutcome{
			Parked: &BusApprovalParkedReceipt{ApprovalID: "appr-1"},
		}
		if !o.WasParked() {
			t.Errorf("WasParked should be true for parked outcome")
		}
	})
}

// Server-driven tests — use httptest to assert that PostBusToolCall
// branches correctly on 200 vs 202, ApproveBusApproval / RejectBusApproval
// hit the right paths, and the URL builder honors the configured base.

func TestPostBusToolCallBranchesOn202(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !strings.HasSuffix(r.URL.Path, "/tool-calls") {
			t.Errorf("unexpected path: %s", r.URL.Path)
		}
		// Decide based on the request body's `require_approval` flag.
		var body map[string]any
		_ = json.NewDecoder(r.Body).Decode(&body)
		if v, _ := body["require_approval"].(bool); v {
			w.WriteHeader(http.StatusAccepted)
			_, _ = w.Write([]byte(`{
				"approval_id": "appr-1",
				"namespace": "n", "tenant": "te",
				"conversation_id": "c1",
				"correlation_token": "call-1",
				"status": "pending",
				"created_at": "2026-01-01T00:00:00Z",
				"expires_at": "2026-01-02T00:00:00Z"
			}`))
			return
		}
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`{
			"events_topic": "n.te.events",
			"conversation_id": "c1", "call_id": "call-1",
			"partition": 0, "offset": 17,
			"produced_at": "2026-01-01T00:00:00Z",
			"cursor": "abc"
		}`))
	}))
	defer server.Close()

	client := NewClient(server.URL)
	ctx := context.Background()

	t.Run("immediate produce → kind=Produced", func(t *testing.T) {
		out, err := client.PostBusToolCall(ctx, "n", "te", "c1", &PostBusToolCall{
			CallID: "call-1", Tool: "calendar.list",
		})
		if err != nil {
			t.Fatalf("post: %v", err)
		}
		if out.WasParked() {
			t.Errorf("expected immediate produce; got parked")
		}
		if out.Produced == nil || out.Produced.Offset != 17 {
			t.Errorf("expected Produced.Offset==17; got %v", out.Produced)
		}
	})

	t.Run("require_approval → kind=Parked", func(t *testing.T) {
		out, err := client.PostBusToolCall(ctx, "n", "te", "c1", &PostBusToolCall{
			CallID:          "call-1",
			Tool:            "billing.charge",
			Arguments:       map[string]any{"usd": 42},
			RequireApproval: true,
			ApprovalReason:  ptr("paid action"),
		})
		if err != nil {
			t.Fatalf("post: %v", err)
		}
		if !out.WasParked() {
			t.Errorf("expected parked outcome; got produced")
		}
		if out.Parked == nil || out.Parked.ApprovalID != "appr-1" {
			t.Errorf("expected Parked.ApprovalID=appr-1; got %v", out.Parked)
		}
	})
}

func TestApproveBusApprovalRoutesAndDecodes(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// The handler asserts the path the SDK builds matches the
		// server's expected route.
		if r.Method != http.MethodPost {
			t.Errorf("expected POST; got %s", r.Method)
		}
		expected := "/v1/bus/approvals/" + url.PathEscape("agents") + "/" + url.PathEscape("demo") +
			"/" + url.PathEscape("appr-1") + "/approve"
		if r.URL.Path != expected {
			t.Errorf("path mismatch: want %s, got %s", expected, r.URL.Path)
		}
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`{
			"approval": {
				"approval_id": "appr-1",
				"namespace": "agents", "tenant": "demo",
				"conversation_id": "c1", "correlation_token": "call-1",
				"envelope_kind": "tool_call", "status": "approved",
				"created_at": "2026-01-01T00:00:00Z",
				"expires_at": "2026-01-02T00:00:00Z",
				"envelope": {},
				"decided_by": "ops-1"
			},
			"receipt": {
				"events_topic": "agents.demo.events",
				"conversation_id": "c1", "call_id": "call-1",
				"partition": 0, "offset": 99,
				"produced_at": "2026-01-01T00:00:01Z",
				"cursor": "xx"
			}
		}`))
	}))
	defer server.Close()

	client := NewClient(server.URL)
	resp, err := client.ApproveBusApproval(context.Background(), "agents", "demo", "appr-1",
		&BusApprovalDecision{DecidedBy: "ops-1"})
	if err != nil {
		t.Fatalf("approve: %v", err)
	}
	if resp.Approval.Status != "approved" {
		t.Errorf("status mismatch: %s", resp.Approval.Status)
	}
	if resp.Receipt == nil || resp.Receipt.Offset != 99 {
		t.Errorf("expected receipt with offset 99; got %v", resp.Receipt)
	}
}

func TestBusErrorParsing(t *testing.T) {
	// The bus error path should map structured `{"error": "..."}`
	// bodies (the shape Acteon's bus handlers emit) to *APIError.
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusBadRequest)
		_, _ = w.Write([]byte(`{"error":"sender 'alpha' is not a participant"}`))
	}))
	defer server.Close()

	client := NewClient(server.URL)
	_, err := client.CreateBusTopic(context.Background(), &CreateBusTopic{
		Name: "t", Namespace: "n", Tenant: "te",
	})
	if err == nil {
		t.Fatalf("expected error; got nil")
	}
	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("expected *APIError; got %T: %v", err, err)
	}
	if !strings.Contains(apiErr.Message, "sender") {
		t.Errorf("expected sender error; got %s", apiErr.Message)
	}
}
