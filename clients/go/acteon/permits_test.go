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

func TestPermitDispatchHeaders(t *testing.T) {
	raw, err := os.ReadFile("../../contract-fixtures/execution-permits.json")
	if err != nil {
		t.Fatal(err)
	}
	var permits []PermitReference
	if err := json.Unmarshal(raw, &permits); err != nil {
		t.Fatal(err)
	}
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer test-key" {
			t.Error("lost credentials")
		}
		header := r.Header.Get("x-acteon-execution-permits")
		if calls == 0 {
			if header != "" {
				t.Error("unexpected legacy permit header")
			}
		} else {
			var actual []PermitReference
			if err := json.Unmarshal([]byte(header), &actual); err != nil || !reflect.DeepEqual(actual, permits) {
				t.Errorf("invalid permit header: %s", header)
			}
		}
		calls++
		w.Header().Set("Content-Type", "application/json")
		if r.URL.Path == "/v1/dispatch/batch" {
			w.Write([]byte(`["Deduplicated"]`))
		} else {
			w.Write([]byte(`"Deduplicated"`))
		}
	}))
	defer server.Close()
	client := NewClient(server.URL, WithAPIKey("test-key"))
	action := &Action{Namespace: "prod", Tenant: "acme", Provider: "incident", ActionType: "execute", Payload: map[string]any{"incident": 42}}
	if _, err := client.Dispatch(context.Background(), action); err != nil {
		t.Fatal(err)
	}
	if _, err := client.DispatchWithPermits(context.Background(), action, permits); err != nil {
		t.Fatal(err)
	}
	if _, err := client.DispatchBatchWithPermits(context.Background(), []*Action{action}, permits); err != nil {
		t.Fatal(err)
	}
	if calls != 3 {
		t.Fatalf("unexpected calls: %d", calls)
	}
}

func TestPermitRefusalPreservesHTTPStatus(t *testing.T) {
	for _, status := range []int{400, 403, 409} {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(status)
			w.Write([]byte(`{"error":"permit refused"}`))
		}))
		client := NewClient(server.URL)
		_, err := client.DispatchWithPermits(context.Background(), &Action{}, []PermitReference{})
		if actual, ok := err.(*HTTPError); !ok || actual.Status != status {
			t.Fatalf("unexpected refusal: %v", err)
		}
		_, err = client.DispatchBatchWithPermits(context.Background(), []*Action{{}}, []PermitReference{})
		if actual, ok := err.(*HTTPError); !ok || actual.Status != status {
			t.Fatalf("unexpected batch refusal: %v", err)
		}
		server.Close()
	}
}
