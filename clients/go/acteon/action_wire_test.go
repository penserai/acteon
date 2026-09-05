package acteon

import (
	"encoding/json"
	"os"
	"reflect"
	"testing"
)

func TestActionWireMetadata(t *testing.T) {
	data, err := os.ReadFile("../../contract-fixtures/action-wire.json")
	if err != nil {
		t.Fatal(err)
	}
	var action Action
	if err := json.Unmarshal(data, &action); err != nil {
		t.Fatal(err)
	}
	if action.Metadata.Labels["owner"] != "alice" {
		t.Fatal("metadata was not flattened")
	}
	wire, err := json.Marshal(action)
	if err != nil {
		t.Fatal(err)
	}
	var expected, actual map[string]any
	if err := json.Unmarshal(data, &expected); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(wire, &actual); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(expected, actual) {
		t.Fatalf("wire mismatch: %s", wire)
	}
}
