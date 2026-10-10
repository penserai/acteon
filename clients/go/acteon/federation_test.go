package acteon

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"testing"
)

func TestFederationManagementAndInertImportWire(t *testing.T) {
	wireBytes, err := os.ReadFile("../../contract-fixtures/federation-management.json")
	if err != nil {
		t.Fatal(err)
	}
	var wire map[string]json.RawMessage
	if err = json.Unmarshal(wireBytes, &wire); err != nil {
		t.Fatal(err)
	}
	var publication PublishFederationTrustRequest
	var revocation RevokeFederationTrustRequest
	var envelope SignedFederationEnvelopeDeclaration
	if err = json.Unmarshal(wire["publication"], &publication); err != nil {
		t.Fatal(err)
	}
	if err = json.Unmarshal(wire["revocation"], &revocation); err != nil {
		t.Fatal(err)
	}
	if err = json.Unmarshal(wire["envelope"], &envelope); err != nil {
		t.Fatal(err)
	}
	var calls []json.RawMessage
	var paths []string
	transport := roundTripFunc(func(r *http.Request) (*http.Response, error) {
		if r.Header.Get("Authorization") != "Bearer federation-key" {
			t.Fatal("missing authentication")
		}
		var body json.RawMessage
		if r.Method != http.MethodGet {
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Fatal(err)
			}
			calls = append(calls, body)
		}
		paths = append(paths, r.URL.Path)
		response := wire["change_receipt"]
		if r.URL.Path == "/v1/governance/federation/deliveries" {
			response = wire["delivery_evidence_page"]
		} else if bytes.Contains([]byte(r.URL.Path), []byte("/federation/exports/")) {
			response = wire["export_receipt"]
		} else if r.URL.Path == "/a2a/prod/acme/federation/envelopes:import" {
			response = wire["import_receipt"]
		} else if r.URL.Path == "/a2a/prod/acme/federation/agents/maya/v1/message:send" {
			response = json.RawMessage(`{"id":"federated-task","namespace":"prod","tenant":"acme"}`)
		}
		return &http.Response{
			StatusCode: http.StatusOK,
			Header:     http.Header{"Content-Type": []string{"application/json"}},
			Body:       io.NopCloser(bytes.NewReader(response)),
		}, nil
	})
	client := NewClient("https://acteon.example", WithAPIKey("federation-key"), WithHTTPClient(&http.Client{Transport: transport}))
	ctx := context.Background()
	if _, err = client.PublishFederationTrust(ctx, publication); err != nil {
		t.Fatal(err)
	}
	if _, err = client.RevokeFederationTrust(ctx, revocation); err != nil {
		t.Fatal(err)
	}
	receipt, err := client.ImportFederationEnvelope(ctx, "prod", "acme", envelope)
	if err != nil {
		t.Fatal(err)
	}
	if receipt.ExecutionStarted {
		t.Fatal("import started execution")
	}
	invocation := SignedFederationInvocationDeclaration{Payload: "signed-payload", Signature: string(bytes.Repeat([]byte("a"), 128))}
	message := map[string]any{"messageId": "message-1", "role": "user", "parts": []any{map[string]any{"text": "inspect"}}}
	task, err := client.InvokeFederatedAgent(ctx, "prod", "acme", "maya", invocation, message)
	if err != nil || task["id"] != "federated-task" {
		t.Fatalf("unexpected federated task: %v (%v)", task, err)
	}
	var exportMessage map[string]any
	if err = json.Unmarshal(wire["export_message"], &exportMessage); err != nil {
		t.Fatal(err)
	}
	sourceTask := "11111111-1111-4111-8111-111111111111"
	exported, err := client.ExportFederatedMessage(ctx, "prod", "acme", "maya", sourceTask, "remote-triage", exportMessage)
	if err != nil || exported.AttemptStatus != "in_flight" {
		t.Fatalf("unexpected export receipt: %v (%v)", exported, err)
	}
	if _, err = client.ObserveFederatedExport(ctx, "prod", "acme", "maya", sourceTask, "remote-triage", exportMessage); err != nil {
		t.Fatal(err)
	}
	if _, err = client.ReplayFederatedExport(ctx, "prod", "acme", "maya", sourceTask, "remote-triage", exportMessage); err != nil {
		t.Fatal(err)
	}
	limit := 25
	evidence, err := client.FederationDeliveryEvidence(ctx, "prod", "acme", nil, &limit)
	if err != nil || len(evidence.Deliveries) != 1 || evidence.Deliveries[0].MessageID != "export-message-1" {
		t.Fatalf("unexpected delivery evidence: %v (%v)", evidence, err)
	}
	if len(paths) != 8 || paths[2] != "/a2a/prod/acme/federation/envelopes:import" || paths[3] != "/a2a/prod/acme/federation/agents/maya/v1/message:send" || paths[7] != "/v1/governance/federation/deliveries" {
		t.Fatalf("unexpected paths: %v", paths)
	}
	for i, action := range []string{"send", "observe", "replay"} {
		expected := "/a2a/prod/acme/agents/maya/v1/tasks/" + sourceTask + "/federation/exports/remote-triage/message:" + action
		if paths[i+4] != expected {
			t.Fatalf("unexpected export path: %s", paths[i+4])
		}
	}
	if len(calls) != 7 {
		t.Fatalf("got %d calls", len(calls))
	}
	if !bytes.Equal(compactJSON(t, calls[0]), compactJSON(t, wire["publication"])) {
		t.Fatal("publication wire changed")
	}
	if !bytes.Equal(compactJSON(t, calls[1]), compactJSON(t, wire["revocation"])) {
		t.Fatal("revocation wire changed")
	}
	var importBody map[string]json.RawMessage
	if err = json.Unmarshal(calls[2], &importBody); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(compactJSON(t, importBody["envelope"]), compactJSON(t, wire["envelope"])) {
		t.Fatal("envelope wire changed")
	}
	var invocationBody map[string]json.RawMessage
	if err = json.Unmarshal(calls[3], &invocationBody); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(compactJSON(t, invocationBody["invocation"]), compactJSON(t, []byte(`{"payload":"signed-payload","signature":"`+string(bytes.Repeat([]byte("a"), 128))+`"}`))) {
		t.Fatal("invocation wire changed")
	}
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (fn roundTripFunc) RoundTrip(request *http.Request) (*http.Response, error) {
	return fn(request)
}

func compactJSON(t *testing.T, value []byte) []byte {
	t.Helper()
	var buffer bytes.Buffer
	if err := json.Compact(&buffer, value); err != nil {
		t.Fatal(err)
	}
	return buffer.Bytes()
}
