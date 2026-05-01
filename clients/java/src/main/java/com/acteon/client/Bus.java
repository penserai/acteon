package com.acteon.client;

import com.fasterxml.jackson.annotation.JsonInclude;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;
import java.util.Map;

/**
 * Container class for the Acteon agentic bus surface (Phases 1-6c).
 *
 * <p>The DTOs live as nested records to keep the bus types in one
 * place rather than sprawling across 40 files. Wire fields use
 * snake_case via {@code @JsonProperty}; Java field names stay
 * camelCase to match the SDK convention. Optional request fields
 * use boxed types ({@code Integer}, {@code Long}) so callers can
 * pass {@code null} to mean "unset" — Jackson + the
 * {@code @JsonInclude(JsonInclude.Include.NON_NULL)} on each record
 * drops nulls from the wire form, matching the server's
 * {@code skip_serializing_if = "Option::is_none"} pattern.
 *
 * <p>Bus methods themselves live as members of {@link ActeonClient}.
 */
public final class Bus {
    private Bus() {}

    // =========================================================================
    // Phase 1: Topics + publish
    // =========================================================================

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record CreateBusTopic(
        String name,
        String namespace,
        String tenant,
        Integer partitions,
        @JsonProperty("replication_factor") Integer replicationFactor,
        @JsonProperty("retention_ms") Long retentionMs,
        String description,
        Map<String, String> labels
    ) {
        public CreateBusTopic(String name, String namespace, String tenant) {
            this(name, namespace, tenant, null, null, null, null, null);
        }
    }

    public record BusTopic(
        String name,
        String namespace,
        String tenant,
        @JsonProperty("kafka_name") String kafkaName,
        int partitions,
        @JsonProperty("replication_factor") int replicationFactor,
        @JsonProperty("retention_ms") Long retentionMs,
        String description,
        Map<String, String> labels,
        @JsonProperty("schema_subject") String schemaSubject,
        @JsonProperty("schema_version") Integer schemaVersion,
        @JsonProperty("created_at") String createdAt,
        @JsonProperty("updated_at") String updatedAt
    ) {}

    public record ListBusTopicsResponse(
        List<BusTopic> topics,
        int count
    ) {}

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record PublishBusMessage(
        String topic,
        String namespace,
        String tenant,
        String name,
        String key,
        Object payload,
        Map<String, String> headers
    ) {}

    public record PublishReceipt(
        String topic,
        int partition,
        long offset,
        @JsonProperty("produced_at") String producedAt
    ) {}

    // =========================================================================
    // Phase 2: Subscriptions + lag
    // =========================================================================

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record CreateBusSubscription(
        String id,
        String topic,
        String namespace,
        String tenant,
        @JsonProperty("starting_offset") String startingOffset,
        @JsonProperty("ack_mode") String ackMode,
        @JsonProperty("dead_letter_topic") String deadLetterTopic,
        @JsonProperty("ack_timeout_ms") Long ackTimeoutMs,
        String description,
        Map<String, String> labels
    ) {
        public CreateBusSubscription(String id, String topic, String namespace, String tenant) {
            this(id, topic, namespace, tenant, null, null, null, null, null, null);
        }
    }

    public record BusSubscription(
        String id,
        String topic,
        String namespace,
        String tenant,
        @JsonProperty("starting_offset") String startingOffset,
        @JsonProperty("ack_mode") String ackMode,
        @JsonProperty("dead_letter_topic") String deadLetterTopic,
        @JsonProperty("ack_timeout_ms") long ackTimeoutMs,
        String description,
        Map<String, String> labels,
        @JsonProperty("created_at") String createdAt,
        @JsonProperty("updated_at") String updatedAt
    ) {}

    public record ListBusSubscriptionsResponse(
        List<BusSubscription> subscriptions,
        int count
    ) {}

    public record BusLagPartition(
        int partition,
        long committed,
        @JsonProperty("high_water_mark") long highWaterMark,
        long lag
    ) {}

    public record BusLag(
        @JsonProperty("subscription_id") String subscriptionId,
        String topic,
        List<BusLagPartition> partitions,
        @JsonProperty("total_lag") long totalLag
    ) {}

    // =========================================================================
    // Phase 3: Schemas
    // =========================================================================

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record RegisterBusSchema(
        String subject,
        String namespace,
        String tenant,
        Object body,
        Map<String, String> labels
    ) {
        public RegisterBusSchema(String subject, String namespace, String tenant, Object body) {
            this(subject, namespace, tenant, body, null);
        }
    }

    public record BusSchema(
        String subject,
        int version,
        String namespace,
        String tenant,
        Object body,
        Map<String, String> labels,
        @JsonProperty("created_at") String createdAt
    ) {}

    public record ListBusSchemasResponse(
        List<BusSchema> schemas,
        int count
    ) {}

    // =========================================================================
    // Phase 4: Agents + heartbeat
    // =========================================================================

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record RegisterBusAgent(
        @JsonProperty("agent_id") String agentId,
        String namespace,
        String tenant,
        List<String> capabilities,
        @JsonProperty("inbox_suffix") String inboxSuffix,
        @JsonProperty("heartbeat_ttl_ms") Long heartbeatTtlMs,
        String description,
        Map<String, String> labels
    ) {
        public RegisterBusAgent(String agentId, String namespace, String tenant) {
            this(agentId, namespace, tenant, null, null, null, null, null);
        }
    }

    public record BusAgent(
        @JsonProperty("agent_id") String agentId,
        String namespace,
        String tenant,
        List<String> capabilities,
        @JsonProperty("inbox_topic") String inboxTopic,
        String status,
        @JsonProperty("last_heartbeat_at") String lastHeartbeatAt,
        @JsonProperty("heartbeat_ttl_ms") long heartbeatTtlMs,
        String description,
        Map<String, String> labels,
        @JsonProperty("created_at") String createdAt,
        @JsonProperty("updated_at") String updatedAt
    ) {}

    public record ListBusAgentsResponse(
        List<BusAgent> agents,
        int count
    ) {}

    // =========================================================================
    // Phase 5: Conversations
    // =========================================================================

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record CreateBusConversation(
        @JsonProperty("conversation_id") String conversationId,
        String namespace,
        String tenant,
        List<String> participants,
        @JsonProperty("topic_subject") String topicSubject,
        @JsonProperty("events_topic") String eventsTopic,
        String description,
        Map<String, String> labels
    ) {
        public CreateBusConversation(String conversationId, String namespace, String tenant) {
            this(conversationId, namespace, tenant, null, null, null, null, null);
        }
    }

    public record BusConversation(
        @JsonProperty("conversation_id") String conversationId,
        String namespace,
        String tenant,
        List<String> participants,
        String state,
        @JsonProperty("topic_subject") String topicSubject,
        @JsonProperty("events_topic") String eventsTopic,
        String description,
        Map<String, String> labels,
        @JsonProperty("created_at") String createdAt,
        @JsonProperty("updated_at") String updatedAt
    ) {}

    public record ListBusConversationsResponse(
        List<BusConversation> conversations,
        int count
    ) {}

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record AppendBusConversationMessage(
        Object payload,
        String sender,
        Map<String, String> headers
    ) {
        public AppendBusConversationMessage(Object payload) {
            this(payload, null, null);
        }
    }

    public record BusReplayMessage(
        int partition,
        long offset,
        @JsonProperty("produced_at") String producedAt,
        String sender,
        Object payload,
        Map<String, String> headers
    ) {}

    public record BusReplayResponse(
        @JsonProperty("conversation_id") String conversationId,
        @JsonProperty("events_topic") String eventsTopic,
        List<BusReplayMessage> messages,
        @JsonProperty("next_cursor") String nextCursor,
        @JsonProperty("exit_reason") String exitReason
    ) {}

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record TransitionBusConversationRequest(
        @JsonProperty("target_state") String targetState
    ) {}

    // =========================================================================
    // Phase 6a: Tool envelopes
    // =========================================================================

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record PostBusToolCall(
        @JsonProperty("call_id") String callId,
        String tool,
        Object arguments,
        @JsonProperty("correlation_id") String correlationId,
        @JsonProperty("reply_to") String replyTo,
        String sender,
        Map<String, String> metadata,
        // Phase 6c: opt into pre-publish HITL gating.
        @JsonProperty("require_approval") @JsonInclude(JsonInclude.Include.NON_DEFAULT) boolean requireApproval,
        @JsonProperty("approval_reason") String approvalReason,
        @JsonProperty("approval_ttl_ms") Long approvalTtlMs
    ) {
        public PostBusToolCall(String callId, String tool, Object arguments) {
            this(callId, tool, arguments, null, null, null, null, false, null, null);
        }
    }

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record PostBusToolResult(
        @JsonProperty("call_id") String callId,
        String status, // "ok" | "error" | "canceled"
        Object output,
        @JsonProperty("error_message") String errorMessage,
        @JsonProperty("correlation_id") String correlationId,
        String sender,
        Map<String, String> metadata
    ) {
        public PostBusToolResult(String callId, String status, Object output) {
            this(callId, status, output, null, null, null, null);
        }
    }

    public record BusToolEnvelopeReceipt(
        @JsonProperty("events_topic") String eventsTopic,
        @JsonProperty("conversation_id") String conversationId,
        @JsonProperty("call_id") String callId,
        int partition,
        long offset,
        @JsonProperty("produced_at") String producedAt,
        String cursor
    ) {}

    public record BusToolResult(
        @JsonProperty("call_id") String callId,
        String status,
        Object output,
        @JsonProperty("error_message") String errorMessage,
        @JsonProperty("correlation_id") String correlationId,
        String sender,
        Map<String, String> metadata,
        @JsonProperty("created_at") String createdAt
    ) {}

    public record BusToolResultLookupParams(
        String conversationId,
        String cursor,
        Long timeoutMs,
        String asAgent
    ) {
        public BusToolResultLookupParams(String conversationId) {
            this(conversationId, null, null, null);
        }
    }

    public record BusToolResultLookup(
        @JsonProperty("call_id") String callId,
        @JsonProperty("events_topic") String eventsTopic,
        @JsonProperty("conversation_id") String conversationId,
        int partition,
        long offset,
        @JsonProperty("produced_at") String producedAt,
        BusToolResult result
    ) {}

    // =========================================================================
    // Phase 6b: Stream envelopes
    // =========================================================================

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record PostBusStreamChunk(
        @JsonProperty("stream_id") String streamId,
        @JsonProperty("chunk_seq") long chunkSeq,
        Object body,
        String sender,
        Map<String, String> metadata
    ) {
        public PostBusStreamChunk(String streamId, long chunkSeq, Object body) {
            this(streamId, chunkSeq, body, null, null);
        }
    }

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record PostBusStreamEnd(
        @JsonProperty("stream_id") String streamId,
        @JsonProperty("chunk_seq") long chunkSeq,
        String status, // "complete" | "aborted" | "error"
        @JsonProperty("error_message") String errorMessage,
        String sender,
        Map<String, String> metadata
    ) {
        public PostBusStreamEnd(String streamId, long chunkSeq, String status) {
            this(streamId, chunkSeq, status, null, null, null);
        }
    }

    public record BusStreamEnvelopeReceipt(
        @JsonProperty("events_topic") String eventsTopic,
        @JsonProperty("conversation_id") String conversationId,
        @JsonProperty("stream_id") String streamId,
        @JsonProperty("chunk_seq") long chunkSeq,
        int partition,
        long offset,
        @JsonProperty("produced_at") String producedAt,
        String cursor
    ) {}

    // =========================================================================
    // Phase 6c: HITL approvals
    // =========================================================================

    public record BusApprovalParkedReceipt(
        @JsonProperty("approval_id") String approvalId,
        String namespace,
        String tenant,
        @JsonProperty("conversation_id") String conversationId,
        @JsonProperty("correlation_token") String correlationToken,
        String status,
        @JsonProperty("created_at") String createdAt,
        @JsonProperty("expires_at") String expiresAt
    ) {}

    public record BusApprovalView(
        @JsonProperty("approval_id") String approvalId,
        String namespace,
        String tenant,
        @JsonProperty("conversation_id") String conversationId,
        @JsonProperty("correlation_token") String correlationToken,
        @JsonProperty("envelope_kind") String envelopeKind,
        String status,
        String reason,
        @JsonProperty("created_at") String createdAt,
        @JsonProperty("expires_at") String expiresAt,
        @JsonProperty("decided_by") String decidedBy,
        @JsonProperty("decided_at") String decidedAt,
        @JsonProperty("decision_note") String decisionNote,
        @JsonProperty("produced_partition") Integer producedPartition,
        @JsonProperty("produced_offset") Long producedOffset,
        @JsonProperty("produced_at") String producedAt,
        Object envelope
    ) {}

    public record ListBusApprovalsResponse(
        List<BusApprovalView> approvals,
        int count
    ) {}

    @JsonInclude(JsonInclude.Include.NON_NULL)
    public record BusApprovalDecision(
        @JsonProperty("decided_by") String decidedBy,
        @JsonProperty("decision_note") String decisionNote
    ) {
        public BusApprovalDecision(String decidedBy) {
            this(decidedBy, null);
        }
    }

    public record BusApprovalDecisionResponse(
        BusApprovalView approval,
        BusToolEnvelopeReceipt receipt
    ) {}

    // =========================================================================
    // Sum type for postBusToolCall — produced vs parked
    // =========================================================================

    /**
     * Sealed-interface discriminated union covering both branches of
     * {@link ActeonClient#postBusToolCall}. Pattern-match with
     * {@code switch} on {@code outcome} to handle the two cases:
     *
     * <pre>{@code
     * var outcome = client.postBusToolCall(ns, tenant, convId, req);
     * switch (outcome) {
     *     case Bus.PostBusToolCallOutcome.Produced p ->
     *         System.out.println("on Kafka at " + p.receipt().offset());
     *     case Bus.PostBusToolCallOutcome.Parked pk ->
     *         System.out.println("awaiting approval " + pk.receipt().approvalId());
     * }
     * }</pre>
     */
    public sealed interface PostBusToolCallOutcome
        permits PostBusToolCallOutcome.Produced, PostBusToolCallOutcome.Parked {

        /** Tool-call landed on Kafka — receipt carries partition/offset. */
        record Produced(BusToolEnvelopeReceipt receipt) implements PostBusToolCallOutcome {}

        /** Tool-call parked under a Phase 6c HITL approval — receipt carries the approval id. */
        record Parked(BusApprovalParkedReceipt receipt) implements PostBusToolCallOutcome {}

        default boolean isParked() { return this instanceof Parked; }
    }
}
