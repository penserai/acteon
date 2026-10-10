package acteon

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"regexp"
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
type FederationGrantCeilingDeclaration struct {
	Source        PrincipalIdentity  `json:"source"`
	Target        PrincipalIdentity  `json:"target"`
	AgentResource GovernanceResource `json:"agent_resource"`
	BindingDigest string             `json:"binding_digest"`
	Skill         string             `json:"skill"`
	IngressEffect GovernanceEffect   `json:"ingress_effect"`
	Effects       []GovernanceEffect `json:"effects"`
	Limits        GovernanceLimits   `json:"limits"`
	MaxDepth      uint64             `json:"max_depth"`
}
type FederationTrustDeclaration struct {
	ID                       string                              `json:"id"`
	Revision                 uint64                              `json:"revision"`
	ForeignDomain            string                              `json:"foreign_domain"`
	LocalAudience            string                              `json:"local_audience"`
	KeyID                    string                              `json:"key_id"`
	VerifyingKey             string                              `json:"verifying_key"`
	MinimumIssuerEpoch       uint64                              `json:"minimum_issuer_epoch"`
	MaxRevocationStalenessMs int64                               `json:"max_revocation_staleness_ms"`
	MaxClockSkewMs           int64                               `json:"max_clock_skew_ms"`
	ValidFromMs              int64                               `json:"valid_from_ms"`
	DeadlineMs               int64                               `json:"deadline_ms"`
	Approved                 []FederationGrantCeilingDeclaration `json:"approved"`
}
type PublishFederationTrustRequest struct {
	Namespace        string                     `json:"namespace"`
	Tenant           string                     `json:"tenant"`
	ChangeID         string                     `json:"change_id"`
	ExpectedRevision uint64                     `json:"expected_revision"`
	Trust            FederationTrustDeclaration `json:"trust"`
	Reason           string                     `json:"reason"`
}
type RevokeFederationTrustRequest struct {
	Namespace        string `json:"namespace"`
	Tenant           string `json:"tenant"`
	ChangeID         string `json:"change_id"`
	TrustID          string `json:"trust_id"`
	ExpectedRevision uint64 `json:"expected_revision"`
	Reason           string `json:"reason"`
}
type SignedFederationEnvelopeDeclaration struct {
	Payload   string `json:"payload"`
	Signature string `json:"signature"`
}

type SignedFederationInvocationDeclaration struct {
	Payload   string `json:"payload"`
	Signature string `json:"signature"`
}
type FederationImportReceipt struct {
	Namespace        string             `json:"namespace"`
	Tenant           string             `json:"tenant"`
	ImportID         string             `json:"import_id"`
	EnvelopeDigest   string             `json:"envelope_digest"`
	TrustID          string             `json:"trust_id"`
	TrustRevision    uint64             `json:"trust_revision"`
	IssuerDomain     string             `json:"issuer_domain"`
	Source           PrincipalIdentity  `json:"source"`
	Target           PrincipalIdentity  `json:"target"`
	AgentResource    GovernanceResource `json:"agent_resource"`
	BindingDigest    string             `json:"binding_digest"`
	Skill            string             `json:"skill"`
	RootID           string             `json:"root_id"`
	ExpiresAtMs      int64              `json:"expires_at_ms"`
	AcceptedAtMs     int64              `json:"accepted_at_ms"`
	ExecutionStarted bool               `json:"execution_started"`
}
type FederationTrustView struct {
	Trust   FederationTrustDeclaration `json:"trust"`
	Revoked bool                       `json:"revoked"`
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
	Subjects            []PrincipalIdentity `json:"subjects"`
	CanIssuePermits     bool                `json:"can_issue_permits"`
	CanIntervene        bool                `json:"can_intervene"`
	CanReadHistory      bool                `json:"can_read_history,omitempty"`
	CanReconcile        bool                `json:"can_reconcile,omitempty"`
	CanManageFederation bool                `json:"can_manage_federation,omitempty"`
	ValidFromMs         int64               `json:"valid_from_ms"`
	Limits              GovernanceLimits    `json:"limits"`
}
type GovernanceScopeView struct {
	Management       GovernanceManagementBounds `json:"management"`
	Namespace        string                     `json:"namespace"`
	Tenant           string                     `json:"tenant"`
	Incarnation      string                     `json:"incarnation"`
	Generation       uint64                     `json:"generation"`
	Routes           []GovernanceRouteView      `json:"routes"`
	Permits          []GovernancePermitView     `json:"permits"`
	ClosedResources  []GovernanceResource       `json:"closed_resources"`
	RevokedSubjects  []string                   `json:"revoked_subjects"`
	FederationTrusts []FederationTrustView      `json:"federation_trusts,omitempty"`
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
func (c *Client) PublishFederationTrust(ctx context.Context, request PublishFederationTrustRequest) (*GovernanceChangeReceipt, error) {
	data, err := c.PlatformRequest(ctx, OpGovernancePublishFederationTrust, nil, nil, request)
	if err != nil {
		return nil, err
	}
	var result GovernanceChangeReceipt
	err = json.Unmarshal(data, &result)
	return &result, err
}
func (c *Client) RevokeFederationTrust(ctx context.Context, request RevokeFederationTrustRequest) (*GovernanceChangeReceipt, error) {
	data, err := c.PlatformRequest(ctx, OpGovernanceRevokeFederationTrust, nil, nil, request)
	if err != nil {
		return nil, err
	}
	var result GovernanceChangeReceipt
	err = json.Unmarshal(data, &result)
	return &result, err
}

// ImportFederationEnvelope authenticates as a configured peer and retains authority without executing it.
func (c *Client) ImportFederationEnvelope(ctx context.Context, namespace, tenant string, envelope SignedFederationEnvelopeDeclaration) (*FederationImportReceipt, error) {
	data, err := c.PlatformRequest(ctx, OpFederationImportEnvelope,
		map[string]string{"namespace": namespace, "tenant": tenant}, nil,
		struct {
			Envelope SignedFederationEnvelopeDeclaration `json:"envelope"`
		}{envelope})
	if err != nil {
		return nil, err
	}
	var result FederationImportReceipt
	if err = json.Unmarshal(data, &result); err != nil {
		return nil, err
	}
	if result.Namespace != namespace || result.Tenant != tenant || result.ExecutionStarted {
		return nil, fmt.Errorf("federation import receipt identity or execution state mismatch")
	}
	return &result, nil
}

// InvokeFederatedAgent executes or replays one exact message under signed imported federation authority.
func (c *Client) InvokeFederatedAgent(ctx context.Context, namespace, tenant, agent string, invocation SignedFederationInvocationDeclaration, message map[string]any) (map[string]any, error) {
	data, err := c.PlatformRequest(ctx, OpFederationInvokeAgent,
		map[string]string{"namespace": namespace, "tenant": tenant, "agent": agent}, nil,
		struct {
			Invocation SignedFederationInvocationDeclaration `json:"invocation"`
			Message    map[string]any                        `json:"message"`
		}{invocation, message})
	if err != nil {
		return nil, err
	}
	var task map[string]any
	if err = json.Unmarshal(data, &task); err != nil {
		return nil, err
	}
	id, ok := task["id"].(string)
	if task["namespace"] != namespace || task["tenant"] != tenant || !ok || id == "" {
		return nil, fmt.Errorf("federated task receipt identity mismatch")
	}
	return task, nil
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

// RegistryProjection selects one independently versioned metadata record.
type RegistryProjection string

const (
	RegistryProjectionAgent RegistryProjection = "agent"
	RegistryProjectionCard  RegistryProjection = "card"
)

type GovernanceRegistryMutationRequest struct {
	Namespace                 string             `json:"namespace"`
	Tenant                    string             `json:"tenant"`
	AgentID                   string             `json:"agent_id"`
	ChangeID                  string             `json:"change_id"`
	ExpectedRegistryRevision  uint64             `json:"expected_registry_revision"`
	Projection                RegistryProjection `json:"projection"`
	ExpectedProjectionVersion *uint64            `json:"expected_projection_version"`
	Value                     json.RawMessage    `json:"value"`
	Reason                    string             `json:"reason"`
}
type GovernanceRegistryProjectionView struct {
	Namespace            string             `json:"namespace"`
	Tenant               string             `json:"tenant"`
	AgentID              string             `json:"agent_id"`
	AgentResource        GovernanceResource `json:"agent_resource"`
	Projection           RegistryProjection `json:"projection"`
	RegistryRevision     uint64             `json:"registry_revision"`
	QualificationRetired *bool              `json:"qualification_retired"`
	Version              *uint64            `json:"version"`
	Value                json.RawMessage    `json:"value"`
}
type GovernanceRegistryMutationReceipt struct {
	Namespace                string             `json:"namespace"`
	Tenant                   string             `json:"tenant"`
	AgentID                  string             `json:"agent_id"`
	ChangeID                 string             `json:"change_id"`
	Projection               RegistryProjection `json:"projection"`
	ExpectedRegistryRevision uint64             `json:"expected_registry_revision"`
	InputDigest              string             `json:"input_digest"`
	Actor                    string             `json:"actor"`
	DeliveryComplete         bool               `json:"delivery_complete"`
	Applied                  bool               `json:"applied"`
}

var registryReceiptDigest = regexp.MustCompile(`^[0-9a-f]{64}$`)

func (c *Client) RegistryProjection(ctx context.Context, namespace, tenant, agentID string, projection RegistryProjection) (*GovernanceRegistryProjectionView, error) {
	data, err := c.PlatformRequest(ctx, OpGovernanceRegistryProjection, map[string]string{"agent_id": agentID}, url.Values{"namespace": {namespace}, "tenant": {tenant}, "projection": {string(projection)}}, nil)
	if err != nil {
		return nil, err
	}
	var result GovernanceRegistryProjectionView
	if err = json.Unmarshal(data, &result); err != nil {
		return nil, err
	}
	var fields map[string]json.RawMessage
	if err = json.Unmarshal(data, &fields); err != nil {
		return nil, err
	}
	for _, key := range []string{"registry_revision", "qualification_retired", "version", "value"} {
		if len(fields[key]) == 0 {
			return nil, fmt.Errorf("incomplete registry observation")
		}
	}
	if string(fields["registry_revision"]) == "null" {
		return nil, fmt.Errorf("invalid registry revision")
	}
	valueIsNull := len(result.Value) == 0 || string(result.Value) == "null"
	var valueObject map[string]json.RawMessage
	valueIsObject := valueIsNull || json.Unmarshal(result.Value, &valueObject) == nil && valueObject != nil
	if result.Namespace != namespace || result.Tenant != tenant || result.AgentID != agentID || result.Projection != projection || (result.RegistryRevision == 0) != (result.QualificationRetired == nil) || result.AgentResource != (GovernanceResource{"agent", namespace, tenant, agentID}) || (result.Version != nil && *result.Version == 0) || (result.Version == nil) != valueIsNull || !valueIsObject {
		return nil, fmt.Errorf("registry observation identity or version mismatch")
	}
	return &result, nil
}

// MutateRegistry sends once. Retain this exact request/change ID for explicit recovery.
func (c *Client) MutateRegistry(ctx context.Context, request GovernanceRegistryMutationRequest) (*GovernanceRegistryMutationReceipt, error) {
	data, err := c.PlatformRequest(ctx, OpGovernanceMutateRegistry, nil, nil, request)
	if err != nil {
		return nil, err
	}
	var result GovernanceRegistryMutationReceipt
	if err = json.Unmarshal(data, &result); err != nil {
		return nil, err
	}
	var fields map[string]json.RawMessage
	if err = json.Unmarshal(data, &fields); err != nil {
		return nil, err
	}
	if len(fields["expected_registry_revision"]) == 0 || string(fields["expected_registry_revision"]) == "null" || result.Namespace != request.Namespace || result.Tenant != request.Tenant || result.AgentID != request.AgentID || result.ChangeID != request.ChangeID || result.Projection != request.Projection || result.ExpectedRegistryRevision != request.ExpectedRegistryRevision || !result.DeliveryComplete || !result.Applied || result.Actor == "" || !registryReceiptDigest.MatchString(result.InputDigest) {
		return nil, fmt.Errorf("unmatched or incomplete registry mutation receipt")
	}
	return &result, nil
}
