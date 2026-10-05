package acteon

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"reflect"
	"sync"
	"sync/atomic"
	"testing"
)

func TestWorkforceWireContracts(t *testing.T) {
	raw, err := os.ReadFile("../../contract-fixtures/workforce-management.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct {
		Scope   json.RawMessage   `json:"scope"`
		Changes []json.RawMessage `json:"changes"`
		Receipt json.RawMessage   `json:"receipt"`
	}
	if err = json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	var status atomic.Int32
	status.Store(200)
	var count atomic.Int32
	var mu sync.Mutex
	var bodies []any
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		count.Add(1)
		if r.Header.Get("Authorization") != "Bearer operator-key" {
			t.Error("missing authentication")
		}
		w.Header().Set("Content-Type", "application/json")
		if r.Method == "GET" {
			if r.URL.Path != "/v1/workforce" || r.URL.Query().Get("namespace") != "prod" || r.URL.Query().Get("tenant") != "acme" {
				t.Error("wrong scope")
			}
			w.Write(fixture.Scope)
			return
		}
		if r.URL.Path != "/v1/workforce/changes" {
			t.Error("wrong mutation endpoint")
		}
		var body any
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Error(err)
		}
		mu.Lock()
		bodies = append(bodies, body)
		mu.Unlock()
		w.WriteHeader(int(status.Load()))
		w.Write(fixture.Receipt)
	}))
	defer server.Close()
	client := NewClient(server.URL, WithAPIKey("operator-key"))
	ctx := context.Background()
	scope, err := client.Workforce(ctx, "prod", "acme")
	if err != nil {
		t.Fatal(err)
	}
	equalJSON := func(actual any, expected []byte) {
		encoded, err := json.Marshal(actual)
		if err != nil {
			t.Fatal(err)
		}
		var a, b any
		json.Unmarshal(encoded, &a)
		json.Unmarshal(expected, &b)
		if !reflect.DeepEqual(a, b) {
			t.Errorf("wire mismatch: %s", encoded)
		}
	}
	equalJSON(scope, fixture.Scope)
	var request WorkforceChangeRequest
	for index, wire := range fixture.Changes {
		if err = json.Unmarshal(wire, &request); err != nil {
			t.Fatal(err)
		}
		equalJSON(request, wire)
		receipt, err := client.ChangeWorkforce(ctx, request)
		if err != nil {
			t.Fatal(err)
		}
		equalJSON(receipt, fixture.Receipt)
		mu.Lock()
		body := bodies[index]
		mu.Unlock()
		equalJSON(body, wire)
	}
	for _, denied := range []int32{401, 403, 409, 503} {
		status.Store(denied)
		before := count.Load()
		_, err := client.ChangeWorkforce(ctx, request)
		httpError, ok := err.(*HTTPError)
		if !ok || httpError.Status != int(denied) {
			t.Fatalf("status not preserved: %v", err)
		}
		if count.Load() != before+1 {
			t.Fatal("mutation retried")
		}
	}
}
