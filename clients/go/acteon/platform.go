package acteon

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/url"
	"strings"
)

type platformDescriptor struct {
	method, path string
	parameters   []string
	text         bool
}

// PlatformRequest calls a registered finite HTTP operation. Path values are
// escaped as opaque segments; query and body use the server's wire field names.
// Responses preserve JSON envelopes. Text is a JSON string, and 204 is JSON null.
// Calls never auto-retry: retain request IDs when retrying control operations.
func (c *Client) PlatformRequest(ctx context.Context, operation PlatformOperation, pathParams map[string]string, query url.Values, body any) (json.RawMessage, error) {
	d, ok := platformOperations[operation]
	if !ok {
		return nil, fmt.Errorf("unknown platform operation: %s", operation)
	}
	if len(pathParams) != len(d.parameters) {
		return nil, fmt.Errorf("expected path parameters: %v", d.parameters)
	}
	path := d.path
	for _, key := range d.parameters {
		value, exists := pathParams[key]
		if !exists || value == "" || value == "." || value == ".." {
			return nil, fmt.Errorf("invalid or missing path parameter: %s", key)
		}
		path = strings.ReplaceAll(path, "{"+key+"}", url.PathEscape(value))
	}
	if d.method == "GET" && body != nil {
		return nil, fmt.Errorf("GET operations do not accept a body")
	}
	if encoded := query.Encode(); encoded != "" {
		path += "?" + encoded
	}
	resp, err := c.doRequestExt(ctx, d.method, path, body, requestOpts{noRedirect: operation == OpGovernanceMutateRegistry || operation == OpGovernanceRegistryProjection})
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, &HTTPError{Status: resp.StatusCode, Message: string(data)}
	}
	if resp.StatusCode == 204 {
		return json.RawMessage("null"), nil
	}
	if d.text {
		return json.Marshal(string(data))
	}
	if !json.Valid(data) {
		return nil, fmt.Errorf("malformed platform JSON response")
	}
	return data, nil
}
