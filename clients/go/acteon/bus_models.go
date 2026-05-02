package acteon

// DTOs for the Acteon agentic bus surface (Phases 1-6c).
//
// Field types are pointers wherever the server treats the field as
// optional (`#[serde(default, skip_serializing_if = "Option::is_none")]`)
// so callers can distinguish "unset" from "explicitly the zero value".
// Slices and maps use `omitempty` instead — the server treats an
// absent collection identically to an empty one.

// =============================================================================
// Phase 1: Topics + publish
// =============================================================================

type CreateBusTopic struct {
	Name              string            `json:"name"`
	Namespace         string            `json:"namespace"`
	Tenant            string            `json:"tenant"`
	Partitions        *int              `json:"partitions,omitempty"`
	ReplicationFactor *int              `json:"replication_factor,omitempty"`
	RetentionMs       *int64            `json:"retention_ms,omitempty"`
	Description       *string           `json:"description,omitempty"`
	Labels            map[string]string `json:"labels,omitempty"`
}

type BusTopic struct {
	Name              string            `json:"name"`
	Namespace         string            `json:"namespace"`
	Tenant            string            `json:"tenant"`
	KafkaName         string            `json:"kafka_name"`
	Partitions        int               `json:"partitions"`
	ReplicationFactor int               `json:"replication_factor"`
	RetentionMs       *int64            `json:"retention_ms,omitempty"`
	Description       *string           `json:"description,omitempty"`
	Labels            map[string]string `json:"labels,omitempty"`
	SchemaSubject     *string           `json:"schema_subject,omitempty"`
	SchemaVersion     *int              `json:"schema_version,omitempty"`
	CreatedAt         string            `json:"created_at"`
	UpdatedAt         string            `json:"updated_at"`
}

type ListBusTopicsResponse struct {
	Topics []BusTopic `json:"topics"`
	Count  int        `json:"count"`
}

type ListBusTopicsFilter struct {
	Namespace string
	Tenant    string
}

type PublishBusMessage struct {
	Topic     *string                `json:"topic,omitempty"`
	Namespace *string                `json:"namespace,omitempty"`
	Tenant    *string                `json:"tenant,omitempty"`
	Name      *string                `json:"name,omitempty"`
	Key       *string                `json:"key,omitempty"`
	Payload   any                    `json:"payload"`
	Headers   map[string]string      `json:"headers,omitempty"`
}

type PublishReceipt struct {
	Topic      string `json:"topic"`
	Partition  int32  `json:"partition"`
	Offset     int64  `json:"offset"`
	ProducedAt string `json:"produced_at"`
}

// =============================================================================
// Phase 2: Subscriptions + lag
// =============================================================================

type CreateBusSubscription struct {
	ID              string            `json:"id"`
	Topic           string            `json:"topic"`
	Namespace       string            `json:"namespace"`
	Tenant          string            `json:"tenant"`
	StartingOffset  *string           `json:"starting_offset,omitempty"`
	AckMode         *string           `json:"ack_mode,omitempty"`
	DeadLetterTopic *string           `json:"dead_letter_topic,omitempty"`
	AckTimeoutMs    *uint64           `json:"ack_timeout_ms,omitempty"`
	Description     *string           `json:"description,omitempty"`
	Labels          map[string]string `json:"labels,omitempty"`
}

type BusSubscription struct {
	ID              string            `json:"id"`
	Topic           string            `json:"topic"`
	Namespace       string            `json:"namespace"`
	Tenant          string            `json:"tenant"`
	StartingOffset  string            `json:"starting_offset"`
	AckMode         string            `json:"ack_mode"`
	DeadLetterTopic *string           `json:"dead_letter_topic,omitempty"`
	AckTimeoutMs    uint64            `json:"ack_timeout_ms"`
	Description     *string           `json:"description,omitempty"`
	Labels          map[string]string `json:"labels,omitempty"`
	CreatedAt       string            `json:"created_at"`
	UpdatedAt       string            `json:"updated_at"`
}

type ListBusSubscriptionsResponse struct {
	Subscriptions []BusSubscription `json:"subscriptions"`
	Count         int               `json:"count"`
}

type ListBusSubscriptionsFilter struct {
	Namespace string
	Tenant    string
	Topic     string
}

type BusLagPartition struct {
	Partition      int32 `json:"partition"`
	Committed      int64 `json:"committed"`
	HighWaterMark  int64 `json:"high_water_mark"`
	Lag            int64 `json:"lag"`
}

type BusLag struct {
	SubscriptionID string            `json:"subscription_id"`
	Topic          string            `json:"topic"`
	Partitions     []BusLagPartition `json:"partitions"`
	TotalLag       int64             `json:"total_lag"`
}

// =============================================================================
// Phase 3: Schemas
// =============================================================================

type RegisterBusSchema struct {
	Subject   string            `json:"subject"`
	Namespace string            `json:"namespace"`
	Tenant    string            `json:"tenant"`
	Body      any               `json:"body"`
	Labels    map[string]string `json:"labels,omitempty"`
}

type BusSchema struct {
	Subject   string            `json:"subject"`
	Version   int               `json:"version"`
	Namespace string            `json:"namespace"`
	Tenant    string            `json:"tenant"`
	Body      any               `json:"body"`
	Labels    map[string]string `json:"labels,omitempty"`
	CreatedAt string            `json:"created_at"`
}

type ListBusSchemasResponse struct {
	Schemas []BusSchema `json:"schemas"`
	Count   int         `json:"count"`
}

type ListBusSchemasFilter struct {
	Namespace  string
	Tenant     string
	Subject    string
	LatestOnly bool
}

// =============================================================================
// Phase 4: Agents + heartbeat
// =============================================================================

type RegisterBusAgent struct {
	AgentID         string            `json:"agent_id"`
	Namespace       string            `json:"namespace"`
	Tenant          string            `json:"tenant"`
	Capabilities    []string          `json:"capabilities,omitempty"`
	InboxSuffix     *string           `json:"inbox_suffix,omitempty"`
	HeartbeatTtlMs  *uint64           `json:"heartbeat_ttl_ms,omitempty"`
	Description     *string           `json:"description,omitempty"`
	Labels          map[string]string `json:"labels,omitempty"`
}

type BusAgent struct {
	AgentID          string            `json:"agent_id"`
	Namespace        string            `json:"namespace"`
	Tenant           string            `json:"tenant"`
	Capabilities     []string          `json:"capabilities"`
	InboxTopic       string            `json:"inbox_topic"`
	Status           string            `json:"status"`
	LastHeartbeatAt  *string           `json:"last_heartbeat_at,omitempty"`
	HeartbeatTtlMs   uint64            `json:"heartbeat_ttl_ms"`
	Description      *string           `json:"description,omitempty"`
	Labels           map[string]string `json:"labels,omitempty"`
	CreatedAt        string            `json:"created_at"`
	UpdatedAt        string            `json:"updated_at"`
}

type ListBusAgentsResponse struct {
	Agents []BusAgent `json:"agents"`
	Count  int        `json:"count"`
}

type ListBusAgentsFilter struct {
	Namespace string
	Tenant    string
}

// =============================================================================
// Phase 5: Conversations
// =============================================================================

type CreateBusConversation struct {
	ConversationID string            `json:"conversation_id"`
	Namespace      string            `json:"namespace"`
	Tenant         string            `json:"tenant"`
	Participants   []string          `json:"participants,omitempty"`
	TopicSubject   *string           `json:"topic_subject,omitempty"`
	EventsTopic    *string           `json:"events_topic,omitempty"`
	Description    *string           `json:"description,omitempty"`
	Labels         map[string]string `json:"labels,omitempty"`
}

type BusConversation struct {
	ConversationID string            `json:"conversation_id"`
	Namespace      string            `json:"namespace"`
	Tenant         string            `json:"tenant"`
	Participants   []string          `json:"participants"`
	State          string            `json:"state"`
	TopicSubject   *string           `json:"topic_subject,omitempty"`
	EventsTopic    *string           `json:"events_topic,omitempty"`
	Description    *string           `json:"description,omitempty"`
	Labels         map[string]string `json:"labels,omitempty"`
	CreatedAt      string            `json:"created_at"`
	UpdatedAt      string            `json:"updated_at"`
}

type ListBusConversationsResponse struct {
	Conversations []BusConversation `json:"conversations"`
	Count         int               `json:"count"`
}

type ListBusConversationsFilter struct {
	Namespace   string
	Tenant      string
	State       string
	Participant string
}

type AppendBusConversationMessage struct {
	Payload any               `json:"payload"`
	Sender  *string           `json:"sender,omitempty"`
	Headers map[string]string `json:"headers,omitempty"`
}

type BusReplayMessage struct {
	Partition  int32             `json:"partition"`
	Offset     int64             `json:"offset"`
	ProducedAt string            `json:"produced_at"`
	Sender     *string           `json:"sender,omitempty"`
	Payload    any               `json:"payload"`
	Headers    map[string]string `json:"headers,omitempty"`
}

type BusReplayResponse struct {
	ConversationID string             `json:"conversation_id"`
	EventsTopic    string             `json:"events_topic"`
	Messages       []BusReplayMessage `json:"messages"`
	NextCursor     *string            `json:"next_cursor,omitempty"`
	ExitReason     string             `json:"exit_reason"`
}

type ReplayBusConversationParams struct {
	Limit  int
	Cursor string
}

type TransitionBusConversationRequest struct {
	TargetState string `json:"target_state"`
}

// =============================================================================
// Phase 6a: Tool envelopes
// =============================================================================

type PostBusToolCall struct {
	CallID        string            `json:"call_id"`
	Tool          string            `json:"tool"`
	Arguments     any               `json:"arguments"`
	CorrelationID *string           `json:"correlation_id,omitempty"`
	ReplyTo       *string           `json:"reply_to,omitempty"`
	Sender        *string           `json:"sender,omitempty"`
	Metadata      map[string]string `json:"metadata,omitempty"`
	// Phase 6c: opt into pre-publish HITL gating. When true, the
	// server parks the envelope under a BusApproval row and the
	// post returns a PostBusToolCallOutcome with Parked != nil.
	RequireApproval bool    `json:"require_approval,omitempty"`
	ApprovalReason  *string `json:"approval_reason,omitempty"`
	ApprovalTtlMs   *uint64 `json:"approval_ttl_ms,omitempty"`
}

type PostBusToolResult struct {
	CallID        string            `json:"call_id"`
	Status        string            `json:"status"` // "ok" | "error" | "canceled"
	Output        any               `json:"output"`
	ErrorMessage  *string           `json:"error_message,omitempty"`
	CorrelationID *string           `json:"correlation_id,omitempty"`
	Sender        *string           `json:"sender,omitempty"`
	Metadata      map[string]string `json:"metadata,omitempty"`
}

type BusToolEnvelopeReceipt struct {
	EventsTopic    string `json:"events_topic"`
	ConversationID string `json:"conversation_id"`
	CallID         string `json:"call_id"`
	Partition      int32  `json:"partition"`
	Offset         int64  `json:"offset"`
	ProducedAt     string `json:"produced_at"`
	Cursor         string `json:"cursor"`
}

type BusToolResult struct {
	CallID        string            `json:"call_id"`
	Status        string            `json:"status"`
	Output        any               `json:"output"`
	ErrorMessage  *string           `json:"error_message,omitempty"`
	CorrelationID *string           `json:"correlation_id,omitempty"`
	Sender        *string           `json:"sender,omitempty"`
	Metadata      map[string]string `json:"metadata,omitempty"`
	CreatedAt     string            `json:"created_at"`
}

type BusToolResultLookupParams struct {
	ConversationID string
	Cursor         string
	TimeoutMs      uint64
}

type BusToolResultLookup struct {
	CallID         string         `json:"call_id"`
	EventsTopic    string         `json:"events_topic"`
	ConversationID string         `json:"conversation_id"`
	Partition      int32          `json:"partition"`
	Offset         int64          `json:"offset"`
	ProducedAt     string         `json:"produced_at"`
	Result         BusToolResult `json:"result"`
}

// =============================================================================
// Phase 6b: Stream envelopes
// =============================================================================

type PostBusStreamChunk struct {
	StreamID string            `json:"stream_id"`
	ChunkSeq int64             `json:"chunk_seq"`
	Body     any               `json:"body"`
	Sender   *string           `json:"sender,omitempty"`
	Metadata map[string]string `json:"metadata,omitempty"`
}

type PostBusStreamEnd struct {
	StreamID     string            `json:"stream_id"`
	ChunkSeq     int64             `json:"chunk_seq"`
	Status       string            `json:"status"` // "complete" | "aborted" | "error"
	ErrorMessage *string           `json:"error_message,omitempty"`
	Sender       *string           `json:"sender,omitempty"`
	Metadata     map[string]string `json:"metadata,omitempty"`
}

type BusStreamEnvelopeReceipt struct {
	EventsTopic    string `json:"events_topic"`
	ConversationID string `json:"conversation_id"`
	StreamID       string `json:"stream_id"`
	ChunkSeq       int64  `json:"chunk_seq"`
	Partition      int32  `json:"partition"`
	Offset         int64  `json:"offset"`
	ProducedAt     string `json:"produced_at"`
	Cursor         string `json:"cursor"`
}

// =============================================================================
// Phase 6c: HITL approvals
// =============================================================================

type BusApprovalParkedReceipt struct {
	ApprovalID       string `json:"approval_id"`
	Namespace        string `json:"namespace"`
	Tenant           string `json:"tenant"`
	ConversationID   string `json:"conversation_id"`
	CorrelationToken string `json:"correlation_token"`
	Status           string `json:"status"`
	CreatedAt        string `json:"created_at"`
	ExpiresAt        string `json:"expires_at"`
}

type BusApprovalView struct {
	ApprovalID        string  `json:"approval_id"`
	Namespace         string  `json:"namespace"`
	Tenant            string  `json:"tenant"`
	ConversationID    string  `json:"conversation_id"`
	CorrelationToken  string  `json:"correlation_token"`
	EnvelopeKind      string  `json:"envelope_kind"`
	Status            string  `json:"status"`
	Reason            *string `json:"reason,omitempty"`
	CreatedAt         string  `json:"created_at"`
	ExpiresAt         string  `json:"expires_at"`
	DecidedBy         *string `json:"decided_by,omitempty"`
	DecidedAt         *string `json:"decided_at,omitempty"`
	DecisionNote      *string `json:"decision_note,omitempty"`
	ProducedPartition *int32  `json:"produced_partition,omitempty"`
	ProducedOffset    *int64  `json:"produced_offset,omitempty"`
	ProducedAt        *string `json:"produced_at,omitempty"`
	Envelope          any     `json:"envelope,omitempty"`
}

type ListBusApprovalsResponse struct {
	Approvals []BusApprovalView `json:"approvals"`
	Count     int               `json:"count"`
}

type ListBusApprovalsFilter struct {
	Status         string
	ConversationID string
}

type BusApprovalDecision struct {
	DecidedBy    string  `json:"decided_by"`
	DecisionNote *string `json:"decision_note,omitempty"`
}

type BusApprovalDecisionResponse struct {
	Approval BusApprovalView         `json:"approval"`
	Receipt  *BusToolEnvelopeReceipt `json:"receipt,omitempty"`
}

// =============================================================================
// Sum type for PostBusToolCall — produced vs parked
// =============================================================================

// PostBusToolCallOutcome covers both branches of `PostBusToolCall`.
// Exactly one of `Produced` or `Parked` is non-nil. When `Parked`
// is set the call is awaiting a Phase 6c HITL decision. Go has no
// native sum type so this is the idiomatic encoding — callers
// branch on `if outcome.Parked != nil { ... }`.
type PostBusToolCallOutcome struct {
	Produced *BusToolEnvelopeReceipt
	Parked   *BusApprovalParkedReceipt
}

// WasParked returns true iff the server parked the envelope under a
// pending approval row instead of producing to Kafka.
func (o *PostBusToolCallOutcome) WasParked() bool {
	return o != nil && o.Parked != nil
}
