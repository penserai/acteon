package acteon

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
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

func (c *Client) GetBusSubscription(ctx context.Context, namespace, tenant, subID string) (*BusSubscription, error) {
	path := fmt.Sprintf("/v1/bus/subscriptions/%s/%s/%s", busSeg(namespace), busSeg(tenant), busSeg(subID))
	var out BusSubscription
	if _, err := c.busDoJSON(ctx, http.MethodGet, path, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) DeleteBusSubscription(ctx context.Context, namespace, tenant, subID string) error {
	path := fmt.Sprintf("/v1/bus/subscriptions/%s/%s/%s", busSeg(namespace), busSeg(tenant), busSeg(subID))
	_, err := c.busDoJSON(ctx, http.MethodDelete, path, nil, nil)
	return err
}

func (c *Client) GetBusSubscriptionLag(ctx context.Context, namespace, tenant, subID string) (*BusLag, error) {
	path := fmt.Sprintf("/v1/bus/subscriptions/%s/%s/%s/lag", busSeg(namespace), busSeg(tenant), busSeg(subID))
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

// SetBusAgentAdminState sets the operator admin state on an agent
// (active / suspended / banned). Requires the standard ManageAgent
// permission. The server returns 400 if req.ExpiresAt is set on
// anything other than "suspended".
func (c *Client) SetBusAgentAdminState(
	ctx context.Context,
	namespace, tenant, agentID string,
	req *SetBusAgentAdminState,
) (*BusAgent, error) {
	path := fmt.Sprintf("/v1/bus/agents/%s/%s/%s/admin-state",
		busSeg(namespace), busSeg(tenant), busSeg(agentID))
	var out BusAgent
	if _, err := c.busDoJSON(ctx, http.MethodPut, path, req, &out); err != nil {
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

// ConsumeBusSubscriptionOptions configures the SSE topic tail.
type ConsumeBusSubscriptionOptions struct {
	// Topic is the full Kafka topic name (`namespace.tenant.name`).
	Topic string
	// From is "earliest" or "latest" (server defaults to latest if empty).
	From string
	// Reconnect, when non-nil, wraps the underlying SSE pump with
	// best-effort exponential backoff + reconnect-from-latest. A
	// successful reconnect emits an item with Kind ==
	// BusConsumeKindReconnected so callers can resync state.
	Reconnect *ReconnectConfig
}

// ConsumeBusSubscription opens an SSE stream against
// `/v1/bus/subscribe/{subscription_id}` and returns a channel of typed
// items. The channel is closed when the context is cancelled or the
// connection drops (and reconnect is not configured).
//
// Server-side `bus.error` events surface as items with Kind == Error;
// SSE keep-alive comments surface as Kind == KeepAlive so callers can
// use them as a liveness signal.
//
// When `opts.Reconnect` is non-nil, a clean disconnect triggers
// exponential backoff and a fresh subscribe call from `latest` —
// emitting Kind == BusConsumeKindReconnected so callers can resync
// state. Note that resume from `latest` means messages produced
// during the disconnect window are dropped; use Phase 2 durable
// subscriptions with manual ack for lossless delivery.
func (c *Client) ConsumeBusSubscription(
	ctx context.Context,
	subscriptionID string,
	opts *ConsumeBusSubscriptionOptions,
) (<-chan *BusConsumeItem, error) {
	if opts == nil || opts.Topic == "" {
		return nil, &ConnectionError{Message: "ConsumeBusSubscription: opts.Topic is required"}
	}
	if opts.Reconnect == nil {
		path := fmt.Sprintf("/v1/bus/subscribe/%s?%s",
			busSeg(subscriptionID), buildSubscribeQuery(opts.Topic, opts.From).Encode())
		return c.consumeBusSubscriptionOnce(ctx, path)
	}
	return c.consumeBusSubscriptionReconnecting(ctx, subscriptionID, opts), nil
}

// consumeBusSubscriptionOnce opens a single SSE stream — used for
// the no-reconnect path and as the inner pump of the reconnect loop.
func (c *Client) consumeBusSubscriptionOnce(
	ctx context.Context,
	path string,
) (<-chan *BusConsumeItem, error) {
	envCh, err := c.openBusSSE(ctx, path)
	if err != nil {
		return nil, err
	}
	out := make(chan *BusConsumeItem, 64)
	go func() {
		defer close(out)
		for env := range envCh {
			item, perr := parseBusConsumeEnvelope(env)
			if perr != nil {
				select {
				case out <- &BusConsumeItem{Kind: BusConsumeKindError, Error: perr.Error()}:
				case <-ctx.Done():
					return
				}
				continue
			}
			select {
			case out <- item:
			case <-ctx.Done():
				return
			}
		}
	}()
	return out, nil
}

// consumeBusSubscriptionReconnecting wraps the once-pump with
// exponential backoff + reconnect-from-latest. The first attempt uses
// `opts.From`; subsequent attempts always use `latest`. Counter
// resets on a successful read.
func (c *Client) consumeBusSubscriptionReconnecting(
	ctx context.Context,
	subscriptionID string,
	opts *ConsumeBusSubscriptionOptions,
) <-chan *BusConsumeItem {
	out := make(chan *BusConsumeItem, 64)
	cfg := *opts.Reconnect
	if cfg.InitialBackoffMs == 0 {
		cfg.InitialBackoffMs = 500
	}
	if cfg.MaxBackoffMs == 0 {
		cfg.MaxBackoffMs = 30_000
	}
	go func() {
		defer close(out)
		var attempt uint32
		firstOpen := true
		for {
			from := opts.From
			if !firstOpen {
				from = "latest"
			}
			path := fmt.Sprintf("/v1/bus/subscribe/%s?%s",
				busSeg(subscriptionID), buildSubscribeQuery(opts.Topic, from).Encode())
			inner, err := c.consumeBusSubscriptionOnce(ctx, path)
			if err != nil {
				select {
				case out <- &BusConsumeItem{Kind: BusConsumeKindError, Error: err.Error()}:
				case <-ctx.Done():
					return
				}
			} else {
				for item := range inner {
					attempt = 0
					select {
					case out <- item:
					case <-ctx.Done():
						return
					}
				}
			}
			firstOpen = false
			if cfg.MaxAttempts != 0 && attempt >= cfg.MaxAttempts {
				return
			}
			backoff := reconnectBackoffMs(attempt, &cfg)
			timer := time.NewTimer(time.Duration(backoff) * time.Millisecond)
			select {
			case <-timer.C:
			case <-ctx.Done():
				timer.Stop()
				return
			}
			attempt++
			select {
			case out <- &BusConsumeItem{
				Kind:      BusConsumeKindReconnected,
				BackoffMs: backoff,
				Attempt:   attempt,
			}:
			case <-ctx.Done():
				return
			}
		}
	}()
	return out
}

func buildSubscribeQuery(topic, from string) url.Values {
	q := url.Values{}
	q.Set("topic", topic)
	if from != "" {
		q.Set("from", from)
	}
	return q
}

// reconnectBackoffMs is exponential, capped at cfg.MaxBackoffMs. The
// shift is bounded at 20 so wild attempt counters can't overflow.
func reconnectBackoffMs(attempt uint32, cfg *ReconnectConfig) int64 {
	shift := attempt
	if shift > 20 {
		shift = 20
	}
	exp := cfg.InitialBackoffMs * (1 << shift)
	if exp > cfg.MaxBackoffMs {
		return cfg.MaxBackoffMs
	}
	return exp
}

// ConsumeBusStream opens an SSE stream against
// `/v1/bus/streams/{ns}/{tenant}/{conversation_id}/{stream_id}` and
// returns a channel of typed items. The server filters records by
// `(envelope_kind, conversation_id, stream_id)` so this consumer only
// sees chunks for the requested stream id and the channel is closed
// after the terminal `end` envelope is observed.
func (c *Client) ConsumeBusStream(
	ctx context.Context,
	namespace, tenant, conversationID, streamID string,
) (<-chan *BusStreamItem, error) {
	path := fmt.Sprintf("/v1/bus/streams/%s/%s/%s/%s",
		busSeg(namespace), busSeg(tenant),
		busSeg(conversationID), busSeg(streamID))
	envCh, err := c.openBusSSE(ctx, path)
	if err != nil {
		return nil, err
	}
	out := make(chan *BusStreamItem, 64)
	go func() {
		defer close(out)
		for env := range envCh {
			item, perr := parseBusStreamEnvelope(env)
			if perr != nil {
				select {
				case out <- &BusStreamItem{Kind: BusStreamKindError, Error: perr.Error()}:
				case <-ctx.Done():
					return
				}
				continue
			}
			select {
			case out <- item:
			case <-ctx.Done():
				return
			}
			if item.Kind == BusStreamKindEnd {
				return
			}
		}
	}()
	return out, nil
}

// busSseEnvelope is the raw line-protocol output the bus consumers
// post-process into typed items. Either a frame (event/id/data), a
// keep-alive comment, or a synthesized scanner-side error (bumped
// past `bufio.ErrTooLong` and similar transport faults so consumers
// see a typed Error item instead of a silent channel close).
type busSseEnvelope struct {
	keepAlive bool
	event     string
	id        string
	data      string
	// Set when the underlying byte scanner failed mid-stream. The
	// outer consumer parser maps this to its own Error variant
	// (`BusConsumeKindError` or `BusStreamKindError`) so the caller
	// gets a typed signal instead of an opaque close.
	transportErr string
}

func parseBusConsumeEnvelope(env *busSseEnvelope) (*BusConsumeItem, error) {
	if env.keepAlive {
		return &BusConsumeItem{Kind: BusConsumeKindKeepAlive}, nil
	}
	if env.transportErr != "" {
		return &BusConsumeItem{Kind: BusConsumeKindError, Error: env.transportErr}, nil
	}
	name := env.event
	if name == "" {
		name = "message"
	}
	switch name {
	case "bus.message", "message":
		var msg BusConsumedMessage
		if err := json.Unmarshal([]byte(env.data), &msg); err != nil {
			return nil, fmt.Errorf("invalid bus.message payload: %w", err)
		}
		return &BusConsumeItem{Kind: BusConsumeKindMessage, Message: &msg}, nil
	case "bus.error":
		return &BusConsumeItem{Kind: BusConsumeKindError, Error: extractBusErrorMessage(env.data)}, nil
	default:
		return nil, fmt.Errorf("unexpected SSE event %q on bus subscribe stream", name)
	}
}

func parseBusStreamEnvelope(env *busSseEnvelope) (*BusStreamItem, error) {
	if env.keepAlive {
		return &BusStreamItem{Kind: BusStreamKindKeepAlive}, nil
	}
	if env.transportErr != "" {
		return &BusStreamItem{Kind: BusStreamKindError, Error: env.transportErr}, nil
	}
	name := env.event
	if name == "" {
		name = "message"
	}
	switch name {
	case "bus.stream.chunk":
		var chunk StreamChunkEnvelope
		if err := json.Unmarshal([]byte(env.data), &chunk); err != nil {
			return nil, fmt.Errorf("invalid stream chunk payload: %w", err)
		}
		return &BusStreamItem{Kind: BusStreamKindChunk, Chunk: &chunk}, nil
	case "bus.stream.end":
		var end StreamEndEnvelope
		if err := json.Unmarshal([]byte(env.data), &end); err != nil {
			return nil, fmt.Errorf("invalid stream end payload: %w", err)
		}
		return &BusStreamItem{Kind: BusStreamKindEnd, End: &end}, nil
	case "bus.stream.error":
		return &BusStreamItem{Kind: BusStreamKindError, Error: extractBusErrorMessage(env.data)}, nil
	default:
		return nil, fmt.Errorf("unexpected SSE event %q on bus stream consumer", name)
	}
}

// extractBusErrorMessage pulls the `error` field from a JSON `{"error":
// "..."}` body. Falls back to the raw data if the body isn't structured
// — defensive against the server emitting a plain string for some
// future error path.
func extractBusErrorMessage(data string) string {
	var body struct {
		Error string `json:"error"`
	}
	if err := json.Unmarshal([]byte(data), &body); err == nil && body.Error != "" {
		return body.Error
	}
	return data
}

// openBusSSE is a sibling of `openSSE` that surfaces SSE keep-alive
// comments instead of swallowing them. Bus consumers want them as a
// liveness signal so the surface is wider than the dispatch event
// stream's.
func (c *Client) openBusSSE(ctx context.Context, path string) (<-chan *busSseEnvelope, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.baseURL+path, nil)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	req.Header.Set("Accept", "text/event-stream")
	req.Header.Set("Cache-Control", "no-cache")
	if c.apiKey != "" {
		req.Header.Set("Authorization", "Bearer "+c.apiKey)
	}
	// SSE is long-lived — bypass the client timeout the same way
	// `openSSE` does.
	sseClient := &http.Client{}
	resp, err := sseClient.Do(req)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		return nil, parseBusError(resp.StatusCode, body)
	}
	ch := make(chan *busSseEnvelope, 64)
	go func() {
		defer close(ch)
		defer resp.Body.Close()
		scanner := bufio.NewScanner(resp.Body)
		// Bus payloads can carry large LLM outputs or batched state.
		// 8 MiB is the upper bound we'll tolerate per `data:` line —
		// anything longer is almost certainly a bug or a hostile
		// upstream, and we surface the failure as a typed Error item
		// (rather than letting the channel close silently the way
		// `bufio.ErrTooLong` would).
		const maxLine = 8 << 20
		scanner.Buffer(make([]byte, 0, 64*1024), maxLine)
		var event, id string
		var dataLines []string
		flush := func() {
			if len(dataLines) == 0 && event == "" && id == "" {
				return
			}
			frame := &busSseEnvelope{
				event: event,
				id:    id,
				data:  strings.Join(dataLines, "\n"),
			}
			select {
			case ch <- frame:
			case <-ctx.Done():
			}
			event = ""
			id = ""
			dataLines = nil
		}
		for scanner.Scan() {
			line := scanner.Text()
			if strings.HasPrefix(line, ":") {
				select {
				case ch <- &busSseEnvelope{keepAlive: true}:
				case <-ctx.Done():
					return
				}
				continue
			}
			if line == "" {
				flush()
				continue
			}
			switch {
			case strings.HasPrefix(line, "id:"):
				id = strings.TrimSpace(strings.TrimPrefix(line, "id:"))
			case strings.HasPrefix(line, "event:"):
				event = strings.TrimSpace(strings.TrimPrefix(line, "event:"))
			case strings.HasPrefix(line, "data:"):
				dataLines = append(dataLines, strings.TrimSpace(strings.TrimPrefix(line, "data:")))
			}
		}
		// Surface scanner errors (most commonly `bufio.ErrTooLong`)
		// as a synthesized envelope so both the subscription and
		// stream parsers can lift it to their own Error item — the
		// alternative is an opaque channel close that callers can't
		// distinguish from the server cleanly ending the stream.
		if err := scanner.Err(); err != nil {
			select {
			case ch <- &busSseEnvelope{transportErr: err.Error()}:
			case <-ctx.Done():
			}
		}
	}()
	return ch, nil
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
