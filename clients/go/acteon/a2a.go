// A2A protocol surface for the Go ActeonClient.
//
// Mirrors the Rust, Python, and Node SDKs method-for-method.
// Wire payloads are typed as map[string]any matching the A2A JSON
// shapes verbatim — the schema evolves and is JSON-native; pinned
// Go structs would force a translation layer for every field
// change. Factory helpers cover the common construction cases.
//
// Every authenticated call sends `A2A-Version: 1.0` so the server's
// version negotiation honours version-pinned callers. The discovery
// endpoint is intentionally unauthenticated per the A2A spec.

package acteon

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
)

// A2AProtocolVersion is the A2A protocol version this client speaks.
// Matches the Rust client's `A2A_PROTOCOL_VERSION` and the server's
// negotiated version.
const A2AProtocolVersion = "1.0"

// A2AVersionHeader is the HTTP header name carrying the negotiated
// A2A protocol version.
const A2AVersionHeader = "A2A-Version"

// a2aHeaders is the header set every authenticated A2A call sends.
var a2aHeaders = map[string]string{A2AVersionHeader: A2AProtocolVersion}

// ----------------------------------------------------------------------
// Factory helpers
// ----------------------------------------------------------------------

// MakePartText builds a text `Part` payload — the lightest A2A part
// shape. The server rejects text larger than 256 KiB
// (`MAX_PART_TEXT_BYTES`) at validation time.
func MakePartText(text string) map[string]any {
	return map[string]any{"text": text}
}

// MakePartURL builds a URL-reference `Part`. Use this for payloads
// that exceed the 256 KiB inline cap.
func MakePartURL(href string) map[string]any {
	return map[string]any{"url": href}
}

// MakePartData builds a structured-data `Part`. The server
// JSON-encodes the value to measure against `MAX_PART_DATA_BYTES`
// (256 KiB). `mediaType` defaults to `application/json` when empty.
func MakePartData(value any, mediaType string) map[string]any {
	if mediaType == "" {
		mediaType = "application/json"
	}
	return map[string]any{"data": value, "mediaType": mediaType}
}

// MakeMessageOptions carries the optional fields for `MakeMessage`.
type MakeMessageOptions struct {
	// TaskID threads the message into an existing Task's history.
	// Leave empty to mint a fresh Task on `A2ASendMessage`.
	TaskID string
	// ContextID is an optional context id carried across related
	// tasks.
	ContextID string
}

// MakeMessage builds a `TaskMessage` payload. `role` must be
// `"user"` or `"agent"` — the server validates.
func MakeMessage(messageID, role string, parts []map[string]any, opts MakeMessageOptions) map[string]any {
	msg := map[string]any{
		"messageId": messageID,
		"role":      role,
		"parts":     parts,
	}
	if opts.TaskID != "" {
		msg["taskId"] = opts.TaskID
	}
	if opts.ContextID != "" {
		msg["contextId"] = opts.ContextID
	}
	return msg
}

// MakePushConfigOptions carries the optional fields for
// `MakePushConfig`.
type MakePushConfigOptions struct {
	// ID is the optional pre-allocated config id (UUIDv7 by
	// convention). Empty string lets the server mint one.
	ID string
	// Token is the optional bearer token sent in
	// `Authorization: Bearer <token>` on every push POST.
	Token string
	// Authentication is optional richer authentication metadata.
	Authentication map[string]any
}

// MakePushConfig builds a `PushNotificationConfig` body. `url` must
// be `http://` or `https://` — the server denies other schemes at
// registration time.
func MakePushConfig(targetURL string, opts MakePushConfigOptions) map[string]any {
	body := map[string]any{"url": targetURL}
	if opts.ID != "" {
		body["id"] = opts.ID
	}
	if opts.Token != "" {
		body["token"] = opts.Token
	}
	if opts.Authentication != nil {
		body["authentication"] = opts.Authentication
	}
	return body
}

// ----------------------------------------------------------------------
// Internal helpers
// ----------------------------------------------------------------------

// a2aSeg percent-encodes a single path segment opaquely. Mirrors
// the Python/Node `_seg` / `a2aSegment` helpers — a tenant id or
// task id with reserved characters must not leak into additional
// path components.
func a2aSeg(s string) string {
	return url.PathEscape(s)
}

// a2aDoJSON wraps `doRequestExt` with body reading + structured
// error decoding. Mirrors `busDoJSON`. On non-2xx surfaces an
// `*APIError` (with the server's structured envelope) or
// `*HTTPError` (raw body). On 2xx unmarshals into `out` when
// non-nil.
func (c *Client) a2aDoJSON(
	ctx context.Context,
	method, path string,
	body, out any,
	opts requestOpts,
) error {
	resp, err := c.doRequestExt(ctx, method, path, body, opts)
	if err != nil {
		return err
	}
	respBody, readErr := io.ReadAll(resp.Body)
	resp.Body.Close()
	if readErr != nil {
		return &ConnectionError{Message: readErr.Error()}
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		var errResp struct {
			Code    string `json:"code"`
			Message string `json:"message"`
			Error   string `json:"error"`
		}
		if jsonErr := json.Unmarshal(respBody, &errResp); jsonErr == nil {
			msg := errResp.Error
			if msg == "" {
				msg = errResp.Message
			}
			if msg == "" {
				msg = fmt.Sprintf("a2a error (status %d)", resp.StatusCode)
			}
			code := errResp.Code
			if code == "" {
				code = "A2A"
			}
			retryable := resp.StatusCode == http.StatusRequestTimeout ||
				resp.StatusCode == http.StatusTooEarly ||
				resp.StatusCode == http.StatusTooManyRequests ||
				resp.StatusCode >= 500
			return &APIError{Code: code, Message: msg, Retryable: retryable}
		}
		return &HTTPError{Status: resp.StatusCode, Message: string(respBody)}
	}
	if out != nil && len(respBody) > 0 {
		if err := json.Unmarshal(respBody, out); err != nil {
			return &ConnectionError{Message: err.Error()}
		}
	}
	return nil
}

// jsonRPCReply is the JSON-RPC 2.0 reply envelope used by the one
// `agent/getAuthenticatedExtendedCard` round-trip.
type jsonRPCReply struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      any             `json:"id"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *struct {
		Code    int    `json:"code"`
		Message string `json:"message"`
	} `json:"error,omitempty"`
}

// ----------------------------------------------------------------------
// Task lifecycle
// ----------------------------------------------------------------------

// A2ASendMessage calls `POST /a2a/{namespace}/{tenant}/v1/message:send`
// to start a new A2A Task or continue an existing one.
//
// Set `message["taskId"]` (use `MakeMessage` with
// `MakeMessageOptions{TaskID: ...}`) to thread the message into an
// existing Task's history.
func (c *Client) A2ASendMessage(
	ctx context.Context,
	namespace, tenant string,
	message map[string]any,
) (map[string]any, error) {
	var out map[string]any
	path := fmt.Sprintf("/a2a/%s/%s/v1/message:send", a2aSeg(namespace), a2aSeg(tenant))
	err := c.a2aDoJSON(ctx, http.MethodPost, path, map[string]any{"message": message}, &out,
		requestOpts{extraHeaders: a2aHeaders})
	return out, err
}

// A2AGetTask calls `GET /a2a/{namespace}/{tenant}/v1/tasks/{id}`.
// Returns an `*APIError` with HTTP 404 when the task does not exist
// for the caller.
func (c *Client) A2AGetTask(
	ctx context.Context,
	namespace, tenant, taskID string,
) (map[string]any, error) {
	var out map[string]any
	path := fmt.Sprintf("/a2a/%s/%s/v1/tasks/%s",
		a2aSeg(namespace), a2aSeg(tenant), a2aSeg(taskID))
	err := c.a2aDoJSON(ctx, http.MethodGet, path, nil, &out,
		requestOpts{extraHeaders: a2aHeaders})
	return out, err
}

// A2ACancelTask calls `POST /a2a/{namespace}/{tenant}/v1/tasks/{id}:cancel`.
// The `:cancel` verb is part of the URL (spec §11) — the server
// splits it off in-handler.
func (c *Client) A2ACancelTask(
	ctx context.Context,
	namespace, tenant, taskID string,
) (map[string]any, error) {
	var out map[string]any
	path := fmt.Sprintf("/a2a/%s/%s/v1/tasks/%s:cancel",
		a2aSeg(namespace), a2aSeg(tenant), a2aSeg(taskID))
	err := c.a2aDoJSON(ctx, http.MethodPost, path, nil, &out,
		requestOpts{extraHeaders: a2aHeaders})
	return out, err
}

// ----------------------------------------------------------------------
// Push-notification configs
// ----------------------------------------------------------------------

// A2ASetPushConfig calls
// `POST .../v1/tasks/{id}/pushNotificationConfigs` to register or
// upsert a push-notification webhook for a Task. Use
// `MakePushConfig` to build `config`.
func (c *Client) A2ASetPushConfig(
	ctx context.Context,
	namespace, tenant, taskID string,
	config map[string]any,
) (map[string]any, error) {
	var out map[string]any
	path := fmt.Sprintf("/a2a/%s/%s/v1/tasks/%s/pushNotificationConfigs",
		a2aSeg(namespace), a2aSeg(tenant), a2aSeg(taskID))
	err := c.a2aDoJSON(ctx, http.MethodPost, path, config, &out,
		requestOpts{extraHeaders: a2aHeaders})
	return out, err
}

// A2AListPushConfigs calls
// `GET .../v1/tasks/{id}/pushNotificationConfigs` to list every
// config registered for the task.
func (c *Client) A2AListPushConfigs(
	ctx context.Context,
	namespace, tenant, taskID string,
) ([]map[string]any, error) {
	var out []map[string]any
	path := fmt.Sprintf("/a2a/%s/%s/v1/tasks/%s/pushNotificationConfigs",
		a2aSeg(namespace), a2aSeg(tenant), a2aSeg(taskID))
	err := c.a2aDoJSON(ctx, http.MethodGet, path, nil, &out,
		requestOpts{extraHeaders: a2aHeaders})
	return out, err
}

// A2AGetPushConfig calls
// `GET …/pushNotificationConfigs/{cfgId}` to read one config.
func (c *Client) A2AGetPushConfig(
	ctx context.Context,
	namespace, tenant, taskID, configID string,
) (map[string]any, error) {
	var out map[string]any
	path := fmt.Sprintf("/a2a/%s/%s/v1/tasks/%s/pushNotificationConfigs/%s",
		a2aSeg(namespace), a2aSeg(tenant), a2aSeg(taskID), a2aSeg(configID))
	err := c.a2aDoJSON(ctx, http.MethodGet, path, nil, &out,
		requestOpts{extraHeaders: a2aHeaders})
	return out, err
}

// A2ADeletePushConfig calls
// `DELETE …/pushNotificationConfigs/{cfgId}`. Returns an `*APIError`
// with HTTP 404 when the config doesn't exist — the server never
// silently no-ops.
func (c *Client) A2ADeletePushConfig(
	ctx context.Context,
	namespace, tenant, taskID, configID string,
) error {
	path := fmt.Sprintf("/a2a/%s/%s/v1/tasks/%s/pushNotificationConfigs/%s",
		a2aSeg(namespace), a2aSeg(tenant), a2aSeg(taskID), a2aSeg(configID))
	return c.a2aDoJSON(ctx, http.MethodDelete, path, nil, nil,
		requestOpts{extraHeaders: a2aHeaders})
}

// ----------------------------------------------------------------------
// Discovery
// ----------------------------------------------------------------------

// A2ADiscoverAgent calls
// `GET /a2a/{namespace}/{tenant}/.well-known/agent.json` — the
// unauthenticated discovery endpoint.
//
// Issued *without* the Authorization header per the A2A spec. Use
// `A2AGetAuthenticatedExtendedCard` for the authenticated variant.
func (c *Client) A2ADiscoverAgent(
	ctx context.Context,
	namespace, tenant string,
) (map[string]any, error) {
	var out map[string]any
	path := fmt.Sprintf("/a2a/%s/%s/.well-known/agent.json",
		a2aSeg(namespace), a2aSeg(tenant))
	err := c.a2aDoJSON(ctx, http.MethodGet, path, nil, &out,
		requestOpts{skipAuth: true})
	return out, err
}

// A2AGetAuthenticatedExtendedCard invokes the JSON-RPC
// `agent/getAuthenticatedExtendedCard` method. The returned card
// has `capabilities.extendedAgentCard = true` so a client can
// confirm the method was reached.
func (c *Client) A2AGetAuthenticatedExtendedCard(
	ctx context.Context,
	namespace, tenant string,
) (map[string]any, error) {
	envelope := map[string]any{
		"jsonrpc": "2.0",
		"id":      1,
		"method":  "agent/getAuthenticatedExtendedCard",
	}
	path := fmt.Sprintf("/a2a/%s/%s", a2aSeg(namespace), a2aSeg(tenant))
	var reply jsonRPCReply
	if err := c.a2aDoJSON(ctx, http.MethodPost, path, envelope, &reply,
		requestOpts{extraHeaders: a2aHeaders}); err != nil {
		return nil, err
	}
	if reply.Error != nil {
		return nil, &APIError{
			Code:      fmt.Sprintf("%d", reply.Error.Code),
			Message:   reply.Error.Message,
			Retryable: false,
		}
	}
	if len(reply.Result) == 0 {
		return nil, &APIError{
			Code:      "JSONRPC",
			Message:   "JSON-RPC reply had neither result nor error",
			Retryable: false,
		}
	}
	var card map[string]any
	if err := json.Unmarshal(reply.Result, &card); err != nil {
		return nil, &ConnectionError{Message: err.Error()}
	}
	return card, nil
}
