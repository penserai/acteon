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

func TestProviderHealthStatus(t *testing.T) {
	status := ProviderHealthStatus{
		Provider:            "email",
		Healthy:             true,
		CircuitBreakerState: "closed",
		TotalRequests:       1500,
		Successes:           1480,
		Failures:            20,
		SuccessRate:         98.67,
		AvgLatencyMs:        45.2,
		P50LatencyMs:        32.0,
		P95LatencyMs:        120.5,
		P99LatencyMs:        250.0,
	}

	if status.Provider != "email" {
		t.Errorf("expected provider email, got %s", status.Provider)
	}
	if !status.Healthy {
		t.Error("expected healthy to be true")
	}
	if status.CircuitBreakerState != "closed" {
		t.Errorf("expected circuit breaker state closed, got %s", status.CircuitBreakerState)
	}
	if status.TotalRequests != 1500 {
		t.Errorf("expected total requests 1500, got %d", status.TotalRequests)
	}
	if status.SuccessRate != 98.67 {
		t.Errorf("expected success rate 98.67, got %f", status.SuccessRate)
	}
}

func TestListProviderHealthResponse(t *testing.T) {
	response := ListProviderHealthResponse{
		Providers: []ProviderHealthStatus{
			{
				Provider:            "email",
				Healthy:             true,
				CircuitBreakerState: "closed",
				TotalRequests:       1000,
				Successes:           990,
				Failures:            10,
				SuccessRate:         99.0,
				AvgLatencyMs:        50.0,
				P50LatencyMs:        40.0,
				P95LatencyMs:        100.0,
				P99LatencyMs:        150.0,
			},
			{
				Provider:            "slack",
				Healthy:             false,
				CircuitBreakerState: "open",
				TotalRequests:       500,
				Successes:           450,
				Failures:            50,
				SuccessRate:         90.0,
				AvgLatencyMs:        200.0,
				P50LatencyMs:        150.0,
				P95LatencyMs:        400.0,
				P99LatencyMs:        600.0,
			},
		},
	}

	if len(response.Providers) != 2 {
		t.Errorf("expected 2 providers, got %d", len(response.Providers))
	}
	if response.Providers[0].Provider != "email" {
		t.Errorf("expected first provider to be email, got %s", response.Providers[0].Provider)
	}
	if response.Providers[1].Provider != "slack" {
		t.Errorf("expected second provider to be slack, got %s", response.Providers[1].Provider)
	}
	if response.Providers[0].Healthy != true {
		t.Error("expected first provider to be healthy")
	}
	if response.Providers[1].Healthy != false {
		t.Error("expected second provider to be unhealthy")
	}
}
