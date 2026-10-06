package acteon

import (
	"context"
	"encoding/json"
	"net/url"
	"strconv"
)

type GovernanceResource struct {
	Kind      string `json:"kind"`
	Namespace string `json:"namespace"`
	Tenant    string `json:"tenant"`
	ID        string `json:"id"`
}
type GovernanceLimits struct {
	MaxUnits      uint64 `json:"max_units"`
	MaxConcurrent uint64 `json:"max_concurrent"`
	DeadlineMs    int64  `json:"deadline_ms"`
}
type GovernanceRoute struct {
	Provider   string `json:"provider"`
	ActionType string `json:"action_type"`
}
type GovernanceEffect struct {
	Operation string               `json:"operation"`
	Resources []GovernanceResource `json:"resources"`
}
type GovernancePermitDeclaration struct {
	ID          string            `json:"id"`
	Revision    uint64            `json:"revision"`
	Subject     PrincipalIdentity `json:"subject"`
	Routes      []GovernanceRoute `json:"routes"`
	ValidFromMs int64             `json:"valid_from_ms"`
	Limits      GovernanceLimits  `json:"limits"`
}
type PublishGovernancePermitRequest struct {
	Namespace        string                      `json:"namespace"`
	Tenant           string                      `json:"tenant"`
	ChangeID         string                      `json:"change_id"`
	ExpectedRevision uint64                      `json:"expected_revision"`
	Permit           GovernancePermitDeclaration `json:"permit"`
	Reason           string                      `json:"reason"`
}

// Exactly the fields appropriate to Kind must be set; the server rejects unknown fields.
type GovernanceIntervention struct {
	Kind             string              `json:"kind"`
	Resource         *GovernanceResource `json:"resource,omitempty"`
	Subject          *PrincipalIdentity  `json:"subject,omitempty"`
	PermitID         string              `json:"permit_id,omitempty"`
	CredentialID     string              `json:"credential_id,omitempty"`
	ExpectedRevision *uint64             `json:"expected_revision,omitempty"`
}
type GovernanceInterventionRequest struct {
	Namespace string                 `json:"namespace"`
	Tenant    string                 `json:"tenant"`
	ChangeID  string                 `json:"change_id"`
	Change    GovernanceIntervention `json:"change"`
	Reason    string                 `json:"reason"`
}
type GovernanceChangeReceipt struct {
	Namespace  string `json:"namespace"`
	Tenant     string `json:"tenant"`
	ChangeID   string `json:"change_id"`
	Actor      string `json:"actor"`
	Reason     string `json:"reason"`
	Generation uint64 `json:"generation"`
	Pending    bool   `json:"pending"`
}
type GovernancePermitView struct {
	ID          string             `json:"id"`
	Revision    uint64             `json:"revision"`
	Subject     PrincipalIdentity  `json:"subject"`
	Effects     []GovernanceEffect `json:"effects"`
	ValidFromMs int64              `json:"valid_from_ms"`
	Limits      GovernanceLimits   `json:"limits"`
	Revoked     bool               `json:"revoked"`
}
type GovernanceRouteView struct {
	Route  GovernanceRoute  `json:"route"`
	Effect GovernanceEffect `json:"effect"`
	Closed bool             `json:"closed"`
}
type GovernanceManagementBounds struct {
	Subjects        []PrincipalIdentity `json:"subjects"`
	CanIssuePermits bool                `json:"can_issue_permits"`
	CanIntervene    bool                `json:"can_intervene"`
	CanReadHistory  bool                `json:"can_read_history,omitempty"`
	CanReconcile    bool                `json:"can_reconcile,omitempty"`
	ValidFromMs     int64               `json:"valid_from_ms"`
	Limits          GovernanceLimits    `json:"limits"`
}
type GovernanceScopeView struct {
	Management      GovernanceManagementBounds `json:"management"`
	Namespace       string                     `json:"namespace"`
	Tenant          string                     `json:"tenant"`
	Incarnation     string                     `json:"incarnation"`
	Generation      uint64                     `json:"generation"`
	Routes          []GovernanceRouteView      `json:"routes"`
	Permits         []GovernancePermitView     `json:"permits"`
	ClosedResources []GovernanceResource       `json:"closed_resources"`
	RevokedSubjects []string                   `json:"revoked_subjects"`
}

func (c *Client) Governance(ctx context.Context, namespace, tenant string) (*GovernanceScopeView, error) {
	data, err := c.PlatformRequest(ctx, OpGovernanceInspect, nil, url.Values{"namespace": {namespace}, "tenant": {tenant}}, nil)
	if err != nil {
		return nil, err
	}
	var result GovernanceScopeView
	err = json.Unmarshal(data, &result)
	return &result, err
}
func (c *Client) PublishGovernancePermit(ctx context.Context, request PublishGovernancePermitRequest) (*GovernanceChangeReceipt, error) {
	data, err := c.PlatformRequest(ctx, OpGovernancePublishPermit, nil, nil, request)
	if err != nil {
		return nil, err
	}
	var result GovernanceChangeReceipt
	err = json.Unmarshal(data, &result)
	return &result, err
}
func (c *Client) InterveneGovernance(ctx context.Context, request GovernanceInterventionRequest) (*GovernanceChangeReceipt, error) {
	data, err := c.PlatformRequest(ctx, OpGovernanceIntervene, nil, nil, request)
	if err != nil {
		return nil, err
	}
	var result GovernanceChangeReceipt
	err = json.Unmarshal(data, &result)
	return &result, err
}

// ProviderExecutionHistory contains retained evidence, never execution authority.
type ProviderExecutionHistory struct {
	Subject            PrincipalIdentity          `json:"subject"`
	Receipt            ProviderHistoryReceipt     `json:"receipt"`
	ObservedAuthority  ProviderHistoryAuthority   `json:"observed_authority"`
	OperationIntegrity string                     `json:"operation_integrity"`
	Metadata           *ProviderOperationMetadata `json:"metadata"`
	Binding            *ProviderHistoryBinding    `json:"binding"`
	CancellationFenced bool                       `json:"cancellation_fenced"`
	Attempts           []ProviderHistoryAttempt   `json:"attempts"`
}
type ProviderHistoryReceipt struct {
	ExecutionID string                `json:"execution_id"`
	Attempts    uint32                `json:"attempts"`
	Status      ProviderHistoryStatus `json:"status"`
}
type ProviderHistoryStatus struct {
	State       string         `json:"state"`
	AttemptID   string         `json:"attempt_id,omitempty"`
	NotBeforeMs *int64         `json:"not_before_ms,omitempty"`
	Outcome     *ActionOutcome `json:"outcome,omitempty"`
}
type ProviderHistoryAuthority struct {
	Incarnation string `json:"incarnation"`
	Generation  uint64 `json:"generation"`
}
type ProviderOperationMetadata struct {
	OriginalActionID string `json:"original_action_id"`
	MaxAttempts      uint32 `json:"max_attempts"`
}
type ProviderHistoryBinding struct {
	Provider         string           `json:"provider"`
	ProviderRevision string           `json:"provider_revision"`
	FailureRevision  string           `json:"failure_revision"`
	Effect           GovernanceEffect `json:"effect"`
}
type ProviderEvidenceReference struct {
	ID     string `json:"id"`
	Digest string `json:"digest"`
}
type ProviderHistoryAttempt struct {
	AttemptID        string                         `json:"attempt_id"`
	Ordinal          uint32                         `json:"ordinal"`
	LedgerStatus     string                         `json:"ledger_status"`
	OriginalEvidence *ProviderEvidenceReference     `json:"original_evidence"`
	OriginalOutcome  *ActionOutcome                 `json:"original_outcome"`
	Reconciliation   *ProviderHistoryReconciliation `json:"reconciliation"`
}
type ProviderReconciliationAcceptance struct {
	Operator     PrincipalIdentity        `json:"operator"`
	Authority    ProviderHistoryAuthority `json:"authority"`
	AcceptedAtMs int64                    `json:"accepted_at_ms"`
}
type ProviderHistoryReconciliation struct {
	PriorStatus      string                            `json:"prior_status"`
	ExecutionID      string                            `json:"execution_id"`
	AttemptID        string                            `json:"attempt_id"`
	OriginalEvidence *ProviderEvidenceReference        `json:"original_evidence"`
	Resolution       ProviderEvidenceReference         `json:"resolution"`
	VerifierRevision string                            `json:"verifier_revision"`
	ProofDigest      string                            `json:"proof_digest"`
	ResolvedAtMs     int64                             `json:"resolved_at_ms"`
	Acceptance       *ProviderReconciliationAcceptance `json:"acceptance,omitempty"`
	Outcome          ActionOutcome                     `json:"outcome"`
}

func (c *Client) ProviderExecutionHistory(ctx context.Context, namespace, tenant, executionID string) (*ProviderExecutionHistory, error) {
	data, err := c.PlatformRequest(ctx, OpGovernanceProviderHistory,
		map[string]string{"execution_id": executionID}, url.Values{"namespace": {namespace}, "tenant": {tenant}}, nil)
	if err != nil {
		return nil, err
	}
	var result ProviderExecutionHistory
	err = json.Unmarshal(data, &result)
	return &result, err
}

type ProviderReconciliationContext struct {
	ContextID     string            `json:"context_id"`
	ExecutionID   string            `json:"execution_id"`
	Namespace     string            `json:"namespace"`
	Tenant        string            `json:"tenant"`
	Principal     PrincipalIdentity `json:"principal"`
	RequestDigest string            `json:"request_digest"`
}
type ProviderReconciliationCorrelation struct {
	Context       ProviderReconciliationContext `json:"context"`
	ActionID      string                        `json:"action_id"`
	AttemptID     string                        `json:"attempt_id"`
	Ordinal       uint32                        `json:"ordinal"`
	Token         string                        `json:"token"`
	BindingDigest string                        `json:"binding_digest"`
}
type ProviderReconciliationRequest struct {
	ProofBase64 string `json:"proof_base64"`
}

func (c *Client) ProviderReconciliationCorrelation(ctx context.Context, namespace, tenant, executionID string, ordinal uint32) (*ProviderReconciliationCorrelation, error) {
	data, err := c.PlatformRequest(ctx, OpGovernanceReconciliationCorrelation, map[string]string{"execution_id": executionID, "ordinal": strconv.FormatUint(uint64(ordinal), 10)}, url.Values{"namespace": {namespace}, "tenant": {tenant}}, nil)
	if err != nil {
		return nil, err
	}
	var result ProviderReconciliationCorrelation
	err = json.Unmarshal(data, &result)
	return &result, err
}
func (c *Client) AcceptProviderReconciliation(ctx context.Context, namespace, tenant, executionID string, ordinal uint32, request ProviderReconciliationRequest) (*ProviderHistoryReceipt, error) {
	data, err := c.PlatformRequest(ctx, OpGovernanceAcceptReconciliation, map[string]string{"execution_id": executionID, "ordinal": strconv.FormatUint(uint64(ordinal), 10)}, url.Values{"namespace": {namespace}, "tenant": {tenant}}, request)
	if err != nil {
		return nil, err
	}
	var result ProviderHistoryReceipt
	err = json.Unmarshal(data, &result)
	return &result, err
}
