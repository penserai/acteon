package acteon

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
)

// Phase 8c: agentic bus surface (Phases 1-6c).
//
// Method names match the existing Go SDK convention (exported
// PascalCase); wire payloads match the Rust + Python + Node SDKs
// byte-for-byte.

// busSeg percent-encodes a single path segment so reserved
// characters like `/` don't slip into the URL grammar. Acteon's
// bus REST surface treats namespace / tenant / name slots as
// opaque strings.
func busSeg(s string) string {
	return url.PathEscape(s)
}

// busDoJSON is a thin helper around `doRequest` that reads the
// response body and surfaces structured Acteon errors as `*APIError`
// (or falls back to `*HTTPError` when the body isn't structured).
// Successful responses get unmarshalled into `out` (when non-nil).
func (c *Client) busDoJSON(
	ctx context.Context,
	method, path string,
	body, out any,
) (*http.Response, error) {
	resp, err := c.doRequest(ctx, method, path, body)
	if err != nil {
		return nil, err
	}
	respBody, readErr := io.ReadAll(resp.Body)
	resp.Body.Close()
	if readErr != nil {
		return resp, &ConnectionError{Message: readErr.Error()}
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		var errResp struct {
			Code    string `json:"code"`
			Message string `json:"message"`
			Error   string `json:"error"`
		}
		if jsonErr := json.Unmarshal(respBody, &errResp); jsonErr == nil {
			msg := errResp.Error
			if msg == "" {
				msg = errResp.Message
			}
			if msg == "" {
				msg = "bus error"
			}
			code := errResp.Code
			if code == "" {
				code = "BUS"
			}
			return resp, &APIError{Code: code, Message: msg, Retryable: false}
		}
		return resp, &HTTPError{Status: resp.StatusCode, Message: string(respBody)}
	}
	if out != nil && len(respBody) > 0 {
		if err := json.Unmarshal(respBody, out); err != nil {
			return resp, &ConnectionError{Message: err.Error()}
		}
	}
	return resp, nil
}

// =============================================================================
// Phase 1: Topics + publish
// =============================================================================

func (c *Client) CreateBusTopic(ctx context.Context, req *CreateBusTopic) (*BusTopic, error) {
	var out BusTopic
	if _, err := c.busDoJSON(ctx, http.MethodPost, "/v1/bus/topics", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) ListBusTopics(ctx context.Context, filter *ListBusTopicsFilter) ([]BusTopic, error) {
	path := "/v1/bus/topics"
	if filter != nil {
		params := url.Values{}
		if filter.Namespace != "" {
			params.Set("namespace", filter.Namespace)
		}
		if filter.Tenant != "" {
			params.Set("tenant", filter.Tenant)
		}
		if len(params) > 0 {
			path += "?" + params.Encode()
		}
	}
	var out ListBusTopicsResponse
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return out.Topics, nil
}

func (c *Client) GetBusTopic(ctx context.Context, namespace, tenant, name string) (*BusTopic, error) {
	path := fmt.Sprintf("/v1/bus/topics/%s/%s/%s", busSeg(namespace), busSeg(tenant), busSeg(name))
	var out BusTopic
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) DeleteBusTopic(ctx context.Context, namespace, tenant, name string) error {
	path := fmt.Sprintf("/v1/bus/topics/%s/%s/%s", busSeg(namespace), busSeg(tenant), busSeg(name))
	_, err := c.busDoJSON(ctx, http.MethodDelete, path, nil, nil)
	return err
}

func (c *Client) PublishBusMessage(ctx context.Context, req *PublishBusMessage) (*PublishReceipt, error) {
	var out PublishReceipt
	if _, err := c.busDoJSON(ctx, http.MethodPost, "/v1/bus/publish", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// =============================================================================
// Phase 2: Subscriptions + lag
// =============================================================================

func (c *Client) CreateBusSubscription(ctx context.Context, req *CreateBusSubscription) (*BusSubscription, error) {
	var out BusSubscription
	if _, err := c.busDoJSON(ctx, http.MethodPost, "/v1/bus/subscriptions", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) ListBusSubscriptions(ctx context.Context, filter *ListBusSubscriptionsFilter) ([]BusSubscription, error) {
	path := "/v1/bus/subscriptions"
	if filter != nil {
		params := url.Values{}
		if filter.Namespace != "" {
			params.Set("namespace", filter.Namespace)
		}
		if filter.Tenant != "" {
			params.Set("tenant", filter.Tenant)
		}
		if filter.Topic != "" {
			params.Set("topic", filter.Topic)
		}
		if len(params) > 0 {
			path += "?" + params.Encode()
		}
	}
	var out ListBusSubscriptionsResponse
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return out.Subscriptions, nil
}

func (c *Client) GetBusSubscription(ctx context.Context, subID string) (*BusSubscription, error) {
	path := fmt.Sprintf("/v1/bus/subscriptions/%s", busSeg(subID))
	var out BusSubscription
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) DeleteBusSubscription(ctx context.Context, subID string) error {
	path := fmt.Sprintf("/v1/bus/subscriptions/%s", busSeg(subID))
	_, err := c.busDoJSON(ctx, http.MethodDelete, path, nil, nil)
	return err
}

func (c *Client) GetBusSubscriptionLag(ctx context.Context, subID string) (*BusLag, error) {
	path := fmt.Sprintf("/v1/bus/subscriptions/%s/lag", busSeg(subID))
	var out BusLag
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// =============================================================================
// Phase 3: Schemas
// =============================================================================

func (c *Client) RegisterBusSchema(ctx context.Context, req *RegisterBusSchema) (*BusSchema, error) {
	var out BusSchema
	if _, err := c.busDoJSON(ctx, http.MethodPost, "/v1/bus/schemas", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) ListBusSchemas(ctx context.Context, filter *ListBusSchemasFilter) ([]BusSchema, error) {
	path := "/v1/bus/schemas"
	if filter != nil {
		params := url.Values{}
		if filter.Namespace != "" {
			params.Set("namespace", filter.Namespace)
		}
		if filter.Tenant != "" {
			params.Set("tenant", filter.Tenant)
		}
		if filter.Subject != "" {
			params.Set("subject", filter.Subject)
		}
		if filter.LatestOnly {
			params.Set("latest_only", "true")
		}
		if len(params) > 0 {
			path += "?" + params.Encode()
		}
	}
	var out ListBusSchemasResponse
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return out.Schemas, nil
}

func (c *Client) GetBusSchema(ctx context.Context, namespace, tenant, subject string, version int) (*BusSchema, error) {
	path := fmt.Sprintf("/v1/bus/schemas/%s/%s/%s/%d",
		busSeg(namespace), busSeg(tenant), busSeg(subject), version)
	var out BusSchema
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) DeleteBusSchema(ctx context.Context, namespace, tenant, subject string, version int) error {
	path := fmt.Sprintf("/v1/bus/schemas/%s/%s/%s/%d",
		busSeg(namespace), busSeg(tenant), busSeg(subject), version)
	_, err := c.busDoJSON(ctx, http.MethodDelete, path, nil, nil)
	return err
}

// =============================================================================
// Phase 4: Agents + heartbeat
// =============================================================================

func (c *Client) RegisterBusAgent(ctx context.Context, req *RegisterBusAgent) (*BusAgent, error) {
	var out BusAgent
	if _, err := c.busDoJSON(ctx, http.MethodPost, "/v1/bus/agents", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) ListBusAgents(ctx context.Context, filter *ListBusAgentsFilter) ([]BusAgent, error) {
	path := "/v1/bus/agents"
	if filter != nil {
		params := url.Values{}
		if filter.Namespace != "" {
			params.Set("namespace", filter.Namespace)
		}
		if filter.Tenant != "" {
			params.Set("tenant", filter.Tenant)
		}
		if len(params) > 0 {
			path += "?" + params.Encode()
		}
	}
	var out ListBusAgentsResponse
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return out.Agents, nil
}

func (c *Client) GetBusAgent(ctx context.Context, namespace, tenant, agentID string) (*BusAgent, error) {
	path := fmt.Sprintf("/v1/bus/agents/%s/%s/%s",
		busSeg(namespace), busSeg(tenant), busSeg(agentID))
	var out BusAgent
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) DeleteBusAgent(ctx context.Context, namespace, tenant, agentID string) error {
	path := fmt.Sprintf("/v1/bus/agents/%s/%s/%s",
		busSeg(namespace), busSeg(tenant), busSeg(agentID))
	_, err := c.busDoJSON(ctx, http.MethodDelete, path, nil, nil)
	return err
}

func (c *Client) HeartbeatBusAgent(ctx context.Context, namespace, tenant, agentID string) (*BusAgent, error) {
	path := fmt.Sprintf("/v1/bus/agents/%s/%s/%s/heartbeat",
		busSeg(namespace), busSeg(tenant), busSeg(agentID))
	var out BusAgent
	if _, err := c.busDoJSON(ctx, http.MethodPatch, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// =============================================================================
// Phase 5: Conversations
// =============================================================================

func (c *Client) CreateBusConversation(ctx context.Context, req *CreateBusConversation) (*BusConversation, error) {
	var out BusConversation
	if _, err := c.busDoJSON(ctx, http.MethodPost, "/v1/bus/conversations", req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) ListBusConversations(ctx context.Context, filter *ListBusConversationsFilter) ([]BusConversation, error) {
	path := "/v1/bus/conversations"
	if filter != nil {
		params := url.Values{}
		if filter.Namespace != "" {
			params.Set("namespace", filter.Namespace)
		}
		if filter.Tenant != "" {
			params.Set("tenant", filter.Tenant)
		}
		if filter.State != "" {
			params.Set("state", filter.State)
		}
		if filter.Participant != "" {
			params.Set("participant", filter.Participant)
		}
		if len(params) > 0 {
			path += "?" + params.Encode()
		}
	}
	var out ListBusConversationsResponse
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return out.Conversations, nil
}

func (c *Client) GetBusConversation(ctx context.Context, namespace, tenant, conversationID string) (*BusConversation, error) {
	path := fmt.Sprintf("/v1/bus/conversations/%s/%s/%s",
		busSeg(namespace), busSeg(tenant), busSeg(conversationID))
	var out BusConversation
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) DeleteBusConversation(ctx context.Context, namespace, tenant, conversationID string) error {
	path := fmt.Sprintf("/v1/bus/conversations/%s/%s/%s",
		busSeg(namespace), busSeg(tenant), busSeg(conversationID))
	_, err := c.busDoJSON(ctx, http.MethodDelete, path, nil, nil)
	return err
}

func (c *Client) TransitionBusConversation(
	ctx context.Context, namespace, tenant, conversationID, targetState string,
) (*BusConversation, error) {
	path := fmt.Sprintf("/v1/bus/conversations/%s/%s/%s/transition",
		busSeg(namespace), busSeg(tenant), busSeg(conversationID))
	body := TransitionBusConversationRequest{TargetState: targetState}
	var out BusConversation
	if _, err := c.busDoJSON(ctx, http.MethodPost, path, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) AppendBusConversationMessage(
	ctx context.Context,
	namespace, tenant, conversationID string,
	req *AppendBusConversationMessage,
) (map[string]any, error) {
	path := fmt.Sprintf("/v1/bus/conversations/%s/%s/%s/messages",
		busSeg(namespace), busSeg(tenant), busSeg(conversationID))
	out := make(map[string]any)
	if _, err := c.busDoJSON(ctx, http.MethodPost, path, req, &out); err != nil {
		return nil, err
	}
	return out, nil
}

func (c *Client) ReplayBusConversationMessages(
	ctx context.Context,
	namespace, tenant, conversationID string,
	params *ReplayBusConversationParams,
) (*BusReplayResponse, error) {
	path := fmt.Sprintf("/v1/bus/conversations/%s/%s/%s/messages",
		busSeg(namespace), busSeg(tenant), busSeg(conversationID))
	if params != nil {
		q := url.Values{}
		if params.Limit > 0 {
			q.Set("limit", strconv.Itoa(params.Limit))
		}
		if params.Cursor != "" {
			q.Set("cursor", params.Cursor)
		}
		if len(q) > 0 {
			path += "?" + q.Encode()
		}
	}
	var out BusReplayResponse
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// =============================================================================
// Phase 6a: Tool envelopes
// =============================================================================

// PostBusToolCall appends a tool-call envelope. Returns a discriminated
// outcome — `Produced` non-nil when the call landed on Kafka,
// `Parked` non-nil when the server parked it under a Phase 6c HITL
// approval (driven by `req.RequireApproval`).
func (c *Client) PostBusToolCall(
	ctx context.Context,
	namespace, tenant, conversationID string,
	req *PostBusToolCall,
) (*PostBusToolCallOutcome, error) {
	path := fmt.Sprintf("/v1/bus/conversations/%s/%s/%s/tool-calls",
		busSeg(namespace), busSeg(tenant), busSeg(conversationID))
	resp, err := c.doRequest(ctx, http.MethodPost, path, req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	respBody, readErr := io.ReadAll(resp.Body)
	if readErr != nil {
		return nil, &ConnectionError{Message: readErr.Error()}
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, parseBusError(resp.StatusCode, respBody)
	}
	if resp.StatusCode == http.StatusAccepted {
		var parked BusApprovalParkedReceipt
		if err := json.Unmarshal(respBody, &parked); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &PostBusToolCallOutcome{Parked: &parked}, nil
	}
	var produced BusToolEnvelopeReceipt
	if err := json.Unmarshal(respBody, &produced); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &PostBusToolCallOutcome{Produced: &produced}, nil
}

func (c *Client) PostBusToolResult(
	ctx context.Context,
	namespace, tenant, conversationID string,
	req *PostBusToolResult,
) (*BusToolEnvelopeReceipt, error) {
	path := fmt.Sprintf("/v1/bus/conversations/%s/%s/%s/tool-results",
		busSeg(namespace), busSeg(tenant), busSeg(conversationID))
	var out BusToolEnvelopeReceipt
	if _, err := c.busDoJSON(ctx, http.MethodPost, path, req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) LookupBusToolResult(
	ctx context.Context,
	namespace, tenant, callID string,
	params *BusToolResultLookupParams,
) (*BusToolResultLookup, error) {
	path := fmt.Sprintf("/v1/bus/tool-calls/%s/%s/%s/result",
		busSeg(namespace), busSeg(tenant), busSeg(callID))
	if params != nil {
		q := url.Values{}
		q.Set("conversation_id", params.ConversationID)
		if params.Cursor != "" {
			q.Set("cursor", params.Cursor)
		}
		if params.TimeoutMs > 0 {
			q.Set("timeout_ms", strconv.FormatUint(params.TimeoutMs, 10))
		}
		path += "?" + q.Encode()
	}
	var out BusToolResultLookup
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// =============================================================================
// Phase 6b: Stream envelopes
// =============================================================================

func (c *Client) PostBusStreamChunk(
	ctx context.Context,
	namespace, tenant, conversationID string,
	req *PostBusStreamChunk,
) (*BusStreamEnvelopeReceipt, error) {
	path := fmt.Sprintf("/v1/bus/conversations/%s/%s/%s/stream-chunks",
		busSeg(namespace), busSeg(tenant), busSeg(conversationID))
	var out BusStreamEnvelopeReceipt
	if _, err := c.busDoJSON(ctx, http.MethodPost, path, req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) PostBusStreamEnd(
	ctx context.Context,
	namespace, tenant, conversationID string,
	req *PostBusStreamEnd,
) (*BusStreamEnvelopeReceipt, error) {
	path := fmt.Sprintf("/v1/bus/conversations/%s/%s/%s/stream-end",
		busSeg(namespace), busSeg(tenant), busSeg(conversationID))
	var out BusStreamEnvelopeReceipt
	if _, err := c.busDoJSON(ctx, http.MethodPost, path, req, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// BusStreamConsumeURL returns the SSE consume URL for a stream. Plug
// it into your preferred SSE client (`r3labs/sse`, the stdlib HTTP
// streaming reader, etc.). Path segments are encoded the same way
// the Rust + Python + Node SDKs encode them.
func (c *Client) BusStreamConsumeURL(
	namespace, tenant, conversationID, streamID string,
) string {
	return fmt.Sprintf("%s/v1/bus/streams/%s/%s/%s/%s",
		c.baseURL,
		busSeg(namespace), busSeg(tenant),
		busSeg(conversationID), busSeg(streamID))
}

// =============================================================================
// Phase 6c: HITL approvals
// =============================================================================

func (c *Client) ListBusApprovals(
	ctx context.Context,
	namespace, tenant string,
	filter *ListBusApprovalsFilter,
) ([]BusApprovalView, error) {
	path := fmt.Sprintf("/v1/bus/approvals/%s/%s", busSeg(namespace), busSeg(tenant))
	if filter != nil {
		q := url.Values{}
		if filter.Status != "" {
			q.Set("status", filter.Status)
		}
		if filter.ConversationID != "" {
			q.Set("conversation_id", filter.ConversationID)
		}
		if len(q) > 0 {
			path += "?" + q.Encode()
		}
	}
	var out ListBusApprovalsResponse
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return out.Approvals, nil
}

func (c *Client) GetBusApproval(
	ctx context.Context, namespace, tenant, approvalID string,
) (*BusApprovalView, error) {
	path := fmt.Sprintf("/v1/bus/approvals/%s/%s/%s",
		busSeg(namespace), busSeg(tenant), busSeg(approvalID))
	var out BusApprovalView
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) ApproveBusApproval(
	ctx context.Context,
	namespace, tenant, approvalID string,
	decision *BusApprovalDecision,
) (*BusApprovalDecisionResponse, error) {
	path := fmt.Sprintf("/v1/bus/approvals/%s/%s/%s/approve",
		busSeg(namespace), busSeg(tenant), busSeg(approvalID))
	var out BusApprovalDecisionResponse
	if _, err := c.busDoJSON(ctx, http.MethodPost, path, decision, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) RejectBusApproval(
	ctx context.Context,
	namespace, tenant, approvalID string,
	decision *BusApprovalDecision,
) (*BusApprovalDecisionResponse, error) {
	path := fmt.Sprintf("/v1/bus/approvals/%s/%s/%s/reject",
		busSeg(namespace), busSeg(tenant), busSeg(approvalID))
	var out BusApprovalDecisionResponse
	if _, err := c.busDoJSON(ctx, http.MethodPost, path, decision, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func parseBusError(status int, respBody []byte) error {
	var errResp struct {
		Code    string `json:"code"`
		Message string `json:"message"`
		Error   string `json:"error"`
	}
	if jsonErr := json.Unmarshal(respBody, &errResp); jsonErr == nil {
		msg := errResp.Error
		if msg == "" {
			msg = errResp.Message
		}
		if msg == "" {
			msg = "bus error"
		}
		code := errResp.Code
		if code == "" {
			code = "BUS"
		}
		return &APIError{Code: code, Message: msg, Retryable: false}
	}
	return &HTTPError{Status: status, Message: string(respBody)}
}
