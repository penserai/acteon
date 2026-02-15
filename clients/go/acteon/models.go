// Package acteon provides a client for the Acteon action gateway.
package acteon

import (
	"encoding/json"
	"time"

	"github.com/google/uuid"
)

// Action represents an action to be dispatched through Acteon.
type Action struct {
	ID         string          `json:"id"`
	Namespace  string          `json:"namespace"`
	Tenant     string          `json:"tenant"`
	Provider   string          `json:"provider"`
	ActionType string          `json:"action_type"`
	Payload    map[string]any  `json:"payload"`
	DedupKey   string          `json:"dedup_key,omitempty"`
	Metadata   *ActionMetadata `json:"metadata,omitempty"`
	CreatedAt  time.Time       `json:"created_at"`
}

// ActionMetadata contains optional metadata for an action.
type ActionMetadata struct {
	Labels map[string]string `json:"labels,omitempty"`
}

// NewAction creates a new action with an auto-generated ID.
func NewAction(namespace, tenant, provider, actionType string, payload map[string]any) *Action {
	return &Action{
		ID:         uuid.New().String(),
		Namespace:  namespace,
		Tenant:     tenant,
		Provider:   provider,
		ActionType: actionType,
		Payload:    payload,
		CreatedAt:  time.Now().UTC(),
	}
}

// WithDedupKey sets the deduplication key.
func (a *Action) WithDedupKey(key string) *Action {
	a.DedupKey = key
	return a
}

// WithMetadata sets the metadata labels.
func (a *Action) WithMetadata(labels map[string]string) *Action {
	a.Metadata = &ActionMetadata{Labels: labels}
	return a
}

// ProviderResponse represents a response from a provider.
type ProviderResponse struct {
	Status  string            `json:"status"`
	Body    map[string]any    `json:"body"`
	Headers map[string]string `json:"headers"`
}

// ActionOutcome represents the outcome of dispatching an action.
type ActionOutcome struct {
	Type             OutcomeType
	Response         *ProviderResponse // For Executed, Rerouted
	Rule             string            // For Suppressed
	OriginalProvider string            // For Rerouted
	NewProvider      string            // For Rerouted
	RetryAfter       time.Duration     // For Throttled
	Error            *ActionError      // For Failed
	Verdict          string            // For DryRun
	MatchedRule      *string           // For DryRun
	WouldBeProvider  string            // For DryRun
	ActionID         string            // For Scheduled
	ScheduledFor     string            // For Scheduled
	Tenant           string            // For QuotaExceeded
	Limit            int64             // For QuotaExceeded
	Used             int64             // For QuotaExceeded
	OverageBehavior  string            // For QuotaExceeded
}

// OutcomeType represents the type of action outcome.
type OutcomeType string

const (
	OutcomeExecuted      OutcomeType = "executed"
	OutcomeDeduplicated  OutcomeType = "deduplicated"
	OutcomeSuppressed    OutcomeType = "suppressed"
	OutcomeRerouted      OutcomeType = "rerouted"
	OutcomeThrottled     OutcomeType = "throttled"
	OutcomeFailed        OutcomeType = "failed"
	OutcomeDryRun        OutcomeType = "dry_run"
	OutcomeScheduled     OutcomeType = "scheduled"
	OutcomeQuotaExceeded OutcomeType = "quota_exceeded"
)

// ActionError represents error details when an action fails.
type ActionError struct {
	Code      string `json:"code"`
	Message   string `json:"message"`
	Retryable bool   `json:"retryable"`
	Attempts  int    `json:"attempts"`
}

// UnmarshalJSON implements custom JSON unmarshaling for ActionOutcome.
func (o *ActionOutcome) UnmarshalJSON(data []byte) error {
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(data, &raw); err != nil {
		// Try as string (for "Deduplicated")
		var str string
		if err := json.Unmarshal(data, &str); err == nil && str == "Deduplicated" {
			o.Type = OutcomeDeduplicated
			return nil
		}
		return err
	}

	if _, ok := raw["Executed"]; ok {
		o.Type = OutcomeExecuted
		var resp ProviderResponse
		if err := json.Unmarshal(raw["Executed"], &resp); err != nil {
			return err
		}
		o.Response = &resp
		return nil
	}

	if _, ok := raw["Deduplicated"]; ok {
		o.Type = OutcomeDeduplicated
		return nil
	}

	if suppressed, ok := raw["Suppressed"]; ok {
		o.Type = OutcomeSuppressed
		var s struct {
			Rule string `json:"rule"`
		}
		if err := json.Unmarshal(suppressed, &s); err != nil {
			return err
		}
		o.Rule = s.Rule
		return nil
	}

	if rerouted, ok := raw["Rerouted"]; ok {
		o.Type = OutcomeRerouted
		var r struct {
			OriginalProvider string           `json:"original_provider"`
			NewProvider      string           `json:"new_provider"`
			Response         ProviderResponse `json:"response"`
		}
		if err := json.Unmarshal(rerouted, &r); err != nil {
			return err
		}
		o.OriginalProvider = r.OriginalProvider
		o.NewProvider = r.NewProvider
		o.Response = &r.Response
		return nil
	}

	if throttled, ok := raw["Throttled"]; ok {
		o.Type = OutcomeThrottled
		var t struct {
			RetryAfter struct {
				Secs  int64 `json:"secs"`
				Nanos int64 `json:"nanos"`
			} `json:"retry_after"`
		}
		if err := json.Unmarshal(throttled, &t); err != nil {
			return err
		}
		o.RetryAfter = time.Duration(t.RetryAfter.Secs)*time.Second +
			time.Duration(t.RetryAfter.Nanos)*time.Nanosecond
		return nil
	}

	if failed, ok := raw["Failed"]; ok {
		o.Type = OutcomeFailed
		var e ActionError
		if err := json.Unmarshal(failed, &e); err != nil {
			return err
		}
		o.Error = &e
		return nil
	}

	if dryRun, ok := raw["DryRun"]; ok {
		o.Type = OutcomeDryRun
		var d struct {
			Verdict         string  `json:"verdict"`
			MatchedRule     *string `json:"matched_rule"`
			WouldBeProvider string  `json:"would_be_provider"`
		}
		if err := json.Unmarshal(dryRun, &d); err != nil {
			return err
		}
		o.Verdict = d.Verdict
		o.MatchedRule = d.MatchedRule
		o.WouldBeProvider = d.WouldBeProvider
		return nil
	}

	if scheduled, ok := raw["Scheduled"]; ok {
		o.Type = OutcomeScheduled
		var s struct {
			ActionID     string `json:"action_id"`
			ScheduledFor string `json:"scheduled_for"`
		}
		if err := json.Unmarshal(scheduled, &s); err != nil {
			return err
		}
		o.ActionID = s.ActionID
		o.ScheduledFor = s.ScheduledFor
		return nil
	}

	if quotaExceeded, ok := raw["QuotaExceeded"]; ok {
		o.Type = OutcomeQuotaExceeded
		var q struct {
			Tenant          string `json:"tenant"`
			Limit           int64  `json:"limit"`
			Used            int64  `json:"used"`
			OverageBehavior string `json:"overage_behavior"`
		}
		if err := json.Unmarshal(quotaExceeded, &q); err != nil {
			return err
		}
		o.Tenant = q.Tenant
		o.Limit = q.Limit
		o.Used = q.Used
		o.OverageBehavior = q.OverageBehavior
		return nil
	}

	o.Type = OutcomeFailed
	o.Error = &ActionError{Code: "UNKNOWN", Message: "Unknown outcome"}
	return nil
}

// IsExecuted returns true if the outcome is Executed.
func (o *ActionOutcome) IsExecuted() bool { return o.Type == OutcomeExecuted }

// IsDeduplicated returns true if the outcome is Deduplicated.
func (o *ActionOutcome) IsDeduplicated() bool { return o.Type == OutcomeDeduplicated }

// IsSuppressed returns true if the outcome is Suppressed.
func (o *ActionOutcome) IsSuppressed() bool { return o.Type == OutcomeSuppressed }

// IsRerouted returns true if the outcome is Rerouted.
func (o *ActionOutcome) IsRerouted() bool { return o.Type == OutcomeRerouted }

// IsThrottled returns true if the outcome is Throttled.
func (o *ActionOutcome) IsThrottled() bool { return o.Type == OutcomeThrottled }

// IsFailed returns true if the outcome is Failed.
func (o *ActionOutcome) IsFailed() bool { return o.Type == OutcomeFailed }

// IsDryRun returns true if the outcome is DryRun.
func (o *ActionOutcome) IsDryRun() bool { return o.Type == OutcomeDryRun }

// IsScheduled returns true if the outcome is Scheduled.
func (o *ActionOutcome) IsScheduled() bool { return o.Type == OutcomeScheduled }

// IsQuotaExceeded returns true if the outcome is QuotaExceeded.
func (o *ActionOutcome) IsQuotaExceeded() bool { return o.Type == OutcomeQuotaExceeded }

// ErrorResponse represents an error response from the API.
type ErrorResponse struct {
	Code      string `json:"code"`
	Message   string `json:"message"`
	Retryable bool   `json:"retryable"`
}

// BatchResult represents a result from a batch dispatch operation.
type BatchResult struct {
	Success bool
	Outcome *ActionOutcome
	Error   *ErrorResponse
}

// UnmarshalJSON implements custom JSON unmarshaling for BatchResult.
func (r *BatchResult) UnmarshalJSON(data []byte) error {
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(data, &raw); err != nil {
		return err
	}

	if errData, ok := raw["error"]; ok {
		r.Success = false
		var e ErrorResponse
		if err := json.Unmarshal(errData, &e); err != nil {
			return err
		}
		r.Error = &e
		return nil
	}

	r.Success = true
	var outcome ActionOutcome
	if err := json.Unmarshal(data, &outcome); err != nil {
		return err
	}
	r.Outcome = &outcome
	return nil
}

// RuleInfo contains information about a loaded rule.
type RuleInfo struct {
	Name        string  `json:"name"`
	Priority    int     `json:"priority"`
	Enabled     bool    `json:"enabled"`
	Description *string `json:"description,omitempty"`
}

// ReloadResult represents the result of reloading rules.
type ReloadResult struct {
	Loaded int      `json:"loaded"`
	Errors []string `json:"errors"`
}

// AuditQuery contains query parameters for audit search.
type AuditQuery struct {
	Namespace  string
	Tenant     string
	Provider   string
	ActionType string
	Outcome    string
	Limit      int
	Offset     int
}

// AuditRecord represents an audit record.
type AuditRecord struct {
	ID           string  `json:"id"`
	ActionID     string  `json:"action_id"`
	Namespace    string  `json:"namespace"`
	Tenant       string  `json:"tenant"`
	Provider     string  `json:"provider"`
	ActionType   string  `json:"action_type"`
	Verdict      string  `json:"verdict"`
	Outcome      string  `json:"outcome"`
	MatchedRule  *string `json:"matched_rule,omitempty"`
	DurationMs   int64   `json:"duration_ms"`
	DispatchedAt string  `json:"dispatched_at"`
}

// AuditPage represents paginated audit results.
type AuditPage struct {
	Records []AuditRecord `json:"records"`
	Total   int64         `json:"total"`
	Limit   int64         `json:"limit"`
	Offset  int64         `json:"offset"`
}

// =============================================================================
// Event Types (State Machine Lifecycle)
// =============================================================================

// EventQuery contains query parameters for listing events.
type EventQuery struct {
	Namespace string
	Tenant    string
	Status    string
	Limit     int
}

// EventState represents the current state of an event.
type EventState struct {
	Fingerprint string  `json:"fingerprint"`
	State       string  `json:"state"`
	ActionType  *string `json:"action_type,omitempty"`
	UpdatedAt   *string `json:"updated_at,omitempty"`
}

// EventListResponse represents the response from listing events.
type EventListResponse struct {
	Events []EventState `json:"events"`
	Count  int          `json:"count"`
}

// TransitionResponse represents the response from transitioning an event.
type TransitionResponse struct {
	Fingerprint   string `json:"fingerprint"`
	PreviousState string `json:"previous_state"`
	NewState      string `json:"new_state"`
	Notify        bool   `json:"notify"`
}

// =============================================================================
// Group Types (Event Batching)
// =============================================================================

// GroupSummary represents a summary of an event group.
type GroupSummary struct {
	GroupID    string  `json:"group_id"`
	GroupKey   string  `json:"group_key"`
	EventCount int     `json:"event_count"`
	State      string  `json:"state"`
	NotifyAt   *string `json:"notify_at,omitempty"`
	CreatedAt  *string `json:"created_at,omitempty"`
}

// GroupListResponse represents the response from listing groups.
type GroupListResponse struct {
	Groups []GroupSummary `json:"groups"`
	Total  int            `json:"total"`
}

// GroupDetail represents detailed information about a group.
type GroupDetail struct {
	Group  GroupSummary      `json:"group"`
	Events []string          `json:"events"`
	Labels map[string]string `json:"labels"`
}

// FlushGroupResponse represents the response from flushing a group.
type FlushGroupResponse struct {
	GroupID    string `json:"group_id"`
	EventCount int    `json:"event_count"`
	Notified   bool   `json:"notified"`
}

// =============================================================================
// Approval Types (Human-in-the-Loop)
// =============================================================================

// ApprovalActionResponse represents the response from approving or rejecting an action.
type ApprovalActionResponse struct {
	ID      string         `json:"id"`
	Status  string         `json:"status"`
	Outcome map[string]any `json:"outcome,omitempty"`
}

// ApprovalStatus represents the public-facing approval status (no payload exposed).
type ApprovalStatus struct {
	Token     string  `json:"token"`
	Status    string  `json:"status"`
	Rule      string  `json:"rule"`
	CreatedAt string  `json:"created_at"`
	ExpiresAt string  `json:"expires_at"`
	DecidedAt *string `json:"decided_at,omitempty"`
	Message   *string `json:"message,omitempty"`
}

// ApprovalListResponse represents the response from listing pending approvals.
type ApprovalListResponse struct {
	Approvals []ApprovalStatus `json:"approvals"`
	Count     int              `json:"count"`
}

// =============================================================================
// Webhook Helpers
// =============================================================================

// WebhookPayload represents the payload for a webhook action.
//
// Use this to build the payload for an Action targeted at the webhook provider.
type WebhookPayload struct {
	// URL is the target URL for the webhook request.
	URL string `json:"url"`
	// Method is the HTTP method (default: "POST").
	Method string `json:"method"`
	// Body is the JSON body to send to the webhook endpoint.
	Body map[string]any `json:"body"`
	// Headers contains additional HTTP headers to include.
	Headers map[string]string `json:"headers,omitempty"`
}

// ToPayload converts the WebhookPayload to a generic map suitable for an Action payload.
func (w *WebhookPayload) ToPayload() map[string]any {
	result := map[string]any{
		"url":    w.URL,
		"method": w.Method,
		"body":   w.Body,
	}
	if len(w.Headers) > 0 {
		result["headers"] = w.Headers
	}
	return result
}

// NewWebhookAction creates an Action targeting the webhook provider.
//
// This is a convenience function that constructs a properly formatted Action
// for the webhook provider, wrapping the URL, method, headers, and body into
// the payload.
func NewWebhookAction(namespace, tenant, url string, body map[string]any) *Action {
	payload := &WebhookPayload{
		URL:    url,
		Method: "POST",
		Body:   body,
	}
	return NewAction(namespace, tenant, "webhook", "webhook", payload.ToPayload())
}

// NewWebhookActionWithOptions creates a webhook Action with additional options.
func NewWebhookActionWithOptions(namespace, tenant, url, method string, body map[string]any, headers map[string]string) *Action {
	payload := &WebhookPayload{
		URL:     url,
		Method:  method,
		Body:    body,
		Headers: headers,
	}
	return NewAction(namespace, tenant, "webhook", "webhook", payload.ToPayload())
}

// WithWebhookMethod sets a custom HTTP method for a webhook action.
func (a *Action) WithWebhookMethod(method string) *Action {
	if a.Payload != nil {
		a.Payload["method"] = method
	}
	return a
}

// WithWebhookHeaders sets additional headers for a webhook action.
func (a *Action) WithWebhookHeaders(headers map[string]string) *Action {
	if a.Payload != nil {
		a.Payload["headers"] = headers
	}
	return a
}

// ReplayResult is the result of replaying a single action.
type ReplayResult struct {
	OriginalActionID string  `json:"original_action_id"`
	NewActionID      string  `json:"new_action_id"`
	Success          bool    `json:"success"`
	Error            *string `json:"error,omitempty"`
}

// ReplaySummary is the summary of a bulk replay operation.
type ReplaySummary struct {
	Replayed int            `json:"replayed"`
	Failed   int            `json:"failed"`
	Skipped  int            `json:"skipped"`
	Results  []ReplayResult `json:"results"`
}

// ReplayQuery contains query parameters for bulk audit replay.
type ReplayQuery struct {
	Namespace   string
	Tenant      string
	Provider    string
	ActionType  string
	Outcome     string
	Verdict     string
	MatchedRule string
	From        string
	To          string
	Limit       int
}

// =============================================================================
// Recurring Action Types
// =============================================================================

// CreateRecurringAction is the request to create a recurring action.
type CreateRecurringAction struct {
	Namespace      string            `json:"namespace"`
	Tenant         string            `json:"tenant"`
	Provider       string            `json:"provider"`
	ActionType     string            `json:"action_type"`
	Payload        map[string]any    `json:"payload"`
	CronExpression string            `json:"cron_expression"`
	Name           string            `json:"name,omitempty"`
	Metadata       map[string]string `json:"metadata,omitempty"`
	Timezone       string            `json:"timezone,omitempty"`
	EndDate        string            `json:"end_date,omitempty"`
	MaxExecutions  *int              `json:"max_executions,omitempty"`
	Description    string            `json:"description,omitempty"`
	DedupKey       string            `json:"dedup_key,omitempty"`
	Labels         map[string]string `json:"labels,omitempty"`
}

// CreateRecurringResponse is the response from creating a recurring action.
type CreateRecurringResponse struct {
	ID              string  `json:"id"`
	Status          string  `json:"status"`
	Name            *string `json:"name,omitempty"`
	NextExecutionAt *string `json:"next_execution_at,omitempty"`
}

// RecurringFilter contains query parameters for listing recurring actions.
type RecurringFilter struct {
	Namespace string
	Tenant    string
	Status    string
	Limit     int
	Offset    int
}

// RecurringSummary is a summary of a recurring action in list responses.
type RecurringSummary struct {
	ID              string  `json:"id"`
	Namespace       string  `json:"namespace"`
	Tenant          string  `json:"tenant"`
	CronExpr        string  `json:"cron_expr"`
	Timezone        string  `json:"timezone"`
	Enabled         bool    `json:"enabled"`
	Provider        string  `json:"provider"`
	ActionType      string  `json:"action_type"`
	ExecutionCount  int     `json:"execution_count"`
	CreatedAt       string  `json:"created_at"`
	NextExecutionAt *string `json:"next_execution_at,omitempty"`
	Description     *string `json:"description,omitempty"`
}

// ListRecurringResponse is the response from listing recurring actions.
type ListRecurringResponse struct {
	RecurringActions []RecurringSummary `json:"recurring_actions"`
	Count            int               `json:"count"`
}

// RecurringDetail is detailed information about a recurring action.
type RecurringDetail struct {
	ID              string            `json:"id"`
	Namespace       string            `json:"namespace"`
	Tenant          string            `json:"tenant"`
	CronExpr        string            `json:"cron_expr"`
	Timezone        string            `json:"timezone"`
	Enabled         bool              `json:"enabled"`
	Provider        string            `json:"provider"`
	ActionType      string            `json:"action_type"`
	Payload         map[string]any    `json:"payload"`
	Metadata        map[string]string `json:"metadata"`
	ExecutionCount  int               `json:"execution_count"`
	CreatedAt       string            `json:"created_at"`
	UpdatedAt       string            `json:"updated_at"`
	Labels          map[string]string `json:"labels"`
	NextExecutionAt *string           `json:"next_execution_at,omitempty"`
	LastExecutedAt  *string           `json:"last_executed_at,omitempty"`
	EndsAt          *string           `json:"ends_at,omitempty"`
	Description     *string           `json:"description,omitempty"`
	DedupKey        *string           `json:"dedup_key,omitempty"`
}

// UpdateRecurringAction is the request to update a recurring action.
type UpdateRecurringAction struct {
	Namespace      string            `json:"namespace"`
	Tenant         string            `json:"tenant"`
	Name           *string           `json:"name,omitempty"`
	Payload        map[string]any    `json:"payload,omitempty"`
	Metadata       map[string]string `json:"metadata,omitempty"`
	CronExpression *string           `json:"cron_expression,omitempty"`
	Timezone       *string           `json:"timezone,omitempty"`
	EndDate        *string           `json:"end_date,omitempty"`
	MaxExecutions  *int              `json:"max_executions,omitempty"`
	Description    *string           `json:"description,omitempty"`
	DedupKey       *string           `json:"dedup_key,omitempty"`
	Labels         map[string]string `json:"labels,omitempty"`
}

// RecurringLifecycleRequest is the body for pause/resume endpoints.
type RecurringLifecycleRequest struct {
	Namespace string `json:"namespace"`
	Tenant    string `json:"tenant"`
}

// =============================================================================
// Quota Types
// =============================================================================

// CreateQuotaRequest is the request to create a quota policy.
type CreateQuotaRequest struct {
	Namespace       string            `json:"namespace"`
	Tenant          string            `json:"tenant"`
	MaxActions      int64             `json:"max_actions"`
	Window          string            `json:"window"`
	OverageBehavior string            `json:"overage_behavior"`
	Description     string            `json:"description,omitempty"`
	Labels          map[string]string `json:"labels,omitempty"`
}

// UpdateQuotaRequest is the request to update a quota policy.
type UpdateQuotaRequest struct {
	Namespace       string  `json:"namespace"`
	Tenant          string  `json:"tenant"`
	MaxActions      *int64  `json:"max_actions,omitempty"`
	Window          *string `json:"window,omitempty"`
	OverageBehavior *string `json:"overage_behavior,omitempty"`
	Description     *string `json:"description,omitempty"`
	Enabled         *bool   `json:"enabled,omitempty"`
}

// QuotaPolicy represents a quota policy.
type QuotaPolicy struct {
	ID              string            `json:"id"`
	Namespace       string            `json:"namespace"`
	Tenant          string            `json:"tenant"`
	MaxActions      int64             `json:"max_actions"`
	Window          string            `json:"window"`
	OverageBehavior string            `json:"overage_behavior"`
	Enabled         bool              `json:"enabled"`
	CreatedAt       string            `json:"created_at"`
	UpdatedAt       string            `json:"updated_at"`
	Description     *string           `json:"description,omitempty"`
	Labels          map[string]string `json:"labels,omitempty"`
}

// ListQuotasResponse is the response from listing quota policies.
type ListQuotasResponse struct {
	Quotas []QuotaPolicy `json:"quotas"`
	Count  int           `json:"count"`
}

// QuotaUsage represents current usage statistics for a quota.
type QuotaUsage struct {
	Tenant          string `json:"tenant"`
	Namespace       string `json:"namespace"`
	Used            int64  `json:"used"`
	Limit           int64  `json:"limit"`
	Remaining       int64  `json:"remaining"`
	Window          string `json:"window"`
	ResetsAt        string `json:"resets_at"`
	OverageBehavior string `json:"overage_behavior"`
}

// =============================================================================
// Retention Policy Types
// =============================================================================

// CreateRetentionRequest is the request to create a retention policy.
type CreateRetentionRequest struct {
	Namespace       string            `json:"namespace"`
	Tenant          string            `json:"tenant"`
	AuditTTLSeconds int64             `json:"audit_ttl_seconds"`
	StateTTLSeconds int64             `json:"state_ttl_seconds"`
	EventTTLSeconds int64             `json:"event_ttl_seconds"`
	ComplianceHold  bool              `json:"compliance_hold,omitempty"`
	Description     string            `json:"description,omitempty"`
	Labels          map[string]string `json:"labels,omitempty"`
}

// UpdateRetentionRequest is the request to update a retention policy.
type UpdateRetentionRequest struct {
	Enabled         *bool             `json:"enabled,omitempty"`
	AuditTTLSeconds *int64            `json:"audit_ttl_seconds,omitempty"`
	StateTTLSeconds *int64            `json:"state_ttl_seconds,omitempty"`
	EventTTLSeconds *int64            `json:"event_ttl_seconds,omitempty"`
	ComplianceHold  *bool             `json:"compliance_hold,omitempty"`
	Description     *string           `json:"description,omitempty"`
	Labels          map[string]string `json:"labels,omitempty"`
}

// RetentionPolicy represents a retention policy.
type RetentionPolicy struct {
	ID              string            `json:"id"`
	Namespace       string            `json:"namespace"`
	Tenant          string            `json:"tenant"`
	Enabled         bool              `json:"enabled"`
	AuditTTLSeconds int64             `json:"audit_ttl_seconds"`
	StateTTLSeconds int64             `json:"state_ttl_seconds"`
	EventTTLSeconds int64             `json:"event_ttl_seconds"`
	ComplianceHold  bool              `json:"compliance_hold"`
	CreatedAt       string            `json:"created_at"`
	UpdatedAt       string            `json:"updated_at"`
	Description     *string           `json:"description,omitempty"`
	Labels          map[string]string `json:"labels,omitempty"`
}

// ListRetentionResponse is the response from listing retention policies.
type ListRetentionResponse struct {
	Policies []RetentionPolicy `json:"policies"`
	Count    int               `json:"count"`
}

// =============================================================================
// Chain Types
// =============================================================================

// ChainSummary is a summary of a chain execution for list responses.
type ChainSummary struct {
	ChainID     string `json:"chain_id"`
	ChainName   string `json:"chain_name"`
	Status      string `json:"status"`
	CurrentStep int    `json:"current_step"`
	TotalSteps  int    `json:"total_steps"`
	StartedAt   string `json:"started_at"`
	UpdatedAt   string `json:"updated_at"`
}

// ListChainsResponse is the response from listing chain executions.
type ListChainsResponse struct {
	Chains []ChainSummary `json:"chains"`
}

// ChainStepStatus is the detailed status of a single chain step.
type ChainStepStatus struct {
	Name         string         `json:"name"`
	Provider     string         `json:"provider"`
	Status       string         `json:"status"`
	ResponseBody map[string]any `json:"response_body,omitempty"`
	Error        *string        `json:"error,omitempty"`
	CompletedAt  *string        `json:"completed_at,omitempty"`
}

// ChainDetailResponse is the full detail response for a chain execution.
type ChainDetailResponse struct {
	ChainID       string            `json:"chain_id"`
	ChainName     string            `json:"chain_name"`
	Status        string            `json:"status"`
	CurrentStep   int               `json:"current_step"`
	TotalSteps    int               `json:"total_steps"`
	Steps         []ChainStepStatus `json:"steps"`
	StartedAt     string            `json:"started_at"`
	UpdatedAt     string            `json:"updated_at"`
	ExpiresAt     *string           `json:"expires_at,omitempty"`
	CancelReason  *string           `json:"cancel_reason,omitempty"`
	CancelledBy   *string           `json:"cancelled_by,omitempty"`
	ExecutionPath []string          `json:"execution_path,omitempty"`
}

// CancelChainRequest is the request body for cancelling a chain.
type CancelChainRequest struct {
	Namespace   string  `json:"namespace"`
	Tenant      string  `json:"tenant"`
	Reason      *string `json:"reason,omitempty"`
	CancelledBy *string `json:"cancelled_by,omitempty"`
}

// =============================================================================
// DLQ Types (Dead Letter Queue)
// =============================================================================

// DlqStatsResponse is the response from the DLQ stats endpoint.
type DlqStatsResponse struct {
	Enabled bool `json:"enabled"`
	Count   int  `json:"count"`
}

// DlqEntry is a single dead-letter queue entry.
type DlqEntry struct {
	ActionID   string `json:"action_id"`
	Namespace  string `json:"namespace"`
	Tenant     string `json:"tenant"`
	Provider   string `json:"provider"`
	ActionType string `json:"action_type"`
	Error      string `json:"error"`
	Attempts   int    `json:"attempts"`
	Timestamp  uint64 `json:"timestamp"`
}

// DlqDrainResponse is the response from draining the DLQ.
type DlqDrainResponse struct {
	Entries []DlqEntry `json:"entries"`
	Count   int        `json:"count"`
}

// =============================================================================
// Rule Evaluation Types (Rule Playground)
// =============================================================================

// EvaluateRulesRequest is the request body for rule evaluation.
type EvaluateRulesRequest struct {
	Namespace       string                 `json:"namespace"`
	Tenant          string                 `json:"tenant"`
	Provider        string                 `json:"provider"`
	ActionType      string                 `json:"action_type"`
	Payload         map[string]interface{} `json:"payload"`
	Metadata        map[string]string      `json:"metadata,omitempty"`
	IncludeDisabled bool                   `json:"include_disabled,omitempty"`
	EvaluateAll     bool                   `json:"evaluate_all,omitempty"`
	EvaluateAt      *string                `json:"evaluate_at,omitempty"`
	MockState       map[string]string      `json:"mock_state,omitempty"`
}

// SemanticMatchDetail contains details about a semantic match evaluation.
type SemanticMatchDetail struct {
	// ExtractedText is the text that was extracted and compared.
	ExtractedText string `json:"extracted_text"`
	// Topic is the topic the text was compared against.
	Topic string `json:"topic"`
	// Similarity is the computed similarity score.
	Similarity float64 `json:"similarity"`
	// Threshold is the threshold that was configured on the rule.
	Threshold float64 `json:"threshold"`
}

// RuleTraceEntry is a per-rule trace entry from rule evaluation.
type RuleTraceEntry struct {
	RuleName               string               `json:"rule_name"`
	Priority               int                  `json:"priority"`
	Enabled                bool                 `json:"enabled"`
	ConditionDisplay       string               `json:"condition_display"`
	Result                 string               `json:"result"`
	EvaluationDuration     uint64               `json:"evaluation_duration_us"`
	Action                 string               `json:"action"`
	Source                 string               `json:"source"`
	Description            *string              `json:"description,omitempty"`
	SkipReason             *string              `json:"skip_reason,omitempty"`
	Error                  *string              `json:"error,omitempty"`
	SemanticDetails        *SemanticMatchDetail `json:"semantic_details,omitempty"`
	ModifyPatch            json.RawMessage      `json:"modify_patch,omitempty"`
	ModifiedPayloadPreview json.RawMessage      `json:"modified_payload_preview,omitempty"`
}

// TraceContext holds contextual information from rule evaluation.
type TraceContext struct {
	Time              map[string]interface{} `json:"time"`
	EnvironmentKeys   []string               `json:"environment_keys"`
	AccessedStateKeys []string               `json:"accessed_state_keys,omitempty"`
	EffectiveTimezone *string                `json:"effective_timezone,omitempty"`
}

// EvaluateRulesResponse is the response from rule evaluation.
type EvaluateRulesResponse struct {
	Verdict             string                 `json:"verdict"`
	MatchedRule         *string                `json:"matched_rule,omitempty"`
	HasErrors           bool                   `json:"has_errors"`
	TotalRulesEvaluated int                    `json:"total_rules_evaluated"`
	TotalRulesSkipped   int                    `json:"total_rules_skipped"`
	EvaluationDuration  uint64                 `json:"evaluation_duration_us"`
	Trace               []RuleTraceEntry       `json:"trace"`
	Context             TraceContext            `json:"context"`
	ModifiedPayload     map[string]interface{} `json:"modified_payload,omitempty"`
}

// =============================================================================
// SSE Types (Server-Sent Events)
// =============================================================================

// SubscribeOptions contains optional parameters for the Subscribe method.
type SubscribeOptions struct {
	Namespace      *string
	Tenant         *string
	IncludeHistory *bool
}

// StreamOptions contains optional filter parameters for the Stream method.
type StreamOptions struct {
	Namespace   *string
	ActionType  *string
	Outcome     *string
	EventType   *string
	ChainID     *string
	GroupID     *string
	ActionID    *string
	LastEventID *string
}

// SseEvent represents a single Server-Sent Event.
type SseEvent struct {
	ID    string `json:"id,omitempty"`
	Event string `json:"event,omitempty"`
	Data  string `json:"data,omitempty"`
}

// =============================================================================
// Provider Health Types
// =============================================================================

// ProviderHealthStatus represents health and metrics for a single provider.
type ProviderHealthStatus struct {
	Provider             string   `json:"provider"`
	Healthy              bool     `json:"healthy"`
	HealthCheckError     *string  `json:"health_check_error,omitempty"`
	CircuitBreakerState  string   `json:"circuit_breaker_state"`
	TotalRequests        int      `json:"total_requests"`
	Successes            int      `json:"successes"`
	Failures             int      `json:"failures"`
	SuccessRate          float64  `json:"success_rate"`
	AvgLatencyMs         float64  `json:"avg_latency_ms"`
	P50LatencyMs         float64  `json:"p50_latency_ms"`
	P95LatencyMs         float64  `json:"p95_latency_ms"`
	P99LatencyMs         float64  `json:"p99_latency_ms"`
	LastRequestAt        *int64   `json:"last_request_at,omitempty"`
	LastError            *string  `json:"last_error,omitempty"`
}

// ListProviderHealthResponse is the response from listing provider health.
type ListProviderHealthResponse struct {
	Providers []ProviderHealthStatus `json:"providers"`
}

// =============================================================================
// Provider Payload Helpers
// =============================================================================

// NewTwilioSmsPayload creates a payload for the Twilio SMS provider.
func NewTwilioSmsPayload(to, body string) map[string]any {
	return map[string]any{
		"to":   to,
		"body": body,
	}
}

// NewTwilioSmsPayloadWithOptions creates a Twilio SMS payload with optional fields.
func NewTwilioSmsPayloadWithOptions(to, body string, from string, mediaURL string) map[string]any {
	p := NewTwilioSmsPayload(to, body)
	if from != "" {
		p["from"] = from
	}
	if mediaURL != "" {
		p["media_url"] = mediaURL
	}
	return p
}

// NewTeamsMessagePayload creates a payload for the Microsoft Teams provider.
func NewTeamsMessagePayload(text string) map[string]any {
	return map[string]any{
		"text": text,
	}
}

// NewTeamsMessagePayloadWithOptions creates a Teams message payload with optional fields.
func NewTeamsMessagePayloadWithOptions(text, title, themeColor string) map[string]any {
	p := NewTeamsMessagePayload(text)
	if title != "" {
		p["title"] = title
	}
	if themeColor != "" {
		p["theme_color"] = themeColor
	}
	return p
}

// NewTeamsAdaptiveCardPayload creates a payload for Teams with an Adaptive Card.
func NewTeamsAdaptiveCardPayload(card map[string]any) map[string]any {
	return map[string]any{
		"adaptive_card": card,
	}
}

// NewDiscordMessagePayload creates a payload for the Discord webhook provider.
func NewDiscordMessagePayload(content string) map[string]any {
	return map[string]any{
		"content": content,
	}
}

// NewDiscordEmbedPayload creates a Discord payload with an embed.
func NewDiscordEmbedPayload(embeds []map[string]any) map[string]any {
	return map[string]any{
		"embeds": embeds,
	}
}

// NewDiscordMessagePayloadWithOptions creates a Discord payload with all options.
func NewDiscordMessagePayloadWithOptions(content, username, avatarURL string, embeds []map[string]any) map[string]any {
	p := map[string]any{}
	if content != "" {
		p["content"] = content
	}
	if username != "" {
		p["username"] = username
	}
	if avatarURL != "" {
		p["avatar_url"] = avatarURL
	}
	if len(embeds) > 0 {
		p["embeds"] = embeds
	}
	return p
}
