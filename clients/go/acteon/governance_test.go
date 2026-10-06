package acteon

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"reflect"
	"strings"
	"testing"
)

func TestGovernanceWireContracts(t *testing.T) {
	raw, err := os.ReadFile("../../contract-fixtures/governance-management.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture map[string]json.RawMessage
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	var publication PublishGovernancePermitRequest
	var intervention GovernanceInterventionRequest
	if err = json.Unmarshal(fixture["publication"], &publication); err != nil {
		t.Fatal(err)
	}
	if err = json.Unmarshal(fixture["intervention"], &intervention); err != nil {
		t.Fatal(err)
	}
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls++
		if r.Header.Get("Authorization") != "Bearer operator-key" {
			t.Error("missing authentication")
		}
		w.Header().Set("Content-Type", "application/json")
		if r.Method == "GET" {
			if r.URL.Path != "/v1/governance" || r.URL.Query().Get("namespace") != "prod" || r.URL.Query().Get("tenant") != "acme" {
				t.Error("wrong scope")
			}
			w.Write(fixture["scope"])
			return
		}
		key := "publication"
		if r.URL.Path == "/v1/governance/changes" {
			key = "intervention"
		} else if r.URL.Path != "/v1/governance/permits" {
			t.Error("wrong path")
		}
		var actual, expected any
		if err := json.NewDecoder(r.Body).Decode(&actual); err != nil {
			t.Fatal(err)
		}
		json.Unmarshal(fixture[key], &expected)
		if !reflect.DeepEqual(actual, expected) {
			t.Errorf("wrong body: %v", actual)
		}
		w.Write(fixture["receipt"])
	}))
	defer server.Close()
	client := NewClient(server.URL, WithAPIKey("operator-key"))
	ctx := context.Background()
	scope, err := client.Governance(ctx, "prod", "acme")
	if err != nil {
		t.Fatal(err)
	}
	if scope.Management.Subjects[0].ID != "agent/maya" {
		t.Fatal("lost typed management bounds")
	}
	receipt, err := client.PublishGovernancePermit(ctx, publication)
	if err != nil {
		t.Fatal(err)
	}
	if receipt.Generation != 5 {
		t.Fatal("lost generation")
	}
	receipt, err = client.InterveneGovernance(ctx, intervention)
	if err != nil {
		t.Fatal(err)
	}
	if receipt.Actor != "operator" {
		t.Fatal("lost actor")
	}
	if calls != 3 {
		t.Fatalf("unexpected retries: %d", calls)
	}
}
func TestGovernanceRefusalsPreserveStatus(t *testing.T) {
	for _, status := range []int{401, 403, 409, 503} {
		calls := 0
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			calls++
			w.WriteHeader(status)
			w.Write([]byte(`{"error":"denied"}`))
		}))
		_, err := NewClient(server.URL).InterveneGovernance(context.Background(), GovernanceInterventionRequest{})
		server.Close()
		httpErr, ok := err.(*HTTPError)
		if !ok || httpErr.Status != status {
			t.Fatalf("lost HTTP status: %v", err)
		}
		if calls != 1 {
			t.Fatal("unexpected retry")
		}
	}
}

func TestHistoryCapabilityDefaultsDeniedAndPreservesLegacyWire(t *testing.T) {
	for _, granted := range []bool{false, true} {
		input := `{"subjects":[],"can_issue_permits":false,"can_intervene":false,"valid_from_ms":0,"limits":{"max_units":1,"max_concurrent":1,"deadline_ms":1}}`
		var expected map[string]any
		if err := json.Unmarshal([]byte(input), &expected); err != nil {
			t.Fatal(err)
		}
		if granted {
			expected["can_read_history"] = true
			expected["can_reconcile"] = true
		}
		raw, err := json.Marshal(expected)
		if err != nil {
			t.Fatal(err)
		}
		var bounds GovernanceManagementBounds
		if err = json.Unmarshal(raw, &bounds); err != nil {
			t.Fatal(err)
		}
		if bounds.CanReadHistory != granted || bounds.CanReconcile != granted {
			t.Fatal("history permission mismatch")
		}
		encoded, err := json.Marshal(bounds)
		if err != nil {
			t.Fatal(err)
		}
		var actual map[string]any
		if err = json.Unmarshal(encoded, &actual); err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(actual, expected) {
			t.Fatalf("wire mismatch: %s", encoded)
		}
	}
}

type historyTransport func(*http.Request) (*http.Response, error)

func (f historyTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestProviderHistoryReadsNestedEvidence(t *testing.T) {
	raw, err := os.ReadFile("../../contract-fixtures/provider-history.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixtures []json.RawMessage
	if err := json.Unmarshal(raw, &fixtures); err != nil {
		t.Fatal(err)
	}
	index := 0
	client := NewClient("https://acteon.example", WithAPIKey("operator-key"), WithHTTPClient(&http.Client{Transport: historyTransport(func(r *http.Request) (*http.Response, error) {
		if r.Method != "GET" || r.URL.Path != "/v1/governance/executions/73000000-0000-4000-8000-000000000001" ||
			r.URL.Query().Get("namespace") != "prod" || r.URL.Query().Get("tenant") != "acme" || r.Header.Get("Authorization") != "Bearer operator-key" {
			t.Fatalf("wrong history request: %v", r)
		}
		body := fixtures[index]
		index++
		return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(string(body)))}, nil
	})}))
	for _, wire := range fixtures {
		var expected ProviderExecutionHistory
		if err := json.Unmarshal(wire, &expected); err != nil {
			t.Fatal(err)
		}
		result, err := client.ProviderExecutionHistory(context.Background(), "prod", "acme", expected.Receipt.ExecutionID)
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(*result, expected) {
			t.Fatal("history lost retained evidence")
		}
		if result.Receipt.Status.State == "completed" {
			acceptance := result.Attempts[0].Reconciliation.Acceptance
			if strings.Contains(string(wire), "\"acceptance\"") != (acceptance != nil) {
				t.Fatal("acceptance omission/presence was not preserved")
			}
			if acceptance != nil && (acceptance.Operator.ID != "operator/incident" || acceptance.Authority.Generation != 6 || acceptance.AcceptedAtMs != 1700000060000) {
				t.Fatal("operator acceptance audit was not decoded")
			}
			if !result.Receipt.Status.Outcome.IsExecuted() || !result.Attempts[0].OriginalOutcome.IsFailed() || !result.Attempts[0].Reconciliation.Outcome.IsExecuted() {
				t.Fatal("nested outcome decoding failed")
			}
			if result.Attempts[0].OriginalEvidence.ID != "original-result" || result.Attempts[0].Reconciliation.Resolution.ID != "resolution" {
				t.Fatal("original evidence conflated with reconciliation")
			}
		}
	}
	if index != len(fixtures) {
		t.Fatal("missing reads")
	}
}

func TestProviderReconciliationTransport(t *testing.T) {
	raw, err := os.ReadFile("../../contract-fixtures/provider-reconciliation.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		Correlation ProviderReconciliationCorrelation `json:"correlation"`
		Request     ProviderReconciliationRequest     `json:"request"`
		Receipt     json.RawMessage                   `json:"receipt"`
		NoEffect    json.RawMessage                   `json:"no_effect_receipt"`
	}
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	correlation, _ := json.Marshal(fixture.Correlation)
	responses := [][]byte{correlation, fixture.Receipt, fixture.NoEffect}
	index := 0
	client := NewClient("https://acteon.example", WithAPIKey("operator-key"), WithHTTPClient(&http.Client{Transport: historyTransport(func(r *http.Request) (*http.Response, error) {
		tail, method := "correlation", "GET"
		if index > 0 {
			tail, method = "reconciliation", "POST"
		}
		if r.URL.Path != "/v1/governance/executions/"+fixture.Correlation.Context.ExecutionID+"/attempts/0/"+tail || r.Method != method {
			t.Fatal("wrong finality route")
		}
		if r.URL.Query().Get("namespace") != "prod" || r.URL.Query().Get("tenant") != "acme" || r.Header.Get("Authorization") != "Bearer operator-key" {
			t.Fatal("lost scope/authentication")
		}
		if index > 0 {
			var body ProviderReconciliationRequest
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body != fixture.Request {
				t.Fatal("lost opaque proof")
			}
		}
		body := responses[index]
		index++
		return &http.Response{StatusCode: 200, Header: http.Header{"Content-Type": {"application/json"}}, Body: io.NopCloser(strings.NewReader(string(body)))}, nil
	})}))
	result, err := client.ProviderReconciliationCorrelation(context.Background(), "prod", "acme", fixture.Correlation.Context.ExecutionID, 0)
	if err != nil || !reflect.DeepEqual(*result, fixture.Correlation) {
		t.Fatal("lost correlation", err)
	}
	for _, expected := range []json.RawMessage{fixture.Receipt, fixture.NoEffect} {
		result, err := client.AcceptProviderReconciliation(context.Background(), "prod", "acme", fixture.Correlation.Context.ExecutionID, 0, fixture.Request)
		var wanted ProviderHistoryReceipt
		if e := json.Unmarshal(expected, &wanted); e != nil {
			t.Fatal(e)
		}
		if err != nil || !reflect.DeepEqual(*result, wanted) {
			t.Fatal("lost typed finality outcome", err)
		}
	}
	if index != 3 {
		t.Fatal("unexpected replay")
	}
}
