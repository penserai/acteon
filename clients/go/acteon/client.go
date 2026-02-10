package acteon

import (
	"bufio"
	"bytes"
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

// Client is an HTTP client for the Acteon action gateway.
type Client struct {
	baseURL    string
	httpClient *http.Client
	apiKey     string
}

// ClientOption is a function that configures a Client.
type ClientOption func(*Client)

// WithTimeout sets the request timeout.
func WithTimeout(timeout time.Duration) ClientOption {
	return func(c *Client) {
		c.httpClient.Timeout = timeout
	}
}

// WithAPIKey sets the API key for authentication.
func WithAPIKey(apiKey string) ClientOption {
	return func(c *Client) {
		c.apiKey = apiKey
	}
}

// WithHTTPClient sets a custom HTTP client.
func WithHTTPClient(httpClient *http.Client) ClientOption {
	return func(c *Client) {
		c.httpClient = httpClient
	}
}

// NewClient creates a new Acteon client.
func NewClient(baseURL string, opts ...ClientOption) *Client {
	c := &Client{
		baseURL: strings.TrimSuffix(baseURL, "/"),
		httpClient: &http.Client{
			Timeout: 30 * time.Second,
		},
	}

	for _, opt := range opts {
		opt(c)
	}

	return c
}

func (c *Client) doRequest(ctx context.Context, method, path string, body any) (*http.Response, error) {
	var bodyReader io.Reader
	if body != nil {
		jsonBody, err := json.Marshal(body)
		if err != nil {
			return nil, err
		}
		bodyReader = bytes.NewReader(jsonBody)
	}

	req, err := http.NewRequestWithContext(ctx, method, c.baseURL+path, bodyReader)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	req.Header.Set("Content-Type", "application/json")
	if c.apiKey != "" {
		req.Header.Set("Authorization", "Bearer "+c.apiKey)
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	return resp, nil
}

// Health checks if the server is healthy.
func (c *Client) Health(ctx context.Context) (bool, error) {
	resp, err := c.doRequest(ctx, http.MethodGet, "/health", nil)
	if err != nil {
		return false, nil
	}
	defer resp.Body.Close()
	return resp.StatusCode == http.StatusOK, nil
}

// Dispatch dispatches a single action.
func (c *Client) Dispatch(ctx context.Context, action *Action) (*ActionOutcome, error) {
	resp, err := c.doRequest(ctx, http.MethodPost, "/v1/dispatch", action)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var outcome ActionOutcome
		if err := json.Unmarshal(body, &outcome); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &outcome, nil
	}

	var errResp ErrorResponse
	if err := json.Unmarshal(body, &errResp); err != nil {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to parse error response"}
	}
	return nil, &APIError{Code: errResp.Code, Message: errResp.Message, Retryable: errResp.Retryable}
}

// DispatchDryRun dispatches a single action in dry-run mode.
// Rules are evaluated but the action is not executed and no state is mutated.
func (c *Client) DispatchDryRun(ctx context.Context, action *Action) (*ActionOutcome, error) {
	resp, err := c.doRequest(ctx, http.MethodPost, "/v1/dispatch?dry_run=true", action)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var outcome ActionOutcome
		if err := json.Unmarshal(body, &outcome); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &outcome, nil
	}

	var errResp ErrorResponse
	if err := json.Unmarshal(body, &errResp); err != nil {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to parse error response"}
	}
	return nil, &APIError{Code: errResp.Code, Message: errResp.Message, Retryable: errResp.Retryable}
}

// DispatchBatch dispatches multiple actions in a single request.
func (c *Client) DispatchBatch(ctx context.Context, actions []*Action) ([]BatchResult, error) {
	resp, err := c.doRequest(ctx, http.MethodPost, "/v1/dispatch/batch", actions)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var results []BatchResult
		if err := json.Unmarshal(body, &results); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return results, nil
	}

	var errResp ErrorResponse
	if err := json.Unmarshal(body, &errResp); err != nil {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to parse error response"}
	}
	return nil, &APIError{Code: errResp.Code, Message: errResp.Message, Retryable: errResp.Retryable}
}

// DispatchBatchDryRun dispatches multiple actions in dry-run mode.
// Rules are evaluated for each action but none are executed and no state is mutated.
func (c *Client) DispatchBatchDryRun(ctx context.Context, actions []*Action) ([]BatchResult, error) {
	resp, err := c.doRequest(ctx, http.MethodPost, "/v1/dispatch/batch?dry_run=true", actions)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var results []BatchResult
		if err := json.Unmarshal(body, &results); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return results, nil
	}

	var errResp ErrorResponse
	if err := json.Unmarshal(body, &errResp); err != nil {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to parse error response"}
	}
	return nil, &APIError{Code: errResp.Code, Message: errResp.Message, Retryable: errResp.Retryable}
}

// ListRules lists all loaded rules.
func (c *Client) ListRules(ctx context.Context) ([]RuleInfo, error) {
	resp, err := c.doRequest(ctx, http.MethodGet, "/v1/rules", nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to list rules"}
	}

	var rules []RuleInfo
	if err := json.NewDecoder(resp.Body).Decode(&rules); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return rules, nil
}

// ReloadRules reloads rules from the configured directory.
func (c *Client) ReloadRules(ctx context.Context) (*ReloadResult, error) {
	resp, err := c.doRequest(ctx, http.MethodPost, "/v1/rules/reload", nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to reload rules"}
	}

	var result ReloadResult
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &result, nil
}

// SetRuleEnabled enables or disables a specific rule.
func (c *Client) SetRuleEnabled(ctx context.Context, ruleName string, enabled bool) error {
	body := map[string]bool{"enabled": enabled}
	resp, err := c.doRequest(ctx, http.MethodPut, fmt.Sprintf("/v1/rules/%s/enabled", ruleName), body)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return &HTTPError{Status: resp.StatusCode, Message: "Failed to set rule enabled"}
	}
	return nil
}

// QueryAudit queries audit records.
func (c *Client) QueryAudit(ctx context.Context, query *AuditQuery) (*AuditPage, error) {
	path := "/v1/audit"
	if query != nil {
		params := url.Values{}
		if query.Namespace != "" {
			params.Set("namespace", query.Namespace)
		}
		if query.Tenant != "" {
			params.Set("tenant", query.Tenant)
		}
		if query.Provider != "" {
			params.Set("provider", query.Provider)
		}
		if query.ActionType != "" {
			params.Set("action_type", query.ActionType)
		}
		if query.Outcome != "" {
			params.Set("outcome", query.Outcome)
		}
		if query.Limit > 0 {
			params.Set("limit", strconv.Itoa(query.Limit))
		}
		if query.Offset > 0 {
			params.Set("offset", strconv.Itoa(query.Offset))
		}
		if len(params) > 0 {
			path += "?" + params.Encode()
		}
	}

	resp, err := c.doRequest(ctx, http.MethodGet, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to query audit"}
	}

	var page AuditPage
	if err := json.NewDecoder(resp.Body).Decode(&page); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &page, nil
}

// GetAuditRecord gets a specific audit record by action ID.
func (c *Client) GetAuditRecord(ctx context.Context, actionID string) (*AuditRecord, error) {
	resp, err := c.doRequest(ctx, http.MethodGet, fmt.Sprintf("/v1/audit/%s", actionID), nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return nil, nil
	}

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to get audit record"}
	}

	var record AuditRecord
	if err := json.NewDecoder(resp.Body).Decode(&record); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &record, nil
}

// =============================================================================
// Audit Replay
// =============================================================================

// ReplayAction replays a single action from the audit trail by its action ID.
// The action is reconstructed from the stored payload and dispatched with a new ID.
func (c *Client) ReplayAction(ctx context.Context, actionID string) (*ReplayResult, error) {
	resp, err := c.doRequest(ctx, http.MethodPost, fmt.Sprintf("/v1/audit/%s/replay", actionID), nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var result ReplayResult
		if err := json.Unmarshal(body, &result); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &result, nil
	}

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: fmt.Sprintf("Audit record not found: %s", actionID)}
	}
	if resp.StatusCode == http.StatusUnprocessableEntity {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "No stored payload available for replay"}
	}

	return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to replay action"}
}

// ReplayAudit replays actions from the audit trail matching the given query.
func (c *Client) ReplayAudit(ctx context.Context, query *ReplayQuery) (*ReplaySummary, error) {
	path := "/v1/audit/replay"
	if query != nil {
		params := url.Values{}
		if query.Namespace != "" {
			params.Set("namespace", query.Namespace)
		}
		if query.Tenant != "" {
			params.Set("tenant", query.Tenant)
		}
		if query.Provider != "" {
			params.Set("provider", query.Provider)
		}
		if query.ActionType != "" {
			params.Set("action_type", query.ActionType)
		}
		if query.Outcome != "" {
			params.Set("outcome", query.Outcome)
		}
		if query.Verdict != "" {
			params.Set("verdict", query.Verdict)
		}
		if query.MatchedRule != "" {
			params.Set("matched_rule", query.MatchedRule)
		}
		if query.From != "" {
			params.Set("from", query.From)
		}
		if query.To != "" {
			params.Set("to", query.To)
		}
		if query.Limit > 0 {
			params.Set("limit", strconv.Itoa(query.Limit))
		}
		if len(params) > 0 {
			path += "?" + params.Encode()
		}
	}

	resp, err := c.doRequest(ctx, http.MethodPost, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to replay audit"}
	}

	var summary ReplaySummary
	if err := json.NewDecoder(resp.Body).Decode(&summary); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &summary, nil
}

// =============================================================================
// Events (State Machine Lifecycle)
// =============================================================================

// ListEvents lists events filtered by namespace, tenant, and optionally status.
func (c *Client) ListEvents(ctx context.Context, query *EventQuery) (*EventListResponse, error) {
	path := "/v1/events"
	if query != nil {
		params := url.Values{}
		params.Set("namespace", query.Namespace)
		params.Set("tenant", query.Tenant)
		if query.Status != "" {
			params.Set("status", query.Status)
		}
		if query.Limit > 0 {
			params.Set("limit", strconv.Itoa(query.Limit))
		}
		path += "?" + params.Encode()
	}

	resp, err := c.doRequest(ctx, http.MethodGet, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to list events"}
	}

	var result EventListResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &result, nil
}

// GetEvent gets the current state of an event by fingerprint.
func (c *Client) GetEvent(ctx context.Context, fingerprint, namespace, tenant string) (*EventState, error) {
	params := url.Values{}
	params.Set("namespace", namespace)
	params.Set("tenant", tenant)
	path := fmt.Sprintf("/v1/events/%s?%s", fingerprint, params.Encode())

	resp, err := c.doRequest(ctx, http.MethodGet, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return nil, nil
	}

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to get event"}
	}

	var event EventState
	if err := json.NewDecoder(resp.Body).Decode(&event); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &event, nil
}

// TransitionEvent transitions an event to a new state.
func (c *Client) TransitionEvent(ctx context.Context, fingerprint, toState, namespace, tenant string) (*TransitionResponse, error) {
	body := map[string]string{
		"to":        toState,
		"namespace": namespace,
		"tenant":    tenant,
	}
	resp, err := c.doRequest(ctx, http.MethodPut, fmt.Sprintf("/v1/events/%s/transition", fingerprint), body)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var result TransitionResponse
		if err := json.Unmarshal(respBody, &result); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &result, nil
	}

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: fmt.Sprintf("Event not found: %s", fingerprint)}
	}

	var errResp ErrorResponse
	if err := json.Unmarshal(respBody, &errResp); err != nil {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to transition event"}
	}
	return nil, &APIError{Code: errResp.Code, Message: errResp.Message, Retryable: errResp.Retryable}
}

// =============================================================================
// Groups (Event Batching)
// =============================================================================

// ListGroups lists all active event groups.
func (c *Client) ListGroups(ctx context.Context) (*GroupListResponse, error) {
	resp, err := c.doRequest(ctx, http.MethodGet, "/v1/groups", nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to list groups"}
	}

	var result GroupListResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &result, nil
}

// GetGroup gets details of a specific group.
func (c *Client) GetGroup(ctx context.Context, groupKey string) (*GroupDetail, error) {
	resp, err := c.doRequest(ctx, http.MethodGet, fmt.Sprintf("/v1/groups/%s", groupKey), nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return nil, nil
	}

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to get group"}
	}

	var detail GroupDetail
	if err := json.NewDecoder(resp.Body).Decode(&detail); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &detail, nil
}

// =============================================================================
// Approvals (Human-in-the-Loop)
// =============================================================================

// Approve approves a pending action by namespace, tenant, ID, and HMAC signature.
// Does not require authentication -- the HMAC signature serves as proof of authorization.
// Pass an empty string for kid to omit the key ID parameter.
func (c *Client) Approve(ctx context.Context, namespace, tenant, id, sig string, expiresAt int64, kid string) (*ApprovalActionResponse, error) {
	params := url.Values{}
	params.Set("sig", sig)
	params.Set("expires_at", strconv.FormatInt(expiresAt, 10))
	if kid != "" {
		params.Set("kid", kid)
	}
	path := fmt.Sprintf("/v1/approvals/%s/%s/%s/approve?%s", namespace, tenant, id, params.Encode())

	resp, err := c.doRequest(ctx, http.MethodPost, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var result ApprovalActionResponse
		if err := json.Unmarshal(body, &result); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &result, nil
	}

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Approval not found or expired"}
	}
	if resp.StatusCode == http.StatusGone {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Approval already decided"}
	}

	return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to approve"}
}

// Reject rejects a pending action by namespace, tenant, ID, and HMAC signature.
// Does not require authentication -- the HMAC signature serves as proof of authorization.
// Pass an empty string for kid to omit the key ID parameter.
func (c *Client) Reject(ctx context.Context, namespace, tenant, id, sig string, expiresAt int64, kid string) (*ApprovalActionResponse, error) {
	params := url.Values{}
	params.Set("sig", sig)
	params.Set("expires_at", strconv.FormatInt(expiresAt, 10))
	if kid != "" {
		params.Set("kid", kid)
	}
	path := fmt.Sprintf("/v1/approvals/%s/%s/%s/reject?%s", namespace, tenant, id, params.Encode())

	resp, err := c.doRequest(ctx, http.MethodPost, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var result ApprovalActionResponse
		if err := json.Unmarshal(body, &result); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &result, nil
	}

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Approval not found or expired"}
	}
	if resp.StatusCode == http.StatusGone {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Approval already decided"}
	}

	return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to reject"}
}

// GetApproval gets the status of an approval by namespace, tenant, ID, and HMAC signature.
// Returns nil if not found or expired.
// Pass an empty string for kid to omit the key ID parameter.
func (c *Client) GetApproval(ctx context.Context, namespace, tenant, id, sig string, expiresAt int64, kid string) (*ApprovalStatus, error) {
	params := url.Values{}
	params.Set("sig", sig)
	params.Set("expires_at", strconv.FormatInt(expiresAt, 10))
	if kid != "" {
		params.Set("kid", kid)
	}
	path := fmt.Sprintf("/v1/approvals/%s/%s/%s?%s", namespace, tenant, id, params.Encode())

	resp, err := c.doRequest(ctx, http.MethodGet, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return nil, nil
	}

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to get approval"}
	}

	var status ApprovalStatus
	if err := json.NewDecoder(resp.Body).Decode(&status); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &status, nil
}

// ListApprovals lists pending approvals filtered by namespace and tenant.
// Requires authentication.
func (c *Client) ListApprovals(ctx context.Context, namespace, tenant string) (*ApprovalListResponse, error) {
	params := url.Values{}
	params.Set("namespace", namespace)
	params.Set("tenant", tenant)
	path := "/v1/approvals?" + params.Encode()

	resp, err := c.doRequest(ctx, http.MethodGet, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to list approvals"}
	}

	var result ApprovalListResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &result, nil
}

// FlushGroup forces a group to flush, triggering immediate notification.
func (c *Client) FlushGroup(ctx context.Context, groupKey string) (*FlushGroupResponse, error) {
	resp, err := c.doRequest(ctx, http.MethodDelete, fmt.Sprintf("/v1/groups/%s", groupKey), nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var result FlushGroupResponse
		if err := json.Unmarshal(respBody, &result); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &result, nil
	}

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: fmt.Sprintf("Group not found: %s", groupKey)}
	}

	var errResp ErrorResponse
	if err := json.Unmarshal(respBody, &errResp); err != nil {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to flush group"}
	}
	return nil, &APIError{Code: errResp.Code, Message: errResp.Message, Retryable: errResp.Retryable}
}

// =============================================================================
// Recurring Actions
// =============================================================================

// CreateRecurring creates a recurring action.
func (c *Client) CreateRecurring(ctx context.Context, recurring *CreateRecurringAction) (*CreateRecurringResponse, error) {
	resp, err := c.doRequest(ctx, http.MethodPost, "/v1/recurring", recurring)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusCreated {
		var result CreateRecurringResponse
		if err := json.Unmarshal(body, &result); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &result, nil
	}

	var errResp ErrorResponse
	if err := json.Unmarshal(body, &errResp); err != nil {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to create recurring action"}
	}
	return nil, &APIError{Code: errResp.Code, Message: errResp.Message, Retryable: errResp.Retryable}
}

// ListRecurring lists recurring actions with optional filters.
func (c *Client) ListRecurring(ctx context.Context, filter *RecurringFilter) (*ListRecurringResponse, error) {
	path := "/v1/recurring"
	if filter != nil {
		params := url.Values{}
		if filter.Namespace != "" {
			params.Set("namespace", filter.Namespace)
		}
		if filter.Tenant != "" {
			params.Set("tenant", filter.Tenant)
		}
		if filter.Status != "" {
			params.Set("status", filter.Status)
		}
		if filter.Limit > 0 {
			params.Set("limit", strconv.Itoa(filter.Limit))
		}
		if filter.Offset > 0 {
			params.Set("offset", strconv.Itoa(filter.Offset))
		}
		if len(params) > 0 {
			path += "?" + params.Encode()
		}
	}

	resp, err := c.doRequest(ctx, http.MethodGet, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to list recurring actions"}
	}

	var result ListRecurringResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &result, nil
}

// GetRecurring gets details of a specific recurring action.
func (c *Client) GetRecurring(ctx context.Context, recurringID, namespace, tenant string) (*RecurringDetail, error) {
	params := url.Values{}
	params.Set("namespace", namespace)
	params.Set("tenant", tenant)
	path := fmt.Sprintf("/v1/recurring/%s?%s", recurringID, params.Encode())

	resp, err := c.doRequest(ctx, http.MethodGet, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return nil, nil
	}

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to get recurring action"}
	}

	var detail RecurringDetail
	if err := json.NewDecoder(resp.Body).Decode(&detail); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &detail, nil
}

// UpdateRecurring updates a recurring action.
func (c *Client) UpdateRecurring(ctx context.Context, recurringID string, update *UpdateRecurringAction) (*RecurringDetail, error) {
	resp, err := c.doRequest(ctx, http.MethodPut, fmt.Sprintf("/v1/recurring/%s", recurringID), update)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var detail RecurringDetail
		if err := json.Unmarshal(body, &detail); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &detail, nil
	}

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: fmt.Sprintf("Recurring action not found: %s", recurringID)}
	}

	var errResp ErrorResponse
	if err := json.Unmarshal(body, &errResp); err != nil {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to update recurring action"}
	}
	return nil, &APIError{Code: errResp.Code, Message: errResp.Message, Retryable: errResp.Retryable}
}

// DeleteRecurring deletes a recurring action.
func (c *Client) DeleteRecurring(ctx context.Context, recurringID, namespace, tenant string) error {
	params := url.Values{}
	params.Set("namespace", namespace)
	params.Set("tenant", tenant)
	path := fmt.Sprintf("/v1/recurring/%s?%s", recurringID, params.Encode())

	resp, err := c.doRequest(ctx, http.MethodDelete, path, nil)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNoContent {
		return nil
	}
	if resp.StatusCode == http.StatusNotFound {
		return &HTTPError{Status: resp.StatusCode, Message: fmt.Sprintf("Recurring action not found: %s", recurringID)}
	}
	return &HTTPError{Status: resp.StatusCode, Message: "Failed to delete recurring action"}
}

// PauseRecurring pauses a recurring action.
func (c *Client) PauseRecurring(ctx context.Context, recurringID, namespace, tenant string) (*RecurringDetail, error) {
	body := &RecurringLifecycleRequest{Namespace: namespace, Tenant: tenant}
	resp, err := c.doRequest(ctx, http.MethodPost, fmt.Sprintf("/v1/recurring/%s/pause", recurringID), body)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var detail RecurringDetail
		if err := json.Unmarshal(respBody, &detail); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &detail, nil
	}

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: fmt.Sprintf("Recurring action not found: %s", recurringID)}
	}
	if resp.StatusCode == http.StatusConflict {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Recurring action is already paused"}
	}
	return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to pause recurring action"}
}

// ResumeRecurring resumes a paused recurring action.
func (c *Client) ResumeRecurring(ctx context.Context, recurringID, namespace, tenant string) (*RecurringDetail, error) {
	body := &RecurringLifecycleRequest{Namespace: namespace, Tenant: tenant}
	resp, err := c.doRequest(ctx, http.MethodPost, fmt.Sprintf("/v1/recurring/%s/resume", recurringID), body)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var detail RecurringDetail
		if err := json.Unmarshal(respBody, &detail); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &detail, nil
	}

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: fmt.Sprintf("Recurring action not found: %s", recurringID)}
	}
	if resp.StatusCode == http.StatusConflict {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Recurring action is already active"}
	}
	return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to resume recurring action"}
}

// =============================================================================
// Chains
// =============================================================================

// ListChains lists chain executions filtered by namespace, tenant, and optional status.
func (c *Client) ListChains(ctx context.Context, namespace, tenant string, status *string) (*ListChainsResponse, error) {
	params := url.Values{}
	params.Set("namespace", namespace)
	params.Set("tenant", tenant)
	if status != nil {
		params.Set("status", *status)
	}
	path := "/v1/chains?" + params.Encode()

	resp, err := c.doRequest(ctx, http.MethodGet, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to list chains"}
	}

	var result ListChainsResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &result, nil
}

// GetChain gets the full details of a chain execution by ID.
func (c *Client) GetChain(ctx context.Context, chainID, namespace, tenant string) (*ChainDetailResponse, error) {
	params := url.Values{}
	params.Set("namespace", namespace)
	params.Set("tenant", tenant)
	path := fmt.Sprintf("/v1/chains/%s?%s", chainID, params.Encode())

	resp, err := c.doRequest(ctx, http.MethodGet, path, nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: fmt.Sprintf("Chain not found: %s", chainID)}
	}

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to get chain"}
	}

	var detail ChainDetailResponse
	if err := json.NewDecoder(resp.Body).Decode(&detail); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &detail, nil
}

// CancelChain cancels a running chain execution.
func (c *Client) CancelChain(ctx context.Context, chainID string, req *CancelChainRequest) (*ChainDetailResponse, error) {
	resp, err := c.doRequest(ctx, http.MethodPost, fmt.Sprintf("/v1/chains/%s/cancel", chainID), req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var detail ChainDetailResponse
		if err := json.Unmarshal(body, &detail); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &detail, nil
	}

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: fmt.Sprintf("Chain not found: %s", chainID)}
	}
	if resp.StatusCode == http.StatusConflict {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Chain is not running"}
	}

	var errResp ErrorResponse
	if err := json.Unmarshal(body, &errResp); err != nil {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to cancel chain"}
	}
	return nil, &APIError{Code: errResp.Code, Message: errResp.Message, Retryable: errResp.Retryable}
}

// =============================================================================
// Dead Letter Queue (DLQ)
// =============================================================================

// DlqStats returns dead-letter queue statistics.
func (c *Client) DlqStats(ctx context.Context) (*DlqStatsResponse, error) {
	resp, err := c.doRequest(ctx, http.MethodGet, "/v1/dlq/stats", nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to get DLQ stats"}
	}

	var stats DlqStatsResponse
	if err := json.NewDecoder(resp.Body).Decode(&stats); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &stats, nil
}

// DlqDrain drains all entries from the dead-letter queue.
func (c *Client) DlqDrain(ctx context.Context) (*DlqDrainResponse, error) {
	resp, err := c.doRequest(ctx, http.MethodPost, "/v1/dlq/drain", nil)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode == http.StatusOK {
		var result DlqDrainResponse
		if err := json.Unmarshal(body, &result); err != nil {
			return nil, &ConnectionError{Message: err.Error()}
		}
		return &result, nil
	}

	if resp.StatusCode == http.StatusNotFound {
		return nil, &HTTPError{Status: resp.StatusCode, Message: "DLQ is not enabled"}
	}
	return nil, &HTTPError{Status: resp.StatusCode, Message: "Failed to drain DLQ"}
}

// =============================================================================
// Subscribe (SSE)
// =============================================================================

// Subscribe opens an SSE stream for a specific entity (chain, group, or action).
// It returns a channel that receives SseEvent values. The channel is closed when
// the context is cancelled, the connection drops, or the server closes the stream.
func (c *Client) Subscribe(ctx context.Context, entityType, entityID string, opts *SubscribeOptions) (<-chan *SseEvent, error) {
	params := url.Values{}
	if opts != nil {
		if opts.Namespace != nil {
			params.Set("namespace", *opts.Namespace)
		}
		if opts.Tenant != nil {
			params.Set("tenant", *opts.Tenant)
		}
		if opts.IncludeHistory != nil {
			params.Set("include_history", strconv.FormatBool(*opts.IncludeHistory))
		}
	}

	path := fmt.Sprintf("/v1/subscribe/%s/%s", entityType, entityID)
	if len(params) > 0 {
		path += "?" + params.Encode()
	}

	ch, err := c.openSSE(ctx, path, nil)
	if err != nil {
		return nil, err
	}
	return ch, nil
}

// =============================================================================
// Stream (SSE)
// =============================================================================

// Stream opens the general SSE event stream with optional filters.
// It returns a channel that receives SseEvent values. The channel is closed when
// the context is cancelled, the connection drops, or the server closes the stream.
func (c *Client) Stream(ctx context.Context, opts *StreamOptions) (<-chan *SseEvent, error) {
	params := url.Values{}
	var lastEventID *string
	if opts != nil {
		if opts.Namespace != nil {
			params.Set("namespace", *opts.Namespace)
		}
		if opts.ActionType != nil {
			params.Set("action_type", *opts.ActionType)
		}
		if opts.Outcome != nil {
			params.Set("outcome", *opts.Outcome)
		}
		if opts.EventType != nil {
			params.Set("event_type", *opts.EventType)
		}
		if opts.ChainID != nil {
			params.Set("chain_id", *opts.ChainID)
		}
		if opts.GroupID != nil {
			params.Set("group_id", *opts.GroupID)
		}
		if opts.ActionID != nil {
			params.Set("action_id", *opts.ActionID)
		}
		lastEventID = opts.LastEventID
	}

	path := "/v1/stream"
	if len(params) > 0 {
		path += "?" + params.Encode()
	}

	ch, err := c.openSSE(ctx, path, lastEventID)
	if err != nil {
		return nil, err
	}
	return ch, nil
}

// openSSE opens an SSE connection to the given path and returns a channel of events.
func (c *Client) openSSE(ctx context.Context, path string, lastEventID *string) (<-chan *SseEvent, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.baseURL+path, nil)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	req.Header.Set("Accept", "text/event-stream")
	req.Header.Set("Cache-Control", "no-cache")
	if c.apiKey != "" {
		req.Header.Set("Authorization", "Bearer "+c.apiKey)
	}
	if lastEventID != nil {
		req.Header.Set("Last-Event-ID", *lastEventID)
	}

	// Use a separate client without timeout for SSE (long-lived connection).
	sseClient := &http.Client{
		// No timeout -- the connection stays open until context cancellation.
	}

	resp, err := sseClient.Do(req)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		resp.Body.Close()
		return nil, &HTTPError{
			Status:  resp.StatusCode,
			Message: fmt.Sprintf("SSE connection failed: %s", string(body)),
		}
	}

	ch := make(chan *SseEvent, 64)

	go func() {
		defer close(ch)
		defer resp.Body.Close()

		scanner := bufio.NewScanner(resp.Body)

		var currentID string
		var currentEvent string
		var dataLines []string

		for scanner.Scan() {
			line := scanner.Text()

			if line == "" {
				// Empty line means end of event.
				if len(dataLines) > 0 {
					event := &SseEvent{
						ID:    currentID,
						Event: currentEvent,
						Data:  strings.Join(dataLines, "\n"),
					}
					select {
					case ch <- event:
					case <-ctx.Done():
						return
					}
				}
				currentID = ""
				currentEvent = ""
				dataLines = nil
				continue
			}

			if strings.HasPrefix(line, "id:") {
				currentID = strings.TrimSpace(strings.TrimPrefix(line, "id:"))
			} else if strings.HasPrefix(line, "event:") {
				currentEvent = strings.TrimSpace(strings.TrimPrefix(line, "event:"))
			} else if strings.HasPrefix(line, "data:") {
				dataLines = append(dataLines, strings.TrimSpace(strings.TrimPrefix(line, "data:")))
			}
			// Lines starting with ":" are comments (e.g., keep-alive pings); ignore them.
		}
	}()

	return ch, nil
}
