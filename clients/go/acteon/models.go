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
}

// OutcomeType represents the type of action outcome.
type OutcomeType string

const (
	OutcomeExecuted     OutcomeType = "executed"
	OutcomeDeduplicated OutcomeType = "deduplicated"
	OutcomeSuppressed   OutcomeType = "suppressed"
	OutcomeRerouted     OutcomeType = "rerouted"
	OutcomeThrottled    OutcomeType = "throttled"
	OutcomeFailed       OutcomeType = "failed"
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
