package acteon

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/url"
	"regexp"
)

const AgentSourceContextHeader = "x-acteon-agent-source-context"

var agentSourcePattern = regexp.MustCompile(`^[A-Za-z0-9_-]+$`)
var agentAttemptPattern = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`)
var agentProofPattern = regexp.MustCompile(`^[0-9a-f]{64}$`)

// AgentServiceReceipt belongs in host state, separately from model data.
// TaskID is the original accepted identity, independent of mutable Task data.
type AgentServiceReceipt struct {
	Namespace     string         `json:"namespace"`
	Tenant        string         `json:"tenant"`
	Agent         string         `json:"agent"`
	TaskID        string         `json:"task_id"`
	SourceContext string         `json:"source_context"`
	Task          map[string]any `json:"task"`
}

func agentSource(value string) error {
	if len(value) == 0 || len(value) > 8192 || !agentSourcePattern.MatchString(value) {
		return fmt.Errorf("agent service source context missing or malformed")
	}
	return nil
}
func agentSegment(value string) (string, error) {
	if value == "" || value == "." || value == ".." {
		return "", fmt.Errorf("invalid agent service path segment")
	}
	return url.PathEscape(value), nil
}
func agentServiceBase(namespace, tenant, agent string) (string, error) {
	n, err := agentSegment(namespace)
	if err != nil {
		return "", err
	}
	t, err := agentSegment(tenant)
	if err != nil {
		return "", err
	}
	a, err := agentSegment(agent)
	if err != nil {
		return "", err
	}
	return "/a2a/" + n + "/" + t + "/agents/" + a + "/v1", nil
}
func (c *Client) agentServiceRequest(ctx context.Context, method, path string, body any, source string) (map[string]any, string, error) {
	headers := map[string]string{A2AVersionHeader: A2AProtocolVersion}
	if source != "" {
		headers[AgentSourceContextHeader] = source
	}
	resp, err := c.doRequestExt(ctx, method, path, body, requestOpts{extraHeaders: headers, noRedirect: true})
	if err != nil {
		return nil, "", err
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, "", &ConnectionError{Message: err.Error()}
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, "", &HTTPError{Status: resp.StatusCode, Message: string(data)}
	}
	if resp.Header.Get(A2AVersionHeader) != A2AProtocolVersion {
		return nil, "", fmt.Errorf("agent service response version missing or unsupported")
	}
	var task map[string]any
	if err := json.Unmarshal(data, &task); err != nil {
		return nil, "", err
	}
	return task, resp.Header.Get(AgentSourceContextHeader), nil
}
func agentTask(task map[string]any, namespace, tenant, expected string) (string, error) {
	id, ok := task["id"].(string)
	if !ok || id == "" || task["namespace"] != namespace || task["tenant"] != tenant || (expected != "" && id != expected) {
		return "", fmt.Errorf("agent service task identity mismatch")
	}
	return id, nil
}

// AgentServiceSendMessage submits once. Preserve message identity after response loss.
func (c *Client) AgentServiceSendMessage(ctx context.Context, namespace, tenant, agent string, message map[string]any) (*AgentServiceReceipt, error) {
	path, err := agentServiceBase(namespace, tenant, agent)
	if err != nil {
		return nil, err
	}
	task, source, err := c.agentServiceRequest(ctx, "POST", path+"/message:send", map[string]any{"message": message}, "")
	if err != nil {
		return nil, err
	}
	if err := agentSource(source); err != nil {
		return nil, err
	}
	id, err := agentTask(task, namespace, tenant, "")
	if err != nil {
		return nil, err
	}
	return &AgentServiceReceipt{Namespace: namespace, Tenant: tenant, Agent: agent, TaskID: id, SourceContext: source, Task: task}, nil
}

// AgentServiceGetTask observes one retained job without starting provider work.
func (c *Client) AgentServiceGetTask(ctx context.Context, receipt *AgentServiceReceipt) (map[string]any, error) {
	if receipt == nil {
		return nil, fmt.Errorf("agent service receipt required")
	}
	if err := agentSource(receipt.SourceContext); err != nil {
		return nil, err
	}
	path, err := agentServiceBase(receipt.Namespace, receipt.Tenant, receipt.Agent)
	if err != nil {
		return nil, err
	}
	id, err := agentSegment(receipt.TaskID)
	if err != nil {
		return nil, err
	}
	task, _, err := c.agentServiceRequest(ctx, "GET", path+"/tasks/"+id, nil, receipt.SourceContext)
	if err != nil {
		return nil, err
	}
	if _, err := agentTask(task, receipt.Namespace, receipt.Tenant, receipt.TaskID); err != nil {
		return nil, err
	}
	return task, nil
}

// AgentServiceProviderAbort reports provider finality separately from restriction.
type AgentServiceProviderAbort struct {
	State       string `json:"state"`
	AttemptID   string `json:"attempt_id,omitempty"`
	ProofDigest string `json:"proof_digest,omitempty"`
}

func agentProviderAbort(value any) (*AgentServiceProviderAbort, error) {
	if value == nil {
		return nil, nil
	}
	raw, ok := value.(map[string]any)
	if !ok {
		return nil, fmt.Errorf("agent service provider abort status malformed")
	}
	state, ok := raw["state"].(string)
	if !ok {
		return nil, fmt.Errorf("agent service provider abort status malformed")
	}
	if state == "restricted_only" && len(raw) == 1 {
		return &AgentServiceProviderAbort{State: state}, nil
	}
	if id, ok := raw["attempt_id"].(string); state == "uncertain" && ok && len(raw) == 2 && agentAttemptPattern.MatchString(id) {
		return &AgentServiceProviderAbort{State: state, AttemptID: id}, nil
	}
	if digest, ok := raw["proof_digest"].(string); state == "reconciled" && ok && len(raw) == 2 && agentProofPattern.MatchString(digest) {
		return &AgentServiceProviderAbort{State: state, ProofDigest: digest}, nil
	}
	return nil, fmt.Errorf("agent service provider abort status malformed")
}

// AgentServiceStopReceipt acknowledges a durable restriction on future starts.
// Task remains actual provider evidence; ProviderAbort reports separate finality.
type AgentServiceStopReceipt struct {
	Task                map[string]any             `json:"task"`
	FutureStartsBlocked bool                       `json:"future_starts_blocked"`
	ProviderAbort       *AgentServiceProviderAbort `json:"provider_abort,omitempty"`
}

// AgentServiceStopTask stops future starts for the original retained job.
// After response loss, explicitly retry with the same receipt.
func (c *Client) AgentServiceStopTask(ctx context.Context, receipt *AgentServiceReceipt) (*AgentServiceStopReceipt, error) {
	if receipt == nil {
		return nil, fmt.Errorf("agent service receipt required")
	}
	if err := agentSource(receipt.SourceContext); err != nil {
		return nil, err
	}
	path, err := agentServiceBase(receipt.Namespace, receipt.Tenant, receipt.Agent)
	if err != nil {
		return nil, err
	}
	id, err := agentSegment(receipt.TaskID)
	if err != nil {
		return nil, err
	}
	value, _, err := c.agentServiceRequest(ctx, "POST", path+"/tasks/"+id+"/stop", nil, receipt.SourceContext)
	if err != nil {
		return nil, err
	}
	blocked, ok := value["future_starts_blocked"].(bool)
	if !ok || !blocked {
		return nil, fmt.Errorf("agent service stop acknowledgement missing or malformed")
	}
	task, ok := value["task"].(map[string]any)
	if !ok {
		return nil, fmt.Errorf("agent service task identity mismatch")
	}
	if _, err := agentTask(task, receipt.Namespace, receipt.Tenant, receipt.TaskID); err != nil {
		return nil, err
	}
	abort, err := agentProviderAbort(value["provider_abort"])
	if err != nil {
		return nil, err
	}
	return &AgentServiceStopReceipt{Task: task, FutureStartsBlocked: true, ProviderAbort: abort}, nil
}
