package acteon

import (
	"context"
	"encoding/json"
)

// PrincipalIdentity is stable actor metadata, not authority.
type PrincipalIdentity struct {
	ID   string `json:"id"`
	Kind string `json:"kind"`
}
type CredentialIdentity struct {
	AuthorityID  *string            `json:"authority_id,omitempty"`
	CredentialID string             `json:"credential_id"`
	AuthMethod   string             `json:"auth_method"`
	Role         string             `json:"role"`
	Principal    *PrincipalIdentity `json:"principal"`
}

// Identity inspects the current credential's stable actor binding.
func (c *Client) Identity(ctx context.Context) (*CredentialIdentity, error) {
	body, err := c.PlatformRequest(ctx, OpAuthIdentity, nil, nil, nil)
	if err != nil {
		return nil, err
	}
	var identity CredentialIdentity
	if err := json.Unmarshal(body, &identity); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return &identity, nil
}
