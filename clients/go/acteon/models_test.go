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

// ---------------------------------------------------------------------------
// WASM Plugin types
// ---------------------------------------------------------------------------

func TestWasmPluginConfig(t *testing.T) {
	memLimit := int64(16777216)
	timeout := int64(100)
	config := WasmPluginConfig{
		MemoryLimitBytes:     &memLimit,
		TimeoutMs:            &timeout,
		AllowedHostFunctions: []string{"log", "time"},
	}

	if *config.MemoryLimitBytes != 16777216 {
		t.Errorf("expected memory_limit_bytes 16777216, got %d", *config.MemoryLimitBytes)
	}
	if *config.TimeoutMs != 100 {
		t.Errorf("expected timeout_ms 100, got %d", *config.TimeoutMs)
	}
	if len(config.AllowedHostFunctions) != 2 {
		t.Errorf("expected 2 allowed host functions, got %d", len(config.AllowedHostFunctions))
	}
}

func TestWasmPluginConfigEmpty(t *testing.T) {
	config := WasmPluginConfig{}
	if config.MemoryLimitBytes != nil {
		t.Error("expected nil memory_limit_bytes")
	}
	if config.TimeoutMs != nil {
		t.Error("expected nil timeout_ms")
	}
	if config.AllowedHostFunctions != nil {
		t.Error("expected nil allowed_host_functions")
	}
}

func TestWasmPlugin(t *testing.T) {
	desc := "A test plugin"
	memLimit := int64(16777216)
	plugin := WasmPlugin{
		Name:        "my-plugin",
		Description: &desc,
		Status:      "active",
		Enabled:     true,
		Config: &WasmPluginConfig{
			MemoryLimitBytes: &memLimit,
		},
		CreatedAt:       "2026-02-15T00:00:00Z",
		UpdatedAt:       "2026-02-15T01:00:00Z",
		InvocationCount: 42,
	}

	if plugin.Name != "my-plugin" {
		t.Errorf("expected name my-plugin, got %s", plugin.Name)
	}
	if *plugin.Description != "A test plugin" {
		t.Errorf("expected description, got %s", *plugin.Description)
	}
	if plugin.Status != "active" {
		t.Errorf("expected status active, got %s", plugin.Status)
	}
	if !plugin.Enabled {
		t.Error("expected enabled to be true")
	}
	if plugin.Config == nil || *plugin.Config.MemoryLimitBytes != 16777216 {
		t.Error("expected config with memory limit")
	}
	if plugin.InvocationCount != 42 {
		t.Errorf("expected invocation count 42, got %d", plugin.InvocationCount)
	}
}

func TestWasmPluginMinimal(t *testing.T) {
	plugin := WasmPlugin{
		Name:      "minimal-plugin",
		Status:    "active",
		CreatedAt: "2026-02-15T00:00:00Z",
		UpdatedAt: "2026-02-15T00:00:00Z",
	}

	if plugin.Name != "minimal-plugin" {
		t.Errorf("expected name minimal-plugin, got %s", plugin.Name)
	}
	if plugin.Description != nil {
		t.Error("expected nil description")
	}
	if plugin.Config != nil {
		t.Error("expected nil config")
	}
	if plugin.InvocationCount != 0 {
		t.Errorf("expected invocation count 0, got %d", plugin.InvocationCount)
	}
}

func TestRegisterPluginRequest(t *testing.T) {
	memLimit := int64(1024)
	req := RegisterPluginRequest{
		Name:        "test-plugin",
		Description: "A test",
		WasmPath:    "/plugins/test.wasm",
		Config: &WasmPluginConfig{
			MemoryLimitBytes: &memLimit,
		},
	}

	if req.Name != "test-plugin" {
		t.Errorf("expected name test-plugin, got %s", req.Name)
	}
	if req.Description != "A test" {
		t.Errorf("expected description, got %s", req.Description)
	}
	if req.WasmPath != "/plugins/test.wasm" {
		t.Errorf("expected wasm_path, got %s", req.WasmPath)
	}
	if req.Config == nil || *req.Config.MemoryLimitBytes != 1024 {
		t.Error("expected config with memory limit 1024")
	}
}

func TestListPluginsResponse(t *testing.T) {
	response := ListPluginsResponse{
		Plugins: []WasmPlugin{
			{Name: "plugin-a", Status: "active", Enabled: true, CreatedAt: "2026-02-15T00:00:00Z", UpdatedAt: "2026-02-15T00:00:00Z"},
			{Name: "plugin-b", Status: "disabled", Enabled: false, CreatedAt: "2026-02-15T00:00:00Z", UpdatedAt: "2026-02-15T00:00:00Z"},
		},
		Count: 2,
	}

	if len(response.Plugins) != 2 {
		t.Errorf("expected 2 plugins, got %d", len(response.Plugins))
	}
	if response.Count != 2 {
		t.Errorf("expected count 2, got %d", response.Count)
	}
	if response.Plugins[0].Name != "plugin-a" {
		t.Errorf("expected plugin-a, got %s", response.Plugins[0].Name)
	}
	if !response.Plugins[0].Enabled {
		t.Error("expected plugin-a to be enabled")
	}
	if response.Plugins[1].Enabled {
		t.Error("expected plugin-b to be disabled")
	}
}

func TestPluginInvocationRequest(t *testing.T) {
	req := PluginInvocationRequest{
		Input:    map[string]any{"key": "value"},
		Function: "custom_fn",
	}

	if req.Function != "custom_fn" {
		t.Errorf("expected function custom_fn, got %s", req.Function)
	}
	if req.Input["key"] != "value" {
		t.Errorf("expected input key=value, got %v", req.Input["key"])
	}
}

func TestPluginInvocationResponse(t *testing.T) {
	msg := "all good"
	dur := 12.5
	resp := PluginInvocationResponse{
		Verdict:    true,
		Message:    &msg,
		Metadata:   map[string]any{"score": 0.95},
		DurationMs: &dur,
	}

	if !resp.Verdict {
		t.Error("expected verdict to be true")
	}
	if *resp.Message != "all good" {
		t.Errorf("expected message, got %s", *resp.Message)
	}
	if resp.Metadata["score"] != 0.95 {
		t.Errorf("expected score 0.95, got %v", resp.Metadata["score"])
	}
	if *resp.DurationMs != 12.5 {
		t.Errorf("expected duration 12.5, got %f", *resp.DurationMs)
	}
}

func TestPluginInvocationResponseMinimal(t *testing.T) {
	resp := PluginInvocationResponse{
		Verdict: false,
	}

	if resp.Verdict {
		t.Error("expected verdict to be false")
	}
	if resp.Message != nil {
		t.Error("expected nil message")
	}
	if resp.Metadata != nil {
		t.Error("expected nil metadata")
	}
	if resp.DurationMs != nil {
		t.Error("expected nil duration_ms")
	}
}

// ---------------------------------------------------------------------------
// AWS EC2 Provider Payload Helpers
// ---------------------------------------------------------------------------

func TestNewEc2StartInstancesPayload(t *testing.T) {
	p := NewEc2StartInstancesPayload([]string{"i-abc123", "i-def456"})
	ids := p["instance_ids"].([]string)
	if len(ids) != 2 || ids[0] != "i-abc123" || ids[1] != "i-def456" {
		t.Errorf("unexpected instance_ids: %v", ids)
	}
}

func TestNewEc2StopInstancesPayload(t *testing.T) {
	p := NewEc2StopInstancesPayload([]string{"i-abc123"})
	ids := p["instance_ids"].([]string)
	if len(ids) != 1 || ids[0] != "i-abc123" {
		t.Errorf("unexpected instance_ids: %v", ids)
	}
	if _, ok := p["hibernate"]; ok {
		t.Error("expected no hibernate key in basic payload")
	}
	if _, ok := p["force"]; ok {
		t.Error("expected no force key in basic payload")
	}
}

func TestNewEc2StopInstancesPayloadWithOptions(t *testing.T) {
	p := NewEc2StopInstancesPayloadWithOptions([]string{"i-abc123"}, true, true)
	if p["hibernate"] != true {
		t.Errorf("expected hibernate=true, got %v", p["hibernate"])
	}
	if p["force"] != true {
		t.Errorf("expected force=true, got %v", p["force"])
	}
}

func TestNewEc2RebootInstancesPayload(t *testing.T) {
	p := NewEc2RebootInstancesPayload([]string{"i-abc123"})
	ids := p["instance_ids"].([]string)
	if len(ids) != 1 || ids[0] != "i-abc123" {
		t.Errorf("unexpected instance_ids: %v", ids)
	}
}

func TestNewEc2TerminateInstancesPayload(t *testing.T) {
	p := NewEc2TerminateInstancesPayload([]string{"i-abc123", "i-def456"})
	ids := p["instance_ids"].([]string)
	if len(ids) != 2 {
		t.Errorf("expected 2 instance IDs, got %d", len(ids))
	}
}

func TestNewEc2HibernateInstancesPayload(t *testing.T) {
	p := NewEc2HibernateInstancesPayload([]string{"i-abc123"})
	ids := p["instance_ids"].([]string)
	if len(ids) != 1 || ids[0] != "i-abc123" {
		t.Errorf("unexpected instance_ids: %v", ids)
	}
}

func TestNewEc2RunInstancesPayload(t *testing.T) {
	p := NewEc2RunInstancesPayload("ami-12345678", "t3.micro")
	if p["image_id"] != "ami-12345678" {
		t.Errorf("expected image_id ami-12345678, got %v", p["image_id"])
	}
	if p["instance_type"] != "t3.micro" {
		t.Errorf("expected instance_type t3.micro, got %v", p["instance_type"])
	}
	if _, ok := p["min_count"]; ok {
		t.Error("expected no min_count in basic payload")
	}
}

func TestNewEc2RunInstancesPayloadWithOptions(t *testing.T) {
	p := NewEc2RunInstancesPayloadWithOptions(
		"ami-12345678", "t3.large",
		2, 5,
		"my-keypair", "subnet-abc", "IyEvYmluL2Jhc2g=", "my-profile",
		[]string{"sg-111", "sg-222"},
		map[string]string{"Name": "web-server"},
	)
	if p["image_id"] != "ami-12345678" {
		t.Errorf("expected image_id ami-12345678, got %v", p["image_id"])
	}
	if p["min_count"] != 2 {
		t.Errorf("expected min_count 2, got %v", p["min_count"])
	}
	if p["max_count"] != 5 {
		t.Errorf("expected max_count 5, got %v", p["max_count"])
	}
	if p["key_name"] != "my-keypair" {
		t.Errorf("expected key_name my-keypair, got %v", p["key_name"])
	}
	if p["subnet_id"] != "subnet-abc" {
		t.Errorf("expected subnet_id subnet-abc, got %v", p["subnet_id"])
	}
	sgIDs := p["security_group_ids"].([]string)
	if len(sgIDs) != 2 {
		t.Errorf("expected 2 security group IDs, got %d", len(sgIDs))
	}
	tags := p["tags"].(map[string]string)
	if tags["Name"] != "web-server" {
		t.Errorf("expected tag Name=web-server, got %v", tags["Name"])
	}
	if p["iam_instance_profile"] != "my-profile" {
		t.Errorf("expected iam_instance_profile my-profile, got %v", p["iam_instance_profile"])
	}
}

func TestNewEc2AttachVolumePayload(t *testing.T) {
	p := NewEc2AttachVolumePayload("vol-abc123", "i-def456", "/dev/sdf")
	if p["volume_id"] != "vol-abc123" {
		t.Errorf("expected volume_id vol-abc123, got %v", p["volume_id"])
	}
	if p["instance_id"] != "i-def456" {
		t.Errorf("expected instance_id i-def456, got %v", p["instance_id"])
	}
	if p["device"] != "/dev/sdf" {
		t.Errorf("expected device /dev/sdf, got %v", p["device"])
	}
}

func TestNewEc2DetachVolumePayload(t *testing.T) {
	p := NewEc2DetachVolumePayload("vol-abc123")
	if p["volume_id"] != "vol-abc123" {
		t.Errorf("expected volume_id vol-abc123, got %v", p["volume_id"])
	}
	if _, ok := p["instance_id"]; ok {
		t.Error("expected no instance_id in basic payload")
	}
}

func TestNewEc2DetachVolumePayloadWithOptions(t *testing.T) {
	p := NewEc2DetachVolumePayloadWithOptions("vol-abc123", "i-def456", "/dev/sdf", true)
	if p["volume_id"] != "vol-abc123" {
		t.Errorf("expected volume_id vol-abc123, got %v", p["volume_id"])
	}
	if p["instance_id"] != "i-def456" {
		t.Errorf("expected instance_id i-def456, got %v", p["instance_id"])
	}
	if p["device"] != "/dev/sdf" {
		t.Errorf("expected device /dev/sdf, got %v", p["device"])
	}
	if p["force"] != true {
		t.Errorf("expected force=true, got %v", p["force"])
	}
}

func TestNewEc2DescribeInstancesPayload(t *testing.T) {
	p := NewEc2DescribeInstancesPayload(nil)
	if _, ok := p["instance_ids"]; ok {
		t.Error("expected no instance_ids when nil")
	}

	p = NewEc2DescribeInstancesPayload([]string{"i-abc123"})
	ids := p["instance_ids"].([]string)
	if len(ids) != 1 || ids[0] != "i-abc123" {
		t.Errorf("unexpected instance_ids: %v", ids)
	}
}

// ---------------------------------------------------------------------------
// AWS Auto Scaling Provider Payload Helpers
// ---------------------------------------------------------------------------

func TestNewAsgDescribeGroupsPayload(t *testing.T) {
	p := NewAsgDescribeGroupsPayload(nil)
	if _, ok := p["auto_scaling_group_names"]; ok {
		t.Error("expected no group names when nil")
	}

	p = NewAsgDescribeGroupsPayload([]string{"my-asg-1", "my-asg-2"})
	names := p["auto_scaling_group_names"].([]string)
	if len(names) != 2 {
		t.Errorf("expected 2 group names, got %d", len(names))
	}
}

func TestNewAsgSetDesiredCapacityPayload(t *testing.T) {
	p := NewAsgSetDesiredCapacityPayload("my-asg", 5)
	if p["auto_scaling_group_name"] != "my-asg" {
		t.Errorf("expected group name my-asg, got %v", p["auto_scaling_group_name"])
	}
	if p["desired_capacity"] != 5 {
		t.Errorf("expected desired_capacity 5, got %v", p["desired_capacity"])
	}
	if _, ok := p["honor_cooldown"]; ok {
		t.Error("expected no honor_cooldown in basic payload")
	}
}

func TestNewAsgSetDesiredCapacityPayloadWithOptions(t *testing.T) {
	p := NewAsgSetDesiredCapacityPayloadWithOptions("my-asg", 10, true)
	if p["auto_scaling_group_name"] != "my-asg" {
		t.Errorf("expected group name my-asg, got %v", p["auto_scaling_group_name"])
	}
	if p["desired_capacity"] != 10 {
		t.Errorf("expected desired_capacity 10, got %v", p["desired_capacity"])
	}
	if p["honor_cooldown"] != true {
		t.Errorf("expected honor_cooldown=true, got %v", p["honor_cooldown"])
	}
}

func TestNewAsgUpdateGroupPayload(t *testing.T) {
	p := NewAsgUpdateGroupPayload("my-asg")
	if p["auto_scaling_group_name"] != "my-asg" {
		t.Errorf("expected group name my-asg, got %v", p["auto_scaling_group_name"])
	}
	if _, ok := p["min_size"]; ok {
		t.Error("expected no min_size in basic payload")
	}
}

func TestNewAsgUpdateGroupPayloadWithOptions(t *testing.T) {
	minSize := 1
	maxSize := 10
	desiredCapacity := 5
	defaultCooldown := 300
	healthCheckGracePeriod := 120
	p := NewAsgUpdateGroupPayloadWithOptions(
		"my-asg",
		&minSize, &maxSize, &desiredCapacity, &defaultCooldown,
		"ELB", &healthCheckGracePeriod,
	)
	if p["auto_scaling_group_name"] != "my-asg" {
		t.Errorf("expected group name my-asg, got %v", p["auto_scaling_group_name"])
	}
	if p["min_size"] != 1 {
		t.Errorf("expected min_size 1, got %v", p["min_size"])
	}
	if p["max_size"] != 10 {
		t.Errorf("expected max_size 10, got %v", p["max_size"])
	}
	if p["desired_capacity"] != 5 {
		t.Errorf("expected desired_capacity 5, got %v", p["desired_capacity"])
	}
	if p["default_cooldown"] != 300 {
		t.Errorf("expected default_cooldown 300, got %v", p["default_cooldown"])
	}
	if p["health_check_type"] != "ELB" {
		t.Errorf("expected health_check_type ELB, got %v", p["health_check_type"])
	}
	if p["health_check_grace_period"] != 120 {
		t.Errorf("expected health_check_grace_period 120, got %v", p["health_check_grace_period"])
	}
}
