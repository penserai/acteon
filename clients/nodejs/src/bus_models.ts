/**
 * DTOs for the Acteon agentic bus surface (Phases 1-6c).
 *
 * Plain TypeScript interfaces with explicit string-literal unions
 * where the server uses tagged enums. Helper builders normalize
 * optional fields so the wire form drops `undefined` keys (matching
 * the server's `skip_serializing_if = "Option::is_none"`).
 */

// =============================================================================
// Phase 1: Topics + publish
// =============================================================================

export interface CreateBusTopic {
  name: string;
  namespace: string;
  tenant: string;
  partitions?: number;
  replicationFactor?: number;
  retentionMs?: number;
  description?: string;
  labels?: Record<string, string>;
}

export interface BusTopic {
  name: string;
  namespace: string;
  tenant: string;
  kafkaName: string;
  partitions: number;
  replicationFactor: number;
  retentionMs: number | null;
  description: string | null;
  labels: Record<string, string>;
  schemaSubject: string | null;
  schemaVersion: number | null;
  createdAt: string;
  updatedAt: string;
}

export interface PublishBusMessage {
  topic?: string;
  namespace?: string;
  tenant?: string;
  name?: string;
  key?: string;
  payload: unknown;
  headers?: Record<string, string>;
}

export interface PublishReceipt {
  topic: string;
  partition: number;
  offset: number;
  producedAt: string;
}

// =============================================================================
// Phase 2: Subscriptions + lag
// =============================================================================

export interface CreateBusSubscription {
  id: string;
  topic: string;
  namespace: string;
  tenant: string;
  startingOffset?: string;
  ackMode?: string;
  deadLetterTopic?: string;
  ackTimeoutMs?: number;
  description?: string;
  labels?: Record<string, string>;
}

export interface BusSubscription {
  id: string;
  topic: string;
  namespace: string;
  tenant: string;
  startingOffset: string;
  ackMode: string;
  deadLetterTopic: string | null;
  ackTimeoutMs: number;
  description: string | null;
  labels: Record<string, string>;
  createdAt: string;
  updatedAt: string;
}

export interface BusLagPartition {
  partition: number;
  committed: number;
  highWaterMark: number;
  lag: number;
}

export interface BusLag {
  subscriptionId: string;
  topic: string;
  partitions: BusLagPartition[];
  totalLag: number;
}

// =============================================================================
// Phase 3: Schemas
// =============================================================================

export interface RegisterBusSchema {
  subject: string;
  namespace: string;
  tenant: string;
  body: unknown;
  labels?: Record<string, string>;
}

export interface BusSchema {
  subject: string;
  version: number;
  namespace: string;
  tenant: string;
  body: unknown;
  labels: Record<string, string>;
  createdAt: string;
}

// =============================================================================
// Phase 4: Agents + heartbeat
// =============================================================================

export interface RegisterBusAgent {
  agentId: string;
  namespace: string;
  tenant: string;
  capabilities?: string[];
  inboxSuffix?: string;
  heartbeatTtlMs?: number;
  description?: string;
  labels?: Record<string, string>;
}

export interface BusAgent {
  agentId: string;
  namespace: string;
  tenant: string;
  capabilities: string[];
  inboxTopic: string;
  status: string;
  lastHeartbeatAt: string | null;
  heartbeatTtlMs: number;
  description: string | null;
  labels: Record<string, string>;
  createdAt: string;
  updatedAt: string;
}

// =============================================================================
// Phase 5: Conversations
// =============================================================================

export interface CreateBusConversation {
  conversationId: string;
  namespace: string;
  tenant: string;
  participants?: string[];
  topicSubject?: string;
  eventsTopic?: string;
  description?: string;
  labels?: Record<string, string>;
}

export interface BusConversation {
  conversationId: string;
  namespace: string;
  tenant: string;
  participants: string[];
  state: string;
  topicSubject: string | null;
  eventsTopic: string | null;
  description: string | null;
  labels: Record<string, string>;
  createdAt: string;
  updatedAt: string;
}

export interface AppendBusConversationMessage {
  payload: unknown;
  sender?: string;
  headers?: Record<string, string>;
}

export interface BusReplayMessage {
  partition: number;
  offset: number;
  producedAt: string;
  sender: string | null;
  payload: unknown;
  headers: Record<string, string>;
}

export interface BusReplayResponse {
  conversationId: string;
  eventsTopic: string;
  messages: BusReplayMessage[];
  nextCursor: string | null;
  exitReason: string;
}

// =============================================================================
// Phase 6a: Tool envelopes
// =============================================================================

export type BusToolResultStatus = "ok" | "error" | "canceled";

export interface PostBusToolCall {
  callId: string;
  tool: string;
  arguments?: unknown;
  correlationId?: string;
  replyTo?: string;
  sender?: string;
  metadata?: Record<string, string>;
  /**
   * Phase 6c: opt into pre-publish HITL gating. When true, the
   * server parks the envelope under a `BusApproval` row and the
   * post returns a `PostBusToolCallOutcome` whose `kind === "parked"`.
   */
  requireApproval?: boolean;
  approvalReason?: string;
  approvalTtlMs?: number;
}

export interface PostBusToolResult {
  callId: string;
  status: BusToolResultStatus;
  output?: unknown;
  errorMessage?: string;
  correlationId?: string;
  sender?: string;
  metadata?: Record<string, string>;
}

export interface BusToolEnvelopeReceipt {
  eventsTopic: string;
  conversationId: string;
  callId: string;
  partition: number;
  offset: number;
  producedAt: string;
  cursor: string;
}

export interface BusToolResult {
  callId: string;
  status: BusToolResultStatus;
  output: unknown;
  errorMessage: string | null;
  correlationId: string | null;
  sender: string | null;
  metadata: Record<string, string>;
  createdAt: string;
}

export interface BusToolResultLookupParams {
  conversationId: string;
  cursor?: string;
  timeoutMs?: number;
  asAgent?: string;
}

export interface BusToolResultLookup {
  callId: string;
  eventsTopic: string;
  conversationId: string;
  partition: number;
  offset: number;
  producedAt: string;
  result: BusToolResult;
}

// =============================================================================
// Phase 6b: Stream envelopes
// =============================================================================

export type BusStreamEndStatus = "complete" | "aborted" | "error";

export interface PostBusStreamChunk {
  streamId: string;
  chunkSeq: number;
  body?: unknown;
  sender?: string;
  metadata?: Record<string, string>;
}

export interface PostBusStreamEnd {
  streamId: string;
  chunkSeq: number;
  status: BusStreamEndStatus;
  errorMessage?: string;
  sender?: string;
  metadata?: Record<string, string>;
}

export interface BusStreamEnvelopeReceipt {
  eventsTopic: string;
  conversationId: string;
  streamId: string;
  chunkSeq: number;
  partition: number;
  offset: number;
  producedAt: string;
  cursor: string;
}

// =============================================================================
// Phase 6c: HITL approvals
// =============================================================================

export type BusApprovalStatus = "pending" | "approved" | "rejected" | "expired";

export interface BusApprovalParkedReceipt {
  approvalId: string;
  namespace: string;
  tenant: string;
  conversationId: string;
  correlationToken: string;
  status: BusApprovalStatus;
  createdAt: string;
  expiresAt: string;
}

export interface BusApprovalView {
  approvalId: string;
  namespace: string;
  tenant: string;
  conversationId: string;
  correlationToken: string;
  envelopeKind: string;
  status: BusApprovalStatus;
  reason: string | null;
  createdAt: string;
  expiresAt: string;
  decidedBy: string | null;
  decidedAt: string | null;
  decisionNote: string | null;
  producedPartition: number | null;
  producedOffset: number | null;
  producedAt: string | null;
  envelope: unknown;
}

export interface BusApprovalDecision {
  decidedBy: string;
  decisionNote?: string;
}

export interface BusApprovalDecisionResponse {
  approval: BusApprovalView;
  receipt: BusToolEnvelopeReceipt | null;
}

// =============================================================================
// Sum type for `postBusToolCall` — produced vs parked
// =============================================================================

/**
 * Discriminated union covering both branches of `postBusToolCall`.
 * When `kind === "produced"` the call landed on Kafka and the
 * receipt carries partition/offset. When `kind === "parked"` the
 * call is awaiting a HITL decision (Phase 6c) and the receipt
 * carries the approval id.
 */
export type PostBusToolCallOutcome =
  | { kind: "produced"; receipt: BusToolEnvelopeReceipt }
  | { kind: "parked"; receipt: BusApprovalParkedReceipt };

// =============================================================================
// Wire-format helpers
// =============================================================================

/**
 * Build the JSON body for `POST /v1/bus/topics`. Drops undefined
 * fields so the wire form matches the server's
 * `skip_serializing_if = "Option::is_none"` shape.
 */
export function createBusTopicBody(req: CreateBusTopic): Record<string, unknown> {
  const body: Record<string, unknown> = {
    name: req.name,
    namespace: req.namespace,
    tenant: req.tenant,
  };
  if (req.partitions !== undefined) body.partitions = req.partitions;
  if (req.replicationFactor !== undefined) body.replication_factor = req.replicationFactor;
  if (req.retentionMs !== undefined) body.retention_ms = req.retentionMs;
  if (req.description !== undefined) body.description = req.description;
  if (req.labels && Object.keys(req.labels).length > 0) body.labels = req.labels;
  return body;
}

export function publishBusMessageBody(req: PublishBusMessage): Record<string, unknown> {
  const body: Record<string, unknown> = { payload: req.payload };
  if (req.topic !== undefined) body.topic = req.topic;
  if (req.namespace !== undefined) body.namespace = req.namespace;
  if (req.tenant !== undefined) body.tenant = req.tenant;
  if (req.name !== undefined) body.name = req.name;
  if (req.key !== undefined) body.key = req.key;
  if (req.headers && Object.keys(req.headers).length > 0) body.headers = req.headers;
  return body;
}

export function createBusSubscriptionBody(req: CreateBusSubscription): Record<string, unknown> {
  const body: Record<string, unknown> = {
    id: req.id,
    topic: req.topic,
    namespace: req.namespace,
    tenant: req.tenant,
  };
  if (req.startingOffset !== undefined) body.starting_offset = req.startingOffset;
  if (req.ackMode !== undefined) body.ack_mode = req.ackMode;
  if (req.deadLetterTopic !== undefined) body.dead_letter_topic = req.deadLetterTopic;
  if (req.ackTimeoutMs !== undefined) body.ack_timeout_ms = req.ackTimeoutMs;
  if (req.description !== undefined) body.description = req.description;
  if (req.labels && Object.keys(req.labels).length > 0) body.labels = req.labels;
  return body;
}

export function registerBusSchemaBody(req: RegisterBusSchema): Record<string, unknown> {
  const body: Record<string, unknown> = {
    subject: req.subject,
    namespace: req.namespace,
    tenant: req.tenant,
    body: req.body,
  };
  if (req.labels && Object.keys(req.labels).length > 0) body.labels = req.labels;
  return body;
}

export function registerBusAgentBody(req: RegisterBusAgent): Record<string, unknown> {
  const body: Record<string, unknown> = {
    agent_id: req.agentId,
    namespace: req.namespace,
    tenant: req.tenant,
  };
  if (req.capabilities && req.capabilities.length > 0) body.capabilities = req.capabilities;
  if (req.inboxSuffix !== undefined) body.inbox_suffix = req.inboxSuffix;
  if (req.heartbeatTtlMs !== undefined) body.heartbeat_ttl_ms = req.heartbeatTtlMs;
  if (req.description !== undefined) body.description = req.description;
  if (req.labels && Object.keys(req.labels).length > 0) body.labels = req.labels;
  return body;
}

export function createBusConversationBody(req: CreateBusConversation): Record<string, unknown> {
  const body: Record<string, unknown> = {
    conversation_id: req.conversationId,
    namespace: req.namespace,
    tenant: req.tenant,
  };
  if (req.participants && req.participants.length > 0) body.participants = req.participants;
  if (req.topicSubject !== undefined) body.topic_subject = req.topicSubject;
  if (req.eventsTopic !== undefined) body.events_topic = req.eventsTopic;
  if (req.description !== undefined) body.description = req.description;
  if (req.labels && Object.keys(req.labels).length > 0) body.labels = req.labels;
  return body;
}

export function appendBusConversationMessageBody(
  req: AppendBusConversationMessage,
): Record<string, unknown> {
  const body: Record<string, unknown> = { payload: req.payload };
  if (req.sender !== undefined) body.sender = req.sender;
  if (req.headers && Object.keys(req.headers).length > 0) body.headers = req.headers;
  return body;
}

export function postBusToolCallBody(req: PostBusToolCall): Record<string, unknown> {
  const body: Record<string, unknown> = {
    call_id: req.callId,
    tool: req.tool,
    arguments: req.arguments ?? {},
  };
  if (req.correlationId !== undefined) body.correlation_id = req.correlationId;
  if (req.replyTo !== undefined) body.reply_to = req.replyTo;
  if (req.sender !== undefined) body.sender = req.sender;
  if (req.metadata && Object.keys(req.metadata).length > 0) body.metadata = req.metadata;
  if (req.requireApproval) body.require_approval = true;
  if (req.approvalReason !== undefined) body.approval_reason = req.approvalReason;
  if (req.approvalTtlMs !== undefined) body.approval_ttl_ms = req.approvalTtlMs;
  return body;
}

export function postBusToolResultBody(req: PostBusToolResult): Record<string, unknown> {
  const body: Record<string, unknown> = {
    call_id: req.callId,
    status: req.status,
    output: req.output ?? {},
  };
  if (req.errorMessage !== undefined) body.error_message = req.errorMessage;
  if (req.correlationId !== undefined) body.correlation_id = req.correlationId;
  if (req.sender !== undefined) body.sender = req.sender;
  if (req.metadata && Object.keys(req.metadata).length > 0) body.metadata = req.metadata;
  return body;
}

export function postBusStreamChunkBody(req: PostBusStreamChunk): Record<string, unknown> {
  const body: Record<string, unknown> = {
    stream_id: req.streamId,
    chunk_seq: req.chunkSeq,
    body: req.body ?? {},
  };
  if (req.sender !== undefined) body.sender = req.sender;
  if (req.metadata && Object.keys(req.metadata).length > 0) body.metadata = req.metadata;
  return body;
}

export function postBusStreamEndBody(req: PostBusStreamEnd): Record<string, unknown> {
  const body: Record<string, unknown> = {
    stream_id: req.streamId,
    chunk_seq: req.chunkSeq,
    status: req.status,
  };
  if (req.errorMessage !== undefined) body.error_message = req.errorMessage;
  if (req.sender !== undefined) body.sender = req.sender;
  if (req.metadata && Object.keys(req.metadata).length > 0) body.metadata = req.metadata;
  return body;
}

export function busApprovalDecisionBody(d: BusApprovalDecision): Record<string, unknown> {
  const body: Record<string, unknown> = { decided_by: d.decidedBy };
  if (d.decisionNote !== undefined) body.decision_note = d.decisionNote;
  return body;
}

export function busToolResultLookupParams(p: BusToolResultLookupParams): URLSearchParams {
  const params = new URLSearchParams({ conversation_id: p.conversationId });
  if (p.cursor !== undefined) params.set("cursor", p.cursor);
  if (p.timeoutMs !== undefined) params.set("timeout_ms", String(p.timeoutMs));
  if (p.asAgent !== undefined) params.set("as_agent", p.asAgent);
  return params;
}

// =============================================================================
// Wire → camelCase response parsers
// =============================================================================

export function parseBusTopic(d: Record<string, unknown>): BusTopic {
  return {
    name: d.name as string,
    namespace: d.namespace as string,
    tenant: d.tenant as string,
    kafkaName: d.kafka_name as string,
    partitions: d.partitions as number,
    replicationFactor: d.replication_factor as number,
    retentionMs: (d.retention_ms as number | null | undefined) ?? null,
    description: (d.description as string | null | undefined) ?? null,
    labels: (d.labels as Record<string, string> | undefined) ?? {},
    schemaSubject: (d.schema_subject as string | null | undefined) ?? null,
    schemaVersion: (d.schema_version as number | null | undefined) ?? null,
    createdAt: d.created_at as string,
    updatedAt: d.updated_at as string,
  };
}

export function parsePublishReceipt(d: Record<string, unknown>): PublishReceipt {
  return {
    topic: d.topic as string,
    partition: d.partition as number,
    offset: d.offset as number,
    producedAt: d.produced_at as string,
  };
}

export function parseBusSubscription(d: Record<string, unknown>): BusSubscription {
  return {
    id: d.id as string,
    topic: d.topic as string,
    namespace: d.namespace as string,
    tenant: d.tenant as string,
    startingOffset: d.starting_offset as string,
    ackMode: d.ack_mode as string,
    deadLetterTopic: (d.dead_letter_topic as string | null | undefined) ?? null,
    ackTimeoutMs: d.ack_timeout_ms as number,
    description: (d.description as string | null | undefined) ?? null,
    labels: (d.labels as Record<string, string> | undefined) ?? {},
    createdAt: d.created_at as string,
    updatedAt: d.updated_at as string,
  };
}

export function parseBusLag(d: Record<string, unknown>): BusLag {
  const partitionsRaw = (d.partitions ?? []) as Record<string, unknown>[];
  return {
    subscriptionId: d.subscription_id as string,
    topic: d.topic as string,
    partitions: partitionsRaw.map((p) => ({
      partition: p.partition as number,
      committed: p.committed as number,
      highWaterMark: p.high_water_mark as number,
      lag: p.lag as number,
    })),
    totalLag: d.total_lag as number,
  };
}

export function parseBusSchema(d: Record<string, unknown>): BusSchema {
  return {
    subject: d.subject as string,
    version: d.version as number,
    namespace: d.namespace as string,
    tenant: d.tenant as string,
    body: d.body,
    labels: (d.labels as Record<string, string> | undefined) ?? {},
    createdAt: d.created_at as string,
  };
}

export function parseBusAgent(d: Record<string, unknown>): BusAgent {
  return {
    agentId: d.agent_id as string,
    namespace: d.namespace as string,
    tenant: d.tenant as string,
    capabilities: (d.capabilities as string[] | undefined) ?? [],
    inboxTopic: d.inbox_topic as string,
    status: d.status as string,
    lastHeartbeatAt: (d.last_heartbeat_at as string | null | undefined) ?? null,
    heartbeatTtlMs: d.heartbeat_ttl_ms as number,
    description: (d.description as string | null | undefined) ?? null,
    labels: (d.labels as Record<string, string> | undefined) ?? {},
    createdAt: d.created_at as string,
    updatedAt: d.updated_at as string,
  };
}

export function parseBusConversation(d: Record<string, unknown>): BusConversation {
  return {
    conversationId: d.conversation_id as string,
    namespace: d.namespace as string,
    tenant: d.tenant as string,
    participants: (d.participants as string[] | undefined) ?? [],
    state: d.state as string,
    topicSubject: (d.topic_subject as string | null | undefined) ?? null,
    eventsTopic: (d.events_topic as string | null | undefined) ?? null,
    description: (d.description as string | null | undefined) ?? null,
    labels: (d.labels as Record<string, string> | undefined) ?? {},
    createdAt: d.created_at as string,
    updatedAt: d.updated_at as string,
  };
}

export function parseBusReplayResponse(d: Record<string, unknown>): BusReplayResponse {
  const messagesRaw = (d.messages ?? []) as Record<string, unknown>[];
  return {
    conversationId: d.conversation_id as string,
    eventsTopic: d.events_topic as string,
    messages: messagesRaw.map((m) => ({
      partition: m.partition as number,
      offset: m.offset as number,
      producedAt: m.produced_at as string,
      sender: (m.sender as string | null | undefined) ?? null,
      payload: m.payload,
      headers: (m.headers as Record<string, string> | undefined) ?? {},
    })),
    nextCursor: (d.next_cursor as string | null | undefined) ?? null,
    exitReason: d.exit_reason as string,
  };
}

export function parseBusToolEnvelopeReceipt(d: Record<string, unknown>): BusToolEnvelopeReceipt {
  return {
    eventsTopic: d.events_topic as string,
    conversationId: d.conversation_id as string,
    callId: d.call_id as string,
    partition: d.partition as number,
    offset: d.offset as number,
    producedAt: d.produced_at as string,
    cursor: d.cursor as string,
  };
}

export function parseBusToolResult(d: Record<string, unknown>): BusToolResult {
  return {
    callId: d.call_id as string,
    status: d.status as BusToolResultStatus,
    output: d.output,
    errorMessage: (d.error_message as string | null | undefined) ?? null,
    correlationId: (d.correlation_id as string | null | undefined) ?? null,
    sender: (d.sender as string | null | undefined) ?? null,
    metadata: (d.metadata as Record<string, string> | undefined) ?? {},
    createdAt: d.created_at as string,
  };
}

export function parseBusToolResultLookup(d: Record<string, unknown>): BusToolResultLookup {
  return {
    callId: d.call_id as string,
    eventsTopic: d.events_topic as string,
    conversationId: d.conversation_id as string,
    partition: d.partition as number,
    offset: d.offset as number,
    producedAt: d.produced_at as string,
    result: parseBusToolResult(d.result as Record<string, unknown>),
  };
}

export function parseBusStreamEnvelopeReceipt(
  d: Record<string, unknown>,
): BusStreamEnvelopeReceipt {
  return {
    eventsTopic: d.events_topic as string,
    conversationId: d.conversation_id as string,
    streamId: d.stream_id as string,
    chunkSeq: d.chunk_seq as number,
    partition: d.partition as number,
    offset: d.offset as number,
    producedAt: d.produced_at as string,
    cursor: d.cursor as string,
  };
}

export function parseBusApprovalParkedReceipt(
  d: Record<string, unknown>,
): BusApprovalParkedReceipt {
  return {
    approvalId: d.approval_id as string,
    namespace: d.namespace as string,
    tenant: d.tenant as string,
    conversationId: d.conversation_id as string,
    correlationToken: d.correlation_token as string,
    status: d.status as BusApprovalStatus,
    createdAt: d.created_at as string,
    expiresAt: d.expires_at as string,
  };
}

export function parseBusApprovalView(d: Record<string, unknown>): BusApprovalView {
  return {
    approvalId: d.approval_id as string,
    namespace: d.namespace as string,
    tenant: d.tenant as string,
    conversationId: d.conversation_id as string,
    correlationToken: d.correlation_token as string,
    envelopeKind: d.envelope_kind as string,
    status: d.status as BusApprovalStatus,
    reason: (d.reason as string | null | undefined) ?? null,
    createdAt: d.created_at as string,
    expiresAt: d.expires_at as string,
    decidedBy: (d.decided_by as string | null | undefined) ?? null,
    decidedAt: (d.decided_at as string | null | undefined) ?? null,
    decisionNote: (d.decision_note as string | null | undefined) ?? null,
    producedPartition: (d.produced_partition as number | null | undefined) ?? null,
    producedOffset: (d.produced_offset as number | null | undefined) ?? null,
    producedAt: (d.produced_at as string | null | undefined) ?? null,
    envelope: d.envelope,
  };
}

export function parseBusApprovalDecisionResponse(
  d: Record<string, unknown>,
): BusApprovalDecisionResponse {
  const receipt = d.receipt as Record<string, unknown> | null | undefined;
  return {
    approval: parseBusApprovalView(d.approval as Record<string, unknown>),
    receipt: receipt ? parseBusToolEnvelopeReceipt(receipt) : null,
  };
}
