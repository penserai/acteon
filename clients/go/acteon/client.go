package acteon

import (
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
func (c *Client) Approve(ctx context.Context, namespace, tenant, id, sig string, expiresAt int64) (*ApprovalActionResponse, error) {
	params := url.Values{}
	params.Set("sig", sig)
	params.Set("expires_at", strconv.FormatInt(expiresAt, 10))
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
func (c *Client) Reject(ctx context.Context, namespace, tenant, id, sig string, expiresAt int64) (*ApprovalActionResponse, error) {
	params := url.Values{}
	params.Set("sig", sig)
	params.Set("expires_at", strconv.FormatInt(expiresAt, 10))
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
func (c *Client) GetApproval(ctx context.Context, namespace, tenant, id, sig string, expiresAt int64) (*ApprovalStatus, error) {
	params := url.Values{}
	params.Set("sig", sig)
	params.Set("expires_at", strconv.FormatInt(expiresAt, 10))
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
