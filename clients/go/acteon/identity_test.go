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

func TestIdentityContracts(t *testing.T) {
	raw, err := os.ReadFile("../../contract-fixtures/identity.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixtures []json.RawMessage
	if err := json.Unmarshal(raw, &fixtures); err != nil {
		t.Fatal(err)
	}
	for _, wire := range fixtures {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			if r.Method != "GET" || r.URL.Path != "/v1/auth/identity" || r.Header.Get("Authorization") != "Bearer test-key" {
				t.Errorf("unexpected identity request: %s %s", r.Method, r.URL)
			}
			w.Header().Set("Content-Type", "application/json")
			w.Write(wire)
		}))
		actual, err := NewClient(server.URL, WithAPIKey("test-key")).Identity(context.Background())
		server.Close()
		if err != nil {
			t.Fatal(err)
		}
		var expected CredentialIdentity
		if err := json.Unmarshal(wire, &expected); err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(*actual, expected) {
			t.Fatalf("identity lost fields: %#v", actual)
		}
	}
}
