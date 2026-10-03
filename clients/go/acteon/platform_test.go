package acteon

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"reflect"
	"strings"
	"testing"
)

func TestPlatformOperationsWire(t *testing.T) {
	for operation, spec := range platformOperations {
		t.Run(string(operation), func(t *testing.T) {
			path := map[string]string{}
			expected := spec.path
			for _, key := range spec.parameters {
				path[key] = "team/child ?#%"
				expected = strings.ReplaceAll(expected, "{"+key+"}", url.PathEscape(path[key]))
			}
			calls := 0
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				calls++
				if r.Method != spec.method || r.URL.EscapedPath() != expected {
					t.Errorf("wrong request: %s %s", r.Method, r.URL)
				}
				if r.Header.Get("Authorization") != "Bearer test-key" {
					t.Error("missing auth")
				}
				if fmt.Sprint(r.URL.Query()["filter"]) != "[a b c&d]" {
					t.Error("query changed")
				}
				body, _ := io.ReadAll(r.Body)
				if spec.method != "GET" && string(body) != `{"request_id":"stable"}` {
					t.Errorf("body changed: %s", body)
				}
				if spec.text {
					fmt.Fprint(w, "metric 1\n")
				} else {
					fmt.Fprint(w, `{"opaque":[1,null]}`)
				}
			}))
			defer server.Close()
			client := NewClient(server.URL, WithAPIKey("test-key"))
			var body any
			if spec.method != "GET" {
				body = map[string]string{"request_id": "stable"}
			}
			result, err := client.PlatformRequest(context.Background(), operation, path, url.Values{"filter": {"a b", "c&d"}}, body)
			if err != nil {
				t.Fatal(err)
			}
			expectedResult := `{"opaque":[1,null]}`
			if spec.text {
				expectedResult = `"metric 1\n"`
			}
			if string(result) != expectedResult || calls != 1 {
				t.Fatalf("result %s, calls %d", result, calls)
			}
		})
	}
}

func TestPlatformStatusAndValidation(t *testing.T) {
	calls := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { calls++; w.WriteHeader(409); fmt.Fprint(w, "conflict") }))
	defer server.Close()
	client := NewClient(server.URL)
	_, err := client.PlatformRequest(context.Background(), OpAuthLogout, nil, nil, nil)
	var status *HTTPError
	if !errors.As(err, &status) || status.Status != 409 || calls != 1 {
		t.Fatalf("error %v calls %d", err, calls)
	}
	_, err = client.PlatformRequest(context.Background(), OpBusStagesStatus, map[string]string{"namespace": "n", "tenant": "t", "id": ".."}, nil, nil)
	if err == nil || calls != 1 {
		t.Fatal("invalid path reached transport")
	}
	var sub BusSubscription
	if err := json.Unmarshal([]byte(`{"receipt_required":true,"consumer_group":"scoped"}`), &sub); err != nil {
		t.Fatal(err)
	}
	if !sub.ReceiptRequired || sub.ConsumerGroup != "scoped" {
		t.Fatal("receipt fields lost")
	}
}

func TestGovernanceOutcomeContract(t *testing.T) {
	data, err := os.ReadFile("../../contract-fixtures/dispatch-outcomes.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixtures []struct {
		Type string          `json:"type"`
		Wire json.RawMessage `json:"wire"`
	}
	if err := json.Unmarshal(data, &fixtures); err != nil {
		t.Fatal(err)
	}
	for _, fixture := range fixtures {
		var outcome ActionOutcome
		if err := json.Unmarshal(fixture.Wire, &outcome); err != nil {
			t.Fatal(err)
		}
		if string(outcome.Type) != fixture.Type || outcome.IsExecuted() || outcome.IsFailed() {
			t.Fatalf("incorrect outcome %+v", outcome)
		}
		var batch BatchResult
		if err := json.Unmarshal(fixture.Wire, &batch); err != nil {
			t.Fatal(err)
		}
		if !batch.Success || batch.Outcome.Type != outcome.Type {
			t.Fatal("batch lost outcome")
		}
		if fixture.Type == "deduplicated" {
			continue
		}
		var wire map[string]map[string]any
		if err := json.Unmarshal(fixture.Wire, &wire); err != nil {
			t.Fatal(err)
		}
		for _, fields := range wire {
			for key, expected := range fields {
				parts := strings.Split(key, "_")
				for i, part := range parts {
					parts[i] = strings.ToUpper(part[:1]) + part[1:]
				}
				name := strings.ReplaceAll(strings.ReplaceAll(strings.Join(parts, ""), "Id", "ID"), "Url", "URL")
				field := reflect.ValueOf(outcome).FieldByName(name)
				actual, err := json.Marshal(field.Interface())
				if err != nil {
					t.Fatal(err)
				}
				var value any
				if err := json.Unmarshal(actual, &value); err != nil {
					t.Fatal(err)
				}
				if !reflect.DeepEqual(value, expected) {
					t.Errorf("%s lost %s: %s", fixture.Type, key, actual)
				}
			}
		}
	}
}

func TestTypedTopicDeletionResolvesScopedKafkaName(t *testing.T) {
	var seen []string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		seen = append(seen, r.Method+" "+r.URL.Path)
		if r.Method == "GET" {
			if r.URL.Query().Get("namespace") != "n" || r.URL.Query().Get("tenant") != "t" {
				t.Error("missing scope")
			}
			fmt.Fprint(w, `{"topics":[{"name":"logs","namespace":"n","tenant":"other","kafka_name":"wrong"},{"name":"logs","namespace":"n","tenant":"t","kafka_name":"actual.logs"}]}`)
		} else {
			w.WriteHeader(204)
		}
	}))
	defer server.Close()
	if err := NewClient(server.URL).DeleteBusTopic(context.Background(), "n", "t", "logs"); err != nil {
		t.Fatal(err)
	}
	if fmt.Sprint(seen) != "[GET /v1/bus/topics DELETE /v1/bus/topics/actual.logs]" {
		t.Fatal(seen)
	}
}
