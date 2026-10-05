package acteon

import (
	"context"
	"encoding/json"
	"net/url"
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
