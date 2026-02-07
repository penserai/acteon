package acteon

import (
	"testing"
)

func TestWebhookPayloadToPayload(t *testing.T) {
	payload := &WebhookPayload{
		URL:    "https://example.com/hook",
		Method: "POST",
		Body:   map[string]any{"message": "hello"},
	}

	result := payload.ToPayload()
	if result["url"] != "https://example.com/hook" {
		t.Errorf("expected url to be https://example.com/hook, got %v", result["url"])
	}
	if result["method"] != "POST" {
		t.Errorf("expected method to be POST, got %v", result["method"])
	}
	body := result["body"].(map[string]any)
	if body["message"] != "hello" {
		t.Errorf("expected body.message to be hello, got %v", body["message"])
	}
	if _, ok := result["headers"]; ok {
		t.Error("expected no headers key when headers is empty")
	}
}

func TestWebhookPayloadToPayloadWithHeaders(t *testing.T) {
	payload := &WebhookPayload{
		URL:     "https://example.com/hook",
		Method:  "PUT",
		Body:    map[string]any{},
		Headers: map[string]string{"X-Custom": "abc"},
	}

	result := payload.ToPayload()
	headers := result["headers"].(map[string]string)
	if headers["X-Custom"] != "abc" {
		t.Errorf("expected header X-Custom=abc, got %v", headers["X-Custom"])
	}
}

func TestNewWebhookAction(t *testing.T) {
	action := NewWebhookAction("ns", "t1", "https://example.com/hook", map[string]any{"key": "value"})

	if action.Namespace != "ns" {
		t.Errorf("expected namespace ns, got %s", action.Namespace)
	}
	if action.Tenant != "t1" {
		t.Errorf("expected tenant t1, got %s", action.Tenant)
	}
	if action.Provider != "webhook" {
		t.Errorf("expected provider webhook, got %s", action.Provider)
	}
	if action.ActionType != "webhook" {
		t.Errorf("expected action_type webhook, got %s", action.ActionType)
	}
	if action.Payload["url"] != "https://example.com/hook" {
		t.Errorf("expected payload url, got %v", action.Payload["url"])
	}
	if action.Payload["method"] != "POST" {
		t.Errorf("expected payload method POST, got %v", action.Payload["method"])
	}
	if action.ID == "" {
		t.Error("expected auto-generated ID")
	}
}

func TestNewWebhookActionWithOptions(t *testing.T) {
	headers := map[string]string{"Authorization": "Bearer tok"}
	action := NewWebhookActionWithOptions("ns", "t1", "https://example.com/hook", "PUT", map[string]any{"data": 123}, headers)

	if action.Payload["method"] != "PUT" {
		t.Errorf("expected method PUT, got %v", action.Payload["method"])
	}
	payloadHeaders := action.Payload["headers"].(map[string]string)
	if payloadHeaders["Authorization"] != "Bearer tok" {
		t.Errorf("expected Authorization header, got %v", payloadHeaders["Authorization"])
	}
}

func TestWithWebhookMethod(t *testing.T) {
	action := NewWebhookAction("ns", "t1", "https://example.com/hook", map[string]any{})
	action.WithWebhookMethod("PATCH")

	if action.Payload["method"] != "PATCH" {
		t.Errorf("expected method PATCH, got %v", action.Payload["method"])
	}
}

func TestWithWebhookHeaders(t *testing.T) {
	action := NewWebhookAction("ns", "t1", "https://example.com/hook", map[string]any{})
	action.WithWebhookHeaders(map[string]string{"X-Key": "val"})

	headers := action.Payload["headers"].(map[string]string)
	if headers["X-Key"] != "val" {
		t.Errorf("expected header X-Key=val, got %v", headers["X-Key"])
	}
}

func TestWebhookActionChaining(t *testing.T) {
	action := NewWebhookAction("ns", "t1", "https://example.com/hook", map[string]any{"msg": "test"}).
		WithWebhookMethod("DELETE").
		WithWebhookHeaders(map[string]string{"X-Trace": "123"}).
		WithDedupKey("dedup-1").
		WithMetadata(map[string]string{"env": "prod"})

	if action.Payload["method"] != "DELETE" {
		t.Errorf("expected method DELETE, got %v", action.Payload["method"])
	}
	if action.DedupKey != "dedup-1" {
		t.Errorf("expected dedup_key dedup-1, got %s", action.DedupKey)
	}
	if action.Metadata == nil || action.Metadata.Labels["env"] != "prod" {
		t.Error("expected metadata with env=prod")
	}
}
