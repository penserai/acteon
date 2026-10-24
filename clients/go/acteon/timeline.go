package acteon

import (
	"context"
	"encoding/json"
	"net/url"
)

// IngestTimelineChange persists a CDC envelope. Preserve its changeId and revision on retry.
func (c *Client) IngestTimelineChange(ctx context.Context, namespace, tenant string, change map[string]any) (json.RawMessage, error) {
	return c.PlatformRequest(ctx, OpTimelineIngest, nil, nil, map[string]any{"scope": map[string]string{"namespace": namespace, "tenant": tenant}, "change": change})
}

// QueryTimeline searches the latest nonexpired event versions.
// Set query["filter"] to a predicate in the timeline CEL subset.
func (c *Client) QueryTimeline(ctx context.Context, namespace, tenant string, query map[string]any) (json.RawMessage, error) {
	if query == nil {
		query = map[string]any{}
	}
	return c.PlatformRequest(ctx, OpTimelineQuery, nil, nil, map[string]any{"scope": map[string]string{"namespace": namespace, "tenant": tenant}, "query": query})
}
func (c *Client) GetTimelineEvent(ctx context.Context, namespace, tenant, source, eventID string) (json.RawMessage, error) {
	return c.PlatformRequest(ctx, OpTimelineGet, map[string]string{"source": source, "eventId": eventID}, url.Values{"namespace": {namespace}, "tenant": {tenant}}, nil)
}
