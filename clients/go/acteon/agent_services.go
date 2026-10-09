package acteon

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"net/url"
	"regexp"
	"strings"
	"unicode"
)

const AgentSourceContextHeader = "x-acteon-agent-source-context"
const AgentExecutionContextHeader = "x-acteon-execution-context"

var agentSourcePattern = regexp.MustCompile(`^[A-Za-z0-9_-]+$`)
var agentAttemptPattern = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-5[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`)
var agentProofPattern = regexp.MustCompile(`^[0-9a-f]{64}$`)
var agentPeerTokenPattern = regexp.MustCompile(`^[A-Za-z0-9._-]{1,120}$`)

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

type AgentServiceParent struct {
	ExecutionContext string
	Permits          []PermitReference
}

// AgentPeerSendStatus is the durable outcome of one peer submission.
type AgentPeerSendStatus struct {
	State string         `json:"state"`
	Task  map[string]any `json:"task,omitempty"`
	Code  string         `json:"code,omitempty"`
}

// AgentPeerSendReceipt keeps ambiguity distinct from known rejection.
type AgentPeerSendReceipt struct {
	SubmissionID string              `json:"submission_id"`
	Status       AgentPeerSendStatus `json:"status"`
}

// AgentPeerCancelStatus preserves definitive refusal, a future-start fence,
// ambiguity, and observed finality.
type AgentPeerCancelStatus struct {
	State string         `json:"state"`
	Task  map[string]any `json:"task,omitempty"`
	Code  string         `json:"code,omitempty"`
}

// AgentPeerCancelReceipt identifies the one durable cancellation intent.
type AgentPeerCancelReceipt struct {
	SubmissionID   string                `json:"submission_id"`
	CancellationID string                `json:"cancellation_id"`
	Status         AgentPeerCancelStatus `json:"status"`
}

// AgentPeerContinuationStatus preserves delivery ambiguity and the accepted
// task cursor used to bind any later response.
type AgentPeerContinuationStatus struct {
	State          string         `json:"state"`
	Task           map[string]any `json:"task,omitempty"`
	ProgressCursor string         `json:"progress_cursor,omitempty"`
	Code           string         `json:"code,omitempty"`
}

// AgentPeerContinuationReceipt identifies one durable response attempt.
type AgentPeerContinuationReceipt struct {
	SubmissionID   string                      `json:"submission_id"`
	ContinuationID string                      `json:"continuation_id"`
	Status         AgentPeerContinuationStatus `json:"status"`
}

// AgentPeerAuthorizationStatus preserves ambiguity without exposing credentials.
type AgentPeerAuthorizationStatus struct {
	State          string         `json:"state"`
	Task           map[string]any `json:"task,omitempty"`
	ProgressCursor string         `json:"progress_cursor,omitempty"`
	Code           string         `json:"code,omitempty"`
}

// AgentPeerAuthorizationReceipt identifies one durable remote challenge handoff.
type AgentPeerAuthorizationReceipt struct {
	SubmissionID    string                       `json:"submission_id"`
	AuthorizationID string                       `json:"authorization_id"`
	Status          AgentPeerAuthorizationStatus `json:"status"`
}

// AgentPeerSelectionOption is safe registry data. DescriptionUntrusted is
// untrusted text and the option itself grants no authority.
type AgentPeerSelectionOption struct {
	AgentID              string
	Skill                string
	DescriptionUntrusted *string
	CardVersion          string
	BindingDigest        string
	CheckedAtMS          int64
}

// AgentServiceDiscoverPeers lists current source-authorized registry options.
func (c *Client) AgentServiceDiscoverPeers(ctx context.Context, source *AgentServiceReceipt, skill string) ([]AgentPeerSelectionOption, error) {
	if source == nil {
		return nil, fmt.Errorf("agent service source receipt required")
	}
	if !agentPeerTokenPattern.MatchString(skill) || skill == "*" {
		return nil, fmt.Errorf("invalid exact peer skill")
	}
	base, err := agentServiceBase(source.Namespace, source.Tenant, source.Agent)
	if err != nil {
		return nil, err
	}
	taskID, err := agentSegment(source.TaskID)
	if err != nil {
		return nil, err
	}
	value, _, err := c.agentServiceRequest(ctx, "GET", base+"/tasks/"+taskID+"/peers?skill="+url.QueryEscape(skill), nil, "", nil)
	if err != nil {
		return nil, err
	}
	if len(value) != 1 {
		return nil, fmt.Errorf("agent peer discovery response missing or malformed")
	}
	rawPeers, ok := value["peers"].([]any)
	if !ok || len(rawPeers) > 128 {
		return nil, fmt.Errorf("agent peer discovery response missing or malformed")
	}
	seen := map[string]struct{}{}
	options := make([]AgentPeerSelectionOption, 0, len(rawPeers))
	for _, item := range rawPeers {
		raw, ok := item.(map[string]any)
		if !ok || len(raw) != 6 {
			return nil, fmt.Errorf("agent peer discovery response missing or malformed")
		}
		agentID, agentOK := raw["agent_id"].(string)
		peerSkill, skillOK := raw["skill"].(string)
		cardVersion, versionOK := raw["card_version"].(string)
		digest, digestOK := raw["binding_digest"].(string)
		checked, checkedOK := raw["checked_at_ms"].(float64)
		_, descriptionPresent := raw["description_untrusted"]
		if !agentOK || !agentPeerTokenPattern.MatchString(agentID) || !skillOK || peerSkill != skill || !agentPeerTokenPattern.MatchString(peerSkill) || !versionOK || !agentPeerTokenPattern.MatchString(cardVersion) || !digestOK || !agentProofPattern.MatchString(digest) || !checkedOK || !descriptionPresent || checked < 0 || checked > float64(1<<53) || math.Trunc(checked) != checked {
			return nil, fmt.Errorf("agent peer discovery response missing or malformed")
		}
		if _, duplicate := seen[agentID]; duplicate {
			return nil, fmt.Errorf("agent peer discovery response missing or malformed")
		}
		seen[agentID] = struct{}{}
		var description *string
		if raw["description_untrusted"] != nil {
			text, ok := raw["description_untrusted"].(string)
			if !ok || len(text) > 2048 {
				return nil, fmt.Errorf("agent peer discovery response missing or malformed")
			}
			description = &text
		}
		options = append(options, AgentPeerSelectionOption{AgentID: agentID, Skill: peerSkill, DescriptionUntrusted: description, CardVersion: cardVersion, BindingDigest: digest, CheckedAtMS: int64(checked)})
	}
	return options, nil
}

func (c *Client) agentServiceRequest(ctx context.Context, method, path string, body any, source string, parent *AgentServiceParent) (map[string]any, string, error) {
	headers := map[string]string{A2AVersionHeader: A2AProtocolVersion}
	if source != "" {
		headers[AgentSourceContextHeader] = source
	}
	if parent != nil {
		if err := agentSource(parent.ExecutionContext); err != nil || len(parent.Permits) == 0 {
			return nil, "", fmt.Errorf("delegated agent service parent is malformed")
		}
		permits, err := json.Marshal(parent.Permits)
		if err != nil {
			return nil, "", err
		}
		headers[AgentExecutionContextHeader] = parent.ExecutionContext
		headers["x-acteon-execution-permits"] = string(permits)
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
	return c.AgentServiceSendMessageWithParent(ctx, namespace, tenant, agent, message, nil)
}

// AgentServiceSendMessageWithParent carries existing verified authority into one delegated invocation.
func (c *Client) AgentServiceSendMessageWithParent(ctx context.Context, namespace, tenant, agent string, message map[string]any, parent *AgentServiceParent) (*AgentServiceReceipt, error) {
	path, err := agentServiceBase(namespace, tenant, agent)
	if err != nil {
		return nil, err
	}
	task, source, err := c.agentServiceRequest(ctx, "POST", path+"/message:send", map[string]any{"message": message}, "", parent)
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

// AgentServiceSendPeer submits from an accepted source task to one configured
// peer. The request body contains only the message; authority stays server-side.
func (c *Client) AgentServiceSendPeer(ctx context.Context, source *AgentServiceReceipt, target, skill string, message map[string]any) (*AgentPeerSendReceipt, error) {
	if source == nil {
		return nil, fmt.Errorf("agent service source receipt required")
	}
	base, err := agentServiceBase(source.Namespace, source.Tenant, source.Agent)
	if err != nil {
		return nil, err
	}
	taskID, err := agentSegment(source.TaskID)
	if err != nil {
		return nil, err
	}
	targetID, err := agentSegment(target)
	if err != nil {
		return nil, err
	}
	skillID, err := agentSegment(skill)
	if err != nil {
		return nil, err
	}
	value, _, err := c.agentServiceRequest(ctx, "POST", base+"/tasks/"+taskID+"/peers/"+targetID+"/"+skillID+"/message:send", map[string]any{"message": message}, "", nil)
	if err != nil {
		return nil, err
	}
	return agentPeerReceipt(value, source)
}

func agentPeerReceipt(value map[string]any, source *AgentServiceReceipt) (*AgentPeerSendReceipt, error) {
	if len(value) != 2 {
		return nil, fmt.Errorf("agent peer receipt missing or malformed")
	}
	submission, ok := value["submission_id"].(string)
	if !ok || !agentAttemptPattern.MatchString(submission) {
		return nil, fmt.Errorf("agent peer receipt missing or malformed")
	}
	rawStatus, ok := value["status"].(map[string]any)
	if !ok {
		return nil, fmt.Errorf("agent peer receipt missing or malformed")
	}
	state, ok := rawStatus["state"].(string)
	if !ok {
		return nil, fmt.Errorf("agent peer receipt missing or malformed")
	}
	status := AgentPeerSendStatus{State: state}
	switch state {
	case "uncertain":
		if len(rawStatus) != 1 {
			return nil, fmt.Errorf("agent peer receipt missing or malformed")
		}
	case "accepted":
		task, ok := rawStatus["task"].(map[string]any)
		if !ok || len(rawStatus) != 2 {
			return nil, fmt.Errorf("agent peer receipt missing or malformed")
		}
		if _, err := agentTask(task, source.Namespace, source.Tenant, ""); err != nil {
			return nil, err
		}
		status.Task = task
	case "rejected":
		code, ok := rawStatus["code"].(string)
		if !ok || code == "" || len(code) > 1024 || strings.TrimSpace(code) != code || len(rawStatus) != 2 {
			return nil, fmt.Errorf("agent peer receipt missing or malformed")
		}
		for _, character := range code {
			if unicode.IsControl(character) {
				return nil, fmt.Errorf("agent peer receipt missing or malformed")
			}
		}
		status.Code = code
	default:
		return nil, fmt.Errorf("agent peer receipt missing or malformed")
	}
	return &AgentPeerSendReceipt{SubmissionID: submission, Status: status}, nil
}

// AgentServiceRefreshPeer observes and journals an accepted remote task. It
// never resubmits the original message.
func (c *Client) AgentServiceRefreshPeer(ctx context.Context, source *AgentServiceReceipt, target, skill string, peer *AgentPeerSendReceipt) (*AgentPeerSendReceipt, error) {
	if source == nil || peer == nil {
		return nil, fmt.Errorf("agent peer source and receipt required")
	}
	if !agentAttemptPattern.MatchString(peer.SubmissionID) {
		return nil, fmt.Errorf("invalid agent peer submission identity")
	}
	if _, err := agentTask(peer.Status.Task, source.Namespace, source.Tenant, ""); err != nil {
		return nil, err
	}
	base, err := agentServiceBase(source.Namespace, source.Tenant, source.Agent)
	if err != nil {
		return nil, err
	}
	taskID, err := agentSegment(source.TaskID)
	if err != nil {
		return nil, err
	}
	targetID, err := agentSegment(target)
	if err != nil {
		return nil, err
	}
	skillID, err := agentSegment(skill)
	if err != nil {
		return nil, err
	}
	value, _, err := c.agentServiceRequest(ctx, "POST", base+"/tasks/"+taskID+"/peers/"+targetID+"/"+skillID+"/submissions/"+peer.SubmissionID+":refresh", nil, "", nil)
	if err != nil {
		return nil, err
	}
	refreshed, err := agentPeerReceipt(value, source)
	if err != nil {
		return nil, err
	}
	if refreshed.SubmissionID != peer.SubmissionID {
		return nil, fmt.Errorf("agent peer submission identity mismatch")
	}
	sameDisposition := peer.Status.State == refreshed.Status.State
	if sameDisposition {
		switch peer.Status.State {
		case "accepted":
			sameDisposition = peer.Status.Task["id"] == refreshed.Status.Task["id"]
		case "rejected":
			sameDisposition = peer.Status.Code == refreshed.Status.Code
		case "uncertain":
		default:
			sameDisposition = false
		}
	}
	if !sameDisposition {
		return nil, fmt.Errorf("agent peer refresh changed durable disposition")
	}
	return refreshed, nil
}

// AgentServiceCancelPeer persists and delivers at most one remote cancellation.
// An uncertain result is never retried automatically.
func (c *Client) AgentServiceCancelPeer(ctx context.Context, source *AgentServiceReceipt, target, skill string, peer *AgentPeerSendReceipt) (*AgentPeerCancelReceipt, error) {
	if source == nil || peer == nil || peer.Status.State != "accepted" || peer.Status.Task == nil {
		return nil, fmt.Errorf("agent peer cancellation requires an accepted peer receipt")
	}
	if !agentAttemptPattern.MatchString(peer.SubmissionID) {
		return nil, fmt.Errorf("invalid agent peer submission identity")
	}
	base, err := agentServiceBase(source.Namespace, source.Tenant, source.Agent)
	if err != nil {
		return nil, err
	}
	taskID, err := agentSegment(source.TaskID)
	if err != nil {
		return nil, err
	}
	targetID, err := agentSegment(target)
	if err != nil {
		return nil, err
	}
	skillID, err := agentSegment(skill)
	if err != nil {
		return nil, err
	}
	value, _, err := c.agentServiceRequest(ctx, "POST", base+"/tasks/"+taskID+"/peers/"+targetID+"/"+skillID+"/submissions/"+peer.SubmissionID+":cancel", nil, "", nil)
	if err != nil {
		return nil, err
	}
	if len(value) != 3 || value["submission_id"] != peer.SubmissionID {
		return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
	}
	cancellationID, ok := value["cancellation_id"].(string)
	if !ok || !agentAttemptPattern.MatchString(cancellationID) {
		return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
	}
	rawStatus, ok := value["status"].(map[string]any)
	if !ok {
		return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
	}
	state, ok := rawStatus["state"].(string)
	if !ok {
		return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
	}
	status := AgentPeerCancelStatus{State: state}
	switch state {
	case "unsupported", "uncertain":
		if len(rawStatus) != 1 {
			return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
		}
	case "rejected":
		code, ok := rawStatus["code"].(string)
		if !ok || code == "" || len(code) > 1024 || strings.TrimSpace(code) != code || len(rawStatus) != 2 {
			return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
		}
		for _, character := range code {
			if unicode.IsControl(character) {
				return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
			}
		}
		status.Code = code
	case "restricted", "reconciled":
		task, ok := rawStatus["task"].(map[string]any)
		expected, _ := peer.Status.Task["id"].(string)
		if !ok || len(rawStatus) != 2 || expected == "" {
			return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
		}
		if _, err := agentTask(task, source.Namespace, source.Tenant, expected); err != nil {
			return nil, err
		}
		taskStatus, ok := task["status"].(map[string]any)
		if !ok {
			return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
		}
		taskState, ok := taskStatus["state"].(string)
		if !ok || (taskState != "submitted" && taskState != "working" && taskState != "completed" && taskState != "failed" && taskState != "canceled" && taskState != "input_required" && taskState != "auth_required" && taskState != "rejected") {
			return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
		}
		terminal := taskState == "completed" || taskState == "failed" || taskState == "canceled" || taskState == "rejected"
		if (state == "restricted" && terminal) || (state == "reconciled" && !terminal) {
			return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
		}
		status.Task = task
	default:
		return nil, fmt.Errorf("agent peer cancellation receipt missing or malformed")
	}
	return &AgentPeerCancelReceipt{SubmissionID: peer.SubmissionID, CancellationID: cancellationID, Status: status}, nil
}

// AgentServiceContinuePeer delivers one unbound user response to the accepted
// remote task's current input challenge.
func (c *Client) AgentServiceContinuePeer(ctx context.Context, source *AgentServiceReceipt, target, skill string, peer *AgentPeerSendReceipt, message map[string]any) (*AgentPeerContinuationReceipt, error) {
	if source == nil || peer == nil || peer.Status.State != "accepted" || peer.Status.Task == nil || !agentAttemptPattern.MatchString(peer.SubmissionID) {
		return nil, fmt.Errorf("agent peer continuation requires an accepted peer receipt")
	}
	acceptedID, err := agentTask(peer.Status.Task, source.Namespace, source.Tenant, "")
	if err != nil {
		return nil, err
	}
	challengeBound := false
	if metadata := message["metadata"]; metadata != nil {
		encoded, encodeErr := json.Marshal(metadata)
		var normalized map[string]any
		if encodeErr == nil && json.Unmarshal(encoded, &normalized) == nil {
			_, challengeBound = normalized["acteon.challengeId"]
		}
	}
	if message["role"] != "user" || message["taskId"] != nil || message["contextId"] != nil || challengeBound {
		return nil, fmt.Errorf("agent peer continuation requires an unbound user response")
	}
	base, err := agentServiceBase(source.Namespace, source.Tenant, source.Agent)
	if err != nil {
		return nil, err
	}
	taskID, err := agentSegment(source.TaskID)
	if err != nil {
		return nil, err
	}
	targetID, err := agentSegment(target)
	if err != nil {
		return nil, err
	}
	skillID, err := agentSegment(skill)
	if err != nil {
		return nil, err
	}
	path := base + "/tasks/" + taskID + "/peers/" + targetID + "/" + skillID + "/submissions/" + peer.SubmissionID + "/message:send"
	value, _, err := c.agentServiceRequest(ctx, "POST", path, map[string]any{"message": message}, "", nil)
	if err != nil {
		return nil, err
	}
	if len(value) != 3 || value["submission_id"] != peer.SubmissionID {
		return nil, fmt.Errorf("agent peer continuation receipt missing or malformed")
	}
	continuationID, ok := value["continuation_id"].(string)
	if !ok || !agentAttemptPattern.MatchString(continuationID) {
		return nil, fmt.Errorf("agent peer continuation receipt missing or malformed")
	}
	raw, ok := value["status"].(map[string]any)
	if !ok {
		return nil, fmt.Errorf("agent peer continuation receipt missing or malformed")
	}
	state, ok := raw["state"].(string)
	if !ok {
		return nil, fmt.Errorf("agent peer continuation receipt missing or malformed")
	}
	status := AgentPeerContinuationStatus{State: state}
	switch state {
	case "uncertain":
		if len(raw) != 1 {
			return nil, fmt.Errorf("agent peer continuation receipt missing or malformed")
		}
	case "rejected":
		code, ok := raw["code"].(string)
		if !ok || code == "" || len(code) > 1024 || strings.TrimSpace(code) != code || len(raw) != 2 {
			return nil, fmt.Errorf("agent peer continuation receipt missing or malformed")
		}
		for _, character := range code {
			if unicode.IsControl(character) {
				return nil, fmt.Errorf("agent peer continuation receipt missing or malformed")
			}
		}
		status.Code = code
	case "accepted":
		task, taskOK := raw["task"].(map[string]any)
		cursor, cursorOK := raw["progress_cursor"].(string)
		if !taskOK || !cursorOK || len(raw) != 3 || !validAgentProgressCursor(cursor) {
			return nil, fmt.Errorf("agent peer continuation receipt missing or malformed")
		}
		if _, err := agentTask(task, source.Namespace, source.Tenant, acceptedID); err != nil {
			return nil, err
		}
		status.Task, status.ProgressCursor = task, cursor
	default:
		return nil, fmt.Errorf("agent peer continuation receipt missing or malformed")
	}
	return &AgentPeerContinuationReceipt{SubmissionID: peer.SubmissionID, ContinuationID: continuationID, Status: status}, nil
}

// AgentServiceAuthorizePeer asks the target host to resolve an exact challenge.
// Credentials and verifier evidence never enter this call.
func (c *Client) AgentServiceAuthorizePeer(ctx context.Context, source *AgentServiceReceipt, target, skill string, peer *AgentPeerSendReceipt, challengeID string) (*AgentPeerAuthorizationReceipt, error) {
	if source == nil || peer == nil || peer.Status.State != "accepted" || peer.Status.Task == nil || !agentAttemptPattern.MatchString(peer.SubmissionID) || challengeID == "" || len(challengeID) > 1024 || strings.TrimSpace(challengeID) != challengeID || challengeID == "*" {
		return nil, fmt.Errorf("agent peer authorization requires an accepted peer and challenge")
	}
	for _, character := range challengeID {
		if unicode.IsControl(character) {
			return nil, fmt.Errorf("agent peer authorization requires an accepted peer and challenge")
		}
	}
	acceptedID, err := agentTask(peer.Status.Task, source.Namespace, source.Tenant, "")
	if err != nil {
		return nil, err
	}
	base, err := agentServiceBase(source.Namespace, source.Tenant, source.Agent)
	if err != nil {
		return nil, err
	}
	taskID, err := agentSegment(source.TaskID)
	if err != nil {
		return nil, err
	}
	targetID, err := agentSegment(target)
	if err != nil {
		return nil, err
	}
	skillID, err := agentSegment(skill)
	if err != nil {
		return nil, err
	}
	path := base + "/tasks/" + taskID + "/peers/" + targetID + "/" + skillID + "/submissions/" + peer.SubmissionID + "/authorization:resolve"
	value, _, err := c.agentServiceRequest(ctx, "POST", path, map[string]any{"challengeId": challengeID}, "", nil)
	if err != nil {
		return nil, err
	}
	if len(value) != 3 || value["submission_id"] != peer.SubmissionID {
		return nil, fmt.Errorf("agent peer authorization receipt missing or malformed")
	}
	authorizationID, ok := value["authorization_id"].(string)
	if !ok || !agentAttemptPattern.MatchString(authorizationID) {
		return nil, fmt.Errorf("agent peer authorization receipt missing or malformed")
	}
	raw, ok := value["status"].(map[string]any)
	if !ok {
		return nil, fmt.Errorf("agent peer authorization receipt missing or malformed")
	}
	state, ok := raw["state"].(string)
	if !ok {
		return nil, fmt.Errorf("agent peer authorization receipt missing or malformed")
	}
	status := AgentPeerAuthorizationStatus{State: state}
	switch state {
	case "uncertain":
		if len(raw) != 1 {
			return nil, fmt.Errorf("agent peer authorization receipt missing or malformed")
		}
	case "rejected":
		code, ok := raw["code"].(string)
		if !ok || code == "" || len(code) > 1024 || strings.TrimSpace(code) != code || len(raw) != 2 {
			return nil, fmt.Errorf("agent peer authorization receipt missing or malformed")
		}
		for _, character := range code {
			if unicode.IsControl(character) {
				return nil, fmt.Errorf("agent peer authorization receipt missing or malformed")
			}
		}
		status.Code = code
	case "resolved":
		task, taskOK := raw["task"].(map[string]any)
		cursor, cursorOK := raw["progress_cursor"].(string)
		if !taskOK || !cursorOK || len(raw) != 3 || !validAgentProgressCursor(cursor) {
			return nil, fmt.Errorf("agent peer authorization receipt missing or malformed")
		}
		if _, err := agentTask(task, source.Namespace, source.Tenant, acceptedID); err != nil {
			return nil, err
		}
		taskStatus, statusOK := task["status"].(map[string]any)
		taskState, stateOK := taskStatus["state"].(string)
		if !statusOK || !stateOK || (taskState != "working" && taskState != "completed" && taskState != "input_required" && taskState != "auth_required") || (taskState == "auth_required" && task["pendingApprovalId"] == challengeID) {
			return nil, fmt.Errorf("agent peer authorization receipt missing or malformed")
		}
		status.Task, status.ProgressCursor = task, cursor
	default:
		return nil, fmt.Errorf("agent peer authorization receipt missing or malformed")
	}
	return &AgentPeerAuthorizationReceipt{SubmissionID: peer.SubmissionID, AuthorizationID: authorizationID, Status: status}, nil
}

func validAgentProgressCursor(value string) bool {
	if len(value) < 3 || len(value) > 512 || value[0] != '"' || value[len(value)-1] != '"' {
		return false
	}
	for _, character := range value[1 : len(value)-1] {
		if !(character >= 'a' && character <= 'z' || character >= 'A' && character <= 'Z' || character >= '0' && character <= '9' || strings.ContainsRune("-_.:", character)) {
			return false
		}
	}
	return true
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
	task, _, err := c.agentServiceRequest(ctx, "GET", path+"/tasks/"+id, nil, receipt.SourceContext, nil)
	if err != nil {
		return nil, err
	}
	if _, err := agentTask(task, receipt.Namespace, receipt.Tenant, receipt.TaskID); err != nil {
		return nil, err
	}
	return task, nil
}

// AgentServiceContinueTask continues the original retained task using the
// host-owned source context captured in its receipt.
func (c *Client) AgentServiceContinueTask(ctx context.Context, receipt *AgentServiceReceipt, message map[string]any) (map[string]any, error) {
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
	task, _, err := c.agentServiceRequest(ctx, "POST", path+"/tasks/"+id+"/message:send", map[string]any{"message": message}, receipt.SourceContext, nil)
	if err != nil {
		return nil, err
	}
	if _, err := agentTask(task, receipt.Namespace, receipt.Tenant, receipt.TaskID); err != nil {
		return nil, err
	}
	return task, nil
}

// AgentServiceRequestAuthorization opens the recipient's fixed authorization profile.
func (c *Client) AgentServiceRequestAuthorization(ctx context.Context, namespace, tenant, agent, taskID, authorizationRequestID string) (map[string]any, error) {
	base, err := agentServiceBase(namespace, tenant, agent)
	if err != nil {
		return nil, err
	}
	id, err := agentSegment(taskID)
	if err != nil {
		return nil, err
	}
	body := map[string]any{"authorizationRequestId": authorizationRequestID}
	task, _, err := c.agentServiceRequest(ctx, "POST", base+"/tasks/"+id+"/authorization:request", body, "", nil)
	if err != nil {
		return nil, err
	}
	if _, err := agentTask(task, namespace, tenant, taskID); err != nil {
		return nil, err
	}
	return task, nil
}

// AgentServiceResolveAuthorization invokes the verifier for one exact challenge.
func (c *Client) AgentServiceResolveAuthorization(ctx context.Context, receipt *AgentServiceReceipt, challengeID string) (map[string]any, error) {
	if receipt == nil {
		return nil, fmt.Errorf("agent service receipt required")
	}
	if err := agentSource(receipt.SourceContext); err != nil {
		return nil, err
	}
	base, err := agentServiceBase(receipt.Namespace, receipt.Tenant, receipt.Agent)
	if err != nil {
		return nil, err
	}
	id, err := agentSegment(receipt.TaskID)
	if err != nil {
		return nil, err
	}
	task, _, err := c.agentServiceRequest(ctx, "POST", base+"/tasks/"+id+"/authorization:resolve", map[string]any{"challengeId": challengeID}, receipt.SourceContext, nil)
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
	value, _, err := c.agentServiceRequest(ctx, "POST", path+"/tasks/"+id+"/stop", nil, receipt.SourceContext, nil)
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
