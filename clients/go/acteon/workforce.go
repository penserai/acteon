package acteon

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
)

type TeamRef struct {
	Domain string `json:"domain"`
	Tenant string `json:"tenant"`
	ID     string `json:"id"`
}
type RepresentedParty struct {
	Kind      string             `json:"kind"`
	Principal *PrincipalIdentity `json:"principal,omitempty"`
	Team      *TeamRef           `json:"team,omitempty"`
}

func HumanRepresentation(principal PrincipalIdentity) RepresentedParty {
	return RepresentedParty{Kind: "human", Principal: &principal}
}
func TeamRepresentation(team TeamRef) RepresentedParty {
	return RepresentedParty{Kind: "team", Team: &team}
}

type WorkforceReference struct {
	ID               string `json:"id"`
	AcceptedRevision uint64 `json:"accepted_revision"`
}
type WorkforceDependency struct {
	Kind      string             `json:"kind"`
	Reference WorkforceReference `json:"reference"`
}
type TeamRole string

const (
	TeamRequester        TeamRole = "requester"
	TeamApprover         TeamRole = "approver"
	TeamWorkforceManager TeamRole = "workforce_manager"
	TeamMandateIssuer    TeamRole = "mandate_issuer"
)

type WorkforceTeam struct {
	Team     TeamRef `json:"team"`
	Revision uint64  `json:"revision"`
	Name     string  `json:"name"`
}
type WorkforceMembership struct {
	ID          string            `json:"id"`
	Revision    uint64            `json:"revision"`
	Team        TeamRef           `json:"team"`
	Human       PrincipalIdentity `json:"human"`
	Roles       []TeamRole        `json:"roles"`
	ValidFromMs int64             `json:"valid_from_ms"`
	DeadlineMs  int64             `json:"deadline_ms"`
}
type AgentOwnership struct {
	Agent    PrincipalIdentity `json:"agent"`
	Revision uint64            `json:"revision"`
	Owner    RepresentedParty  `json:"owner"`
}
type WorkforceAssignment struct {
	ID          string            `json:"id"`
	Revision    uint64            `json:"revision"`
	Team        TeamRef           `json:"team"`
	Agent       PrincipalIdentity `json:"agent"`
	JobClasses  []string          `json:"job_classes"`
	ValidFromMs int64             `json:"valid_from_ms"`
	DeadlineMs  int64             `json:"deadline_ms"`
}
type WorkforceMandateDeclaration struct {
	ID                 string                `json:"id"`
	Revision           uint64                `json:"revision"`
	Represented        RepresentedParty      `json:"represented"`
	Actor              PrincipalIdentity     `json:"actor"`
	JobClass           string                `json:"job_class"`
	EligibleInitiators []PrincipalIdentity   `json:"eligible_initiators"`
	Ownership          *WorkforceReference   `json:"ownership"`
	Dependencies       []WorkforceDependency `json:"dependencies"`
	ValidFromMs        int64                 `json:"valid_from_ms"`
	Limits             GovernanceLimits      `json:"limits"`
	Routes             []GovernanceRoute     `json:"routes"`
}
type WorkforceMandateView struct {
	ID                 string                `json:"id"`
	Revision           uint64                `json:"revision"`
	Represented        RepresentedParty      `json:"represented"`
	Actor              PrincipalIdentity     `json:"actor"`
	JobClass           string                `json:"job_class"`
	EligibleInitiators []PrincipalIdentity   `json:"eligible_initiators"`
	Ownership          *WorkforceReference   `json:"ownership"`
	Dependencies       []WorkforceDependency `json:"dependencies"`
	ValidFromMs        int64                 `json:"valid_from_ms"`
	Limits             GovernanceLimits      `json:"limits"`
	Effects            []GovernanceEffect    `json:"effects"`
}

// The closed change interface keeps each operation's fields distinct.
type WorkforceChange interface{ workforceChange() }

func marshalWorkforceChange(kind string, value any) ([]byte, error) {
	data, err := json.Marshal(value)
	if err != nil {
		return nil, err
	}
	var fields map[string]json.RawMessage
	if err = json.Unmarshal(data, &fields); err != nil {
		return nil, err
	}
	fields["kind"], err = json.Marshal(kind)
	if err != nil {
		return nil, err
	}
	return json.Marshal(fields)
}

type PutWorkforceTeam struct {
	Team WorkforceTeam `json:"team"`
}

func (PutWorkforceTeam) workforceChange() {}
func (change PutWorkforceTeam) MarshalJSON() ([]byte, error) {
	type wire PutWorkforceTeam
	return marshalWorkforceChange("put_team", wire(change))
}

type DisbandWorkforceTeam struct {
	Team             TeamRef `json:"team"`
	ExpectedRevision uint64  `json:"expected_revision"`
}

func (DisbandWorkforceTeam) workforceChange() {}
func (change DisbandWorkforceTeam) MarshalJSON() ([]byte, error) {
	type wire DisbandWorkforceTeam
	return marshalWorkforceChange("disband_team", wire(change))
}

type PutWorkforceMembership struct {
	Membership WorkforceMembership `json:"membership"`
}

func (PutWorkforceMembership) workforceChange() {}
func (change PutWorkforceMembership) MarshalJSON() ([]byte, error) {
	type wire PutWorkforceMembership
	return marshalWorkforceChange("put_membership", wire(change))
}

type RemoveWorkforceMembership struct {
	ID               string `json:"id"`
	ExpectedRevision uint64 `json:"expected_revision"`
}

func (RemoveWorkforceMembership) workforceChange() {}
func (change RemoveWorkforceMembership) MarshalJSON() ([]byte, error) {
	type wire RemoveWorkforceMembership
	return marshalWorkforceChange("remove_membership", wire(change))
}

type PutAgentOwnership struct {
	Ownership AgentOwnership `json:"ownership"`
}

func (PutAgentOwnership) workforceChange() {}
func (change PutAgentOwnership) MarshalJSON() ([]byte, error) {
	type wire PutAgentOwnership
	return marshalWorkforceChange("put_ownership", wire(change))
}

type PutWorkforceAssignment struct {
	Assignment WorkforceAssignment `json:"assignment"`
}

func (PutWorkforceAssignment) workforceChange() {}
func (change PutWorkforceAssignment) MarshalJSON() ([]byte, error) {
	type wire PutWorkforceAssignment
	return marshalWorkforceChange("put_assignment", wire(change))
}

type RemoveWorkforceAssignment struct {
	ID               string `json:"id"`
	ExpectedRevision uint64 `json:"expected_revision"`
}

func (RemoveWorkforceAssignment) workforceChange() {}
func (change RemoveWorkforceAssignment) MarshalJSON() ([]byte, error) {
	type wire RemoveWorkforceAssignment
	return marshalWorkforceChange("remove_assignment", wire(change))
}

type PutWorkforceMandate struct {
	Mandate WorkforceMandateDeclaration `json:"mandate"`
}

func (PutWorkforceMandate) workforceChange() {}
func (change PutWorkforceMandate) MarshalJSON() ([]byte, error) {
	type wire PutWorkforceMandate
	return marshalWorkforceChange("put_mandate", wire(change))
}

type RevokeWorkforceMandate struct {
	ID               string `json:"id"`
	ExpectedRevision uint64 `json:"expected_revision"`
}

func (RevokeWorkforceMandate) workforceChange() {}
func (change RevokeWorkforceMandate) MarshalJSON() ([]byte, error) {
	type wire RevokeWorkforceMandate
	return marshalWorkforceChange("revoke_mandate", wire(change))
}

type PublishRepresentedPermit struct {
	Permit  GovernancePermitDeclaration `json:"permit"`
	Mandate WorkforceReference          `json:"mandate"`
}

func (PublishRepresentedPermit) workforceChange() {}
func (change PublishRepresentedPermit) MarshalJSON() ([]byte, error) {
	type wire PublishRepresentedPermit
	return marshalWorkforceChange("publish_represented_permit", wire(change))
}

type WorkforceChangeRequest struct {
	Namespace string          `json:"namespace"`
	Tenant    string          `json:"tenant"`
	ChangeID  string          `json:"change_id"`
	Change    WorkforceChange `json:"change"`
	Reason    string          `json:"reason"`
}

func (request *WorkforceChangeRequest) UnmarshalJSON(data []byte) error {
	var wire struct {
		Namespace string          `json:"namespace"`
		Tenant    string          `json:"tenant"`
		ChangeID  string          `json:"change_id"`
		Change    json.RawMessage `json:"change"`
		Reason    string          `json:"reason"`
	}
	if err := json.Unmarshal(data, &wire); err != nil {
		return err
	}
	var discriminator struct {
		Kind string `json:"kind"`
	}
	if err := json.Unmarshal(wire.Change, &discriminator); err != nil {
		return err
	}
	var change WorkforceChange
	switch discriminator.Kind {
	case "put_team":
		change = &PutWorkforceTeam{}
	case "disband_team":
		change = &DisbandWorkforceTeam{}
	case "put_membership":
		change = &PutWorkforceMembership{}
	case "remove_membership":
		change = &RemoveWorkforceMembership{}
	case "put_ownership":
		change = &PutAgentOwnership{}
	case "put_assignment":
		change = &PutWorkforceAssignment{}
	case "remove_assignment":
		change = &RemoveWorkforceAssignment{}
	case "put_mandate":
		change = &PutWorkforceMandate{}
	case "revoke_mandate":
		change = &RevokeWorkforceMandate{}
	case "publish_represented_permit":
		change = &PublishRepresentedPermit{}
	default:
		return fmt.Errorf("unknown workforce change %q", discriminator.Kind)
	}
	if err := json.Unmarshal(wire.Change, change); err != nil {
		return err
	}
	*request = WorkforceChangeRequest{wire.Namespace, wire.Tenant, wire.ChangeID, change, wire.Reason}
	return nil
}

type WorkforceEntry[T any] struct {
	Value   T    `json:"value"`
	Revoked bool `json:"revoked"`
}
type WorkforcePermitBindingView struct {
	PermitID       string             `json:"permit_id"`
	PermitRevision uint64             `json:"permit_revision"`
	Mandate        WorkforceReference `json:"mandate"`
}
type WorkforceManagementBounds struct {
	Teams            []TeamRef           `json:"teams"`
	Principals       []PrincipalIdentity `json:"principals"`
	JobClasses       []string            `json:"job_classes"`
	CanManageRoster  bool                `json:"can_manage_roster"`
	CanIssueMandates bool                `json:"can_issue_mandates"`
	CanIssuePermits  bool                `json:"can_issue_permits"`
	ValidFromMs      int64               `json:"valid_from_ms"`
	Limits           GovernanceLimits    `json:"limits"`
}
type WorkforceScopeView struct {
	Namespace      string                                 `json:"namespace"`
	Tenant         string                                 `json:"tenant"`
	Incarnation    string                                 `json:"incarnation"`
	Generation     uint64                                 `json:"generation"`
	Management     WorkforceManagementBounds              `json:"management"`
	Routes         []GovernanceRouteView                  `json:"routes"`
	Teams          []WorkforceEntry[WorkforceTeam]        `json:"teams"`
	Memberships    []WorkforceEntry[WorkforceMembership]  `json:"memberships"`
	Ownership      []WorkforceEntry[AgentOwnership]       `json:"ownership"`
	Assignments    []WorkforceEntry[WorkforceAssignment]  `json:"assignments"`
	Mandates       []WorkforceEntry[WorkforceMandateView] `json:"mandates"`
	PermitBindings []WorkforcePermitBindingView           `json:"permit_bindings"`
}

func (client *Client) Workforce(ctx context.Context, namespace, tenant string) (*WorkforceScopeView, error) {
	data, err := client.PlatformRequest(ctx, OpWorkforceInspect, nil, url.Values{"namespace": {namespace}, "tenant": {tenant}}, nil)
	if err != nil {
		return nil, err
	}
	var result WorkforceScopeView
	err = json.Unmarshal(data, &result)
	return &result, err
}
func (client *Client) ChangeWorkforce(ctx context.Context, request WorkforceChangeRequest) (*GovernanceChangeReceipt, error) {
	data, err := client.PlatformRequest(ctx, OpWorkforceChange, nil, nil, request)
	if err != nil {
		return nil, err
	}
	var result GovernanceChangeReceipt
	err = json.Unmarshal(data, &result)
	return &result, err
}
