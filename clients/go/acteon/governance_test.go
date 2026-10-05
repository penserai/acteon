package acteon

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"reflect"
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
