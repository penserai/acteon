"""DTOs for the Acteon agentic bus surface (Phases 1-6c).

Dataclasses with explicit ``to_dict`` / ``from_dict`` so the bus
methods stay close to the existing model style. Optional fields
default to ``None`` and are dropped from the wire form when not
set, matching the server's ``#[serde(default,
skip_serializing_if = "Option::is_none")]`` pattern.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Optional


# ============================================================================
# Phase 1: Topics
# ============================================================================


@dataclass
class CreateBusTopic:
    name: str
    namespace: str
    tenant: str
    partitions: Optional[int] = None
    replication_factor: Optional[int] = None
    retention_ms: Optional[int] = None
    description: Optional[str] = None
    labels: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "name": self.name,
            "namespace": self.namespace,
            "tenant": self.tenant,
        }
        if self.partitions is not None:
            d["partitions"] = self.partitions
        if self.replication_factor is not None:
            d["replication_factor"] = self.replication_factor
        if self.retention_ms is not None:
            d["retention_ms"] = self.retention_ms
        if self.description is not None:
            d["description"] = self.description
        if self.labels:
            d["labels"] = self.labels
        return d


@dataclass
class BusTopic:
    name: str
    namespace: str
    tenant: str
    kafka_name: str
    partitions: int
    replication_factor: int
    retention_ms: Optional[int]
    description: Optional[str]
    labels: dict[str, str]
    schema_subject: Optional[str]
    schema_version: Optional[int]
    created_at: str
    updated_at: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusTopic":
        return cls(
            name=d["name"],
            namespace=d["namespace"],
            tenant=d["tenant"],
            kafka_name=d["kafka_name"],
            partitions=d["partitions"],
            replication_factor=d["replication_factor"],
            retention_ms=d.get("retention_ms"),
            description=d.get("description"),
            labels=d.get("labels", {}) or {},
            schema_subject=d.get("schema_subject"),
            schema_version=d.get("schema_version"),
            created_at=d["created_at"],
            updated_at=d["updated_at"],
        )


@dataclass
class PublishBusMessage:
    topic: Optional[str] = None
    namespace: Optional[str] = None
    tenant: Optional[str] = None
    name: Optional[str] = None
    key: Optional[str] = None
    payload: Any = None
    headers: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"payload": self.payload}
        if self.topic is not None:
            d["topic"] = self.topic
        if self.namespace is not None:
            d["namespace"] = self.namespace
        if self.tenant is not None:
            d["tenant"] = self.tenant
        if self.name is not None:
            d["name"] = self.name
        if self.key is not None:
            d["key"] = self.key
        if self.headers:
            d["headers"] = self.headers
        return d


@dataclass
class PublishReceipt:
    topic: str
    partition: int
    offset: int
    produced_at: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "PublishReceipt":
        return cls(
            topic=d["topic"],
            partition=d["partition"],
            offset=d["offset"],
            produced_at=d["produced_at"],
        )


# ============================================================================
# Phase 2: Subscriptions
# ============================================================================


@dataclass
class CreateBusSubscription:
    id: str
    topic: str
    namespace: str
    tenant: str
    starting_offset: Optional[str] = None
    ack_mode: Optional[str] = None
    dead_letter_topic: Optional[str] = None
    ack_timeout_ms: Optional[int] = None
    description: Optional[str] = None
    labels: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "id": self.id,
            "topic": self.topic,
            "namespace": self.namespace,
            "tenant": self.tenant,
        }
        if self.starting_offset is not None:
            d["starting_offset"] = self.starting_offset
        if self.ack_mode is not None:
            d["ack_mode"] = self.ack_mode
        if self.dead_letter_topic is not None:
            d["dead_letter_topic"] = self.dead_letter_topic
        if self.ack_timeout_ms is not None:
            d["ack_timeout_ms"] = self.ack_timeout_ms
        if self.description is not None:
            d["description"] = self.description
        if self.labels:
            d["labels"] = self.labels
        return d


@dataclass
class BusSubscription:
    id: str
    topic: str
    namespace: str
    tenant: str
    starting_offset: str
    ack_mode: str
    dead_letter_topic: Optional[str]
    ack_timeout_ms: int
    description: Optional[str]
    labels: dict[str, str]
    created_at: str
    updated_at: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusSubscription":
        return cls(
            id=d["id"],
            topic=d["topic"],
            namespace=d["namespace"],
            tenant=d["tenant"],
            starting_offset=d["starting_offset"],
            ack_mode=d["ack_mode"],
            dead_letter_topic=d.get("dead_letter_topic"),
            ack_timeout_ms=d["ack_timeout_ms"],
            description=d.get("description"),
            labels=d.get("labels", {}) or {},
            created_at=d["created_at"],
            updated_at=d["updated_at"],
        )


@dataclass
class BusLagPartition:
    partition: int
    committed: int
    high_water_mark: int
    lag: int

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusLagPartition":
        return cls(
            partition=d["partition"],
            committed=d["committed"],
            high_water_mark=d["high_water_mark"],
            lag=d["lag"],
        )


@dataclass
class BusLag:
    subscription_id: str
    topic: str
    partitions: list[BusLagPartition]
    total_lag: int

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusLag":
        return cls(
            subscription_id=d["subscription_id"],
            topic=d["topic"],
            partitions=[BusLagPartition.from_dict(p) for p in d.get("partitions", [])],
            total_lag=d["total_lag"],
        )


# ============================================================================
# Phase 3: Schemas
# ============================================================================


@dataclass
class RegisterBusSchema:
    subject: str
    namespace: str
    tenant: str
    body: Any
    labels: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "subject": self.subject,
            "namespace": self.namespace,
            "tenant": self.tenant,
            "body": self.body,
        }
        if self.labels:
            d["labels"] = self.labels
        return d


@dataclass
class BusSchema:
    subject: str
    version: int
    namespace: str
    tenant: str
    body: Any
    labels: dict[str, str]
    created_at: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusSchema":
        return cls(
            subject=d["subject"],
            version=d["version"],
            namespace=d["namespace"],
            tenant=d["tenant"],
            body=d["body"],
            labels=d.get("labels", {}) or {},
            created_at=d["created_at"],
        )


# ============================================================================
# Phase 4: Agents
# ============================================================================


@dataclass
class RegisterBusAgent:
    agent_id: str
    namespace: str
    tenant: str
    capabilities: list[str] = field(default_factory=list)
    inbox_suffix: Optional[str] = None
    heartbeat_ttl_ms: Optional[int] = None
    description: Optional[str] = None
    labels: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "agent_id": self.agent_id,
            "namespace": self.namespace,
            "tenant": self.tenant,
        }
        if self.capabilities:
            d["capabilities"] = self.capabilities
        if self.inbox_suffix is not None:
            d["inbox_suffix"] = self.inbox_suffix
        if self.heartbeat_ttl_ms is not None:
            d["heartbeat_ttl_ms"] = self.heartbeat_ttl_ms
        if self.description is not None:
            d["description"] = self.description
        if self.labels:
            d["labels"] = self.labels
        return d


@dataclass
class BusAgent:
    agent_id: str
    namespace: str
    tenant: str
    capabilities: list[str]
    inbox_topic: str
    status: str
    last_heartbeat_at: Optional[str]
    heartbeat_ttl_ms: int
    description: Optional[str]
    labels: dict[str, str]
    created_at: str
    updated_at: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusAgent":
        return cls(
            agent_id=d["agent_id"],
            namespace=d["namespace"],
            tenant=d["tenant"],
            capabilities=list(d.get("capabilities", []) or []),
            inbox_topic=d["inbox_topic"],
            status=d["status"],
            last_heartbeat_at=d.get("last_heartbeat_at"),
            heartbeat_ttl_ms=d["heartbeat_ttl_ms"],
            description=d.get("description"),
            labels=d.get("labels", {}) or {},
            created_at=d["created_at"],
            updated_at=d["updated_at"],
        )


# ============================================================================
# Phase 5: Conversations
# ============================================================================


@dataclass
class CreateBusConversation:
    conversation_id: str
    namespace: str
    tenant: str
    participants: list[str] = field(default_factory=list)
    topic_subject: Optional[str] = None
    events_topic: Optional[str] = None
    description: Optional[str] = None
    labels: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "conversation_id": self.conversation_id,
            "namespace": self.namespace,
            "tenant": self.tenant,
        }
        if self.participants:
            d["participants"] = self.participants
        if self.topic_subject is not None:
            d["topic_subject"] = self.topic_subject
        if self.events_topic is not None:
            d["events_topic"] = self.events_topic
        if self.description is not None:
            d["description"] = self.description
        if self.labels:
            d["labels"] = self.labels
        return d


@dataclass
class BusConversation:
    conversation_id: str
    namespace: str
    tenant: str
    participants: list[str]
    state: str
    topic_subject: Optional[str]
    events_topic: Optional[str]
    description: Optional[str]
    labels: dict[str, str]
    created_at: str
    updated_at: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusConversation":
        return cls(
            conversation_id=d["conversation_id"],
            namespace=d["namespace"],
            tenant=d["tenant"],
            participants=list(d.get("participants", []) or []),
            state=d["state"],
            topic_subject=d.get("topic_subject"),
            events_topic=d.get("events_topic"),
            description=d.get("description"),
            labels=d.get("labels", {}) or {},
            created_at=d["created_at"],
            updated_at=d["updated_at"],
        )


@dataclass
class AppendBusConversationMessage:
    payload: Any
    sender: Optional[str] = None
    headers: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"payload": self.payload}
        if self.sender is not None:
            d["sender"] = self.sender
        if self.headers:
            d["headers"] = self.headers
        return d


@dataclass
class BusReplayMessage:
    partition: int
    offset: int
    produced_at: str
    sender: Optional[str]
    payload: Any
    headers: dict[str, str]

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusReplayMessage":
        return cls(
            partition=d["partition"],
            offset=d["offset"],
            produced_at=d["produced_at"],
            sender=d.get("sender"),
            payload=d["payload"],
            headers=d.get("headers", {}) or {},
        )


@dataclass
class BusReplayResponse:
    conversation_id: str
    events_topic: str
    messages: list[BusReplayMessage]
    next_cursor: Optional[str]
    exit_reason: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusReplayResponse":
        return cls(
            conversation_id=d["conversation_id"],
            events_topic=d["events_topic"],
            messages=[BusReplayMessage.from_dict(m) for m in d.get("messages", [])],
            next_cursor=d.get("next_cursor"),
            exit_reason=d["exit_reason"],
        )


# ============================================================================
# Phase 6a: Tool-call envelopes
# ============================================================================


@dataclass
class PostBusToolCall:
    call_id: str
    tool: str
    arguments: Any = None
    correlation_id: Optional[str] = None
    reply_to: Optional[str] = None
    sender: Optional[str] = None
    metadata: dict[str, str] = field(default_factory=dict)
    # Phase 6c: opt into pre-publish HITL gating.
    require_approval: bool = False
    approval_reason: Optional[str] = None
    approval_ttl_ms: Optional[int] = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "call_id": self.call_id,
            "tool": self.tool,
            "arguments": self.arguments if self.arguments is not None else {},
        }
        if self.correlation_id is not None:
            d["correlation_id"] = self.correlation_id
        if self.reply_to is not None:
            d["reply_to"] = self.reply_to
        if self.sender is not None:
            d["sender"] = self.sender
        if self.metadata:
            d["metadata"] = self.metadata
        if self.require_approval:
            d["require_approval"] = True
        if self.approval_reason is not None:
            d["approval_reason"] = self.approval_reason
        if self.approval_ttl_ms is not None:
            d["approval_ttl_ms"] = self.approval_ttl_ms
        return d


@dataclass
class PostBusToolResult:
    call_id: str
    status: str  # "ok" | "error" | "canceled"
    output: Any = None
    error_message: Optional[str] = None
    correlation_id: Optional[str] = None
    sender: Optional[str] = None
    metadata: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "call_id": self.call_id,
            "status": self.status,
            "output": self.output if self.output is not None else {},
        }
        if self.error_message is not None:
            d["error_message"] = self.error_message
        if self.correlation_id is not None:
            d["correlation_id"] = self.correlation_id
        if self.sender is not None:
            d["sender"] = self.sender
        if self.metadata:
            d["metadata"] = self.metadata
        return d


@dataclass
class BusToolEnvelopeReceipt:
    events_topic: str
    conversation_id: str
    call_id: str
    partition: int
    offset: int
    produced_at: str
    cursor: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusToolEnvelopeReceipt":
        return cls(
            events_topic=d["events_topic"],
            conversation_id=d["conversation_id"],
            call_id=d["call_id"],
            partition=d["partition"],
            offset=d["offset"],
            produced_at=d["produced_at"],
            cursor=d["cursor"],
        )


@dataclass
class BusToolResult:
    call_id: str
    status: str
    output: Any
    error_message: Optional[str]
    correlation_id: Optional[str]
    sender: Optional[str]
    metadata: dict[str, str]
    created_at: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusToolResult":
        return cls(
            call_id=d["call_id"],
            status=d["status"],
            output=d.get("output"),
            error_message=d.get("error_message"),
            correlation_id=d.get("correlation_id"),
            sender=d.get("sender"),
            metadata=d.get("metadata", {}) or {},
            created_at=d["created_at"],
        )


@dataclass
class BusToolResultLookup:
    call_id: str
    events_topic: str
    conversation_id: str
    partition: int
    offset: int
    produced_at: str
    result: BusToolResult

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusToolResultLookup":
        return cls(
            call_id=d["call_id"],
            events_topic=d["events_topic"],
            conversation_id=d["conversation_id"],
            partition=d["partition"],
            offset=d["offset"],
            produced_at=d["produced_at"],
            result=BusToolResult.from_dict(d["result"]),
        )


@dataclass
class BusToolResultLookupParams:
    conversation_id: str
    cursor: Optional[str] = None
    timeout_ms: Optional[int] = None

    def to_query(self) -> dict[str, Any]:
        q: dict[str, Any] = {"conversation_id": self.conversation_id}
        if self.cursor is not None:
            q["cursor"] = self.cursor
        if self.timeout_ms is not None:
            q["timeout_ms"] = self.timeout_ms
        return q


# ============================================================================
# Phase 6b: Streaming envelopes
# ============================================================================


@dataclass
class PostBusStreamChunk:
    stream_id: str
    chunk_seq: int
    body: Any = None
    sender: Optional[str] = None
    metadata: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "stream_id": self.stream_id,
            "chunk_seq": self.chunk_seq,
            "body": self.body if self.body is not None else {},
        }
        if self.sender is not None:
            d["sender"] = self.sender
        if self.metadata:
            d["metadata"] = self.metadata
        return d


@dataclass
class PostBusStreamEnd:
    stream_id: str
    chunk_seq: int
    status: str  # "complete" | "aborted" | "error"
    error_message: Optional[str] = None
    sender: Optional[str] = None
    metadata: dict[str, str] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "stream_id": self.stream_id,
            "chunk_seq": self.chunk_seq,
            "status": self.status,
        }
        if self.error_message is not None:
            d["error_message"] = self.error_message
        if self.sender is not None:
            d["sender"] = self.sender
        if self.metadata:
            d["metadata"] = self.metadata
        return d


@dataclass
class BusStreamEnvelopeReceipt:
    events_topic: str
    conversation_id: str
    stream_id: str
    chunk_seq: int
    partition: int
    offset: int
    produced_at: str
    cursor: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusStreamEnvelopeReceipt":
        return cls(
            events_topic=d["events_topic"],
            conversation_id=d["conversation_id"],
            stream_id=d["stream_id"],
            chunk_seq=d["chunk_seq"],
            partition=d["partition"],
            offset=d["offset"],
            produced_at=d["produced_at"],
            cursor=d["cursor"],
        )


# ============================================================================
# Phase 6c: HITL approvals
# ============================================================================


@dataclass
class BusApprovalParkedReceipt:
    approval_id: str
    namespace: str
    tenant: str
    conversation_id: str
    correlation_token: str
    status: str
    created_at: str
    expires_at: str

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusApprovalParkedReceipt":
        return cls(
            approval_id=d["approval_id"],
            namespace=d["namespace"],
            tenant=d["tenant"],
            conversation_id=d["conversation_id"],
            correlation_token=d["correlation_token"],
            status=d["status"],
            created_at=d["created_at"],
            expires_at=d["expires_at"],
        )


@dataclass
class BusApprovalView:
    approval_id: str
    namespace: str
    tenant: str
    conversation_id: str
    correlation_token: str
    envelope_kind: str
    status: str
    reason: Optional[str]
    created_at: str
    expires_at: str
    decided_by: Optional[str]
    decided_at: Optional[str]
    decision_note: Optional[str]
    produced_partition: Optional[int]
    produced_offset: Optional[int]
    produced_at: Optional[str]
    envelope: Any

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusApprovalView":
        return cls(
            approval_id=d["approval_id"],
            namespace=d["namespace"],
            tenant=d["tenant"],
            conversation_id=d["conversation_id"],
            correlation_token=d["correlation_token"],
            envelope_kind=d["envelope_kind"],
            status=d["status"],
            reason=d.get("reason"),
            created_at=d["created_at"],
            expires_at=d["expires_at"],
            decided_by=d.get("decided_by"),
            decided_at=d.get("decided_at"),
            decision_note=d.get("decision_note"),
            produced_partition=d.get("produced_partition"),
            produced_offset=d.get("produced_offset"),
            produced_at=d.get("produced_at"),
            envelope=d.get("envelope"),
        )


@dataclass
class BusApprovalDecision:
    decided_by: str
    decision_note: Optional[str] = None

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"decided_by": self.decided_by}
        if self.decision_note is not None:
            d["decision_note"] = self.decision_note
        return d


@dataclass
class BusApprovalDecisionResponse:
    approval: BusApprovalView
    receipt: Optional[BusToolEnvelopeReceipt]

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "BusApprovalDecisionResponse":
        receipt = d.get("receipt")
        return cls(
            approval=BusApprovalView.from_dict(d["approval"]),
            receipt=BusToolEnvelopeReceipt.from_dict(receipt) if receipt else None,
        )


# ============================================================================
# Sum type for `post_bus_tool_call` — produced vs parked
# ============================================================================


@dataclass
class PostBusToolCallOutcome:
    """Either a Kafka receipt (immediate produce) or a parked-approval
    receipt (Phase 6c HITL gate). Inspect ``produced`` / ``parked`` to
    branch.
    """

    produced: Optional[BusToolEnvelopeReceipt] = None
    parked: Optional[BusApprovalParkedReceipt] = None

    @property
    def was_parked(self) -> bool:
        return self.parked is not None


# ============================================================================
# SSE consumer DTOs — bus subscription tail + stream-id tail
# ============================================================================


@dataclass
class BusConsumedMessage:
    """A single Kafka record observed by a bus subscription consumer.
    Mirrors ``acteon_bus::BusMessage`` on the wire — the typed shape
    saves callers from peeling apart raw JSON.
    """

    topic: str
    payload: Any = None
    key: Optional[str] = None
    headers: dict[str, str] = field(default_factory=dict)
    partition: Optional[int] = None
    offset: Optional[int] = None
    timestamp: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "BusConsumedMessage":
        return cls(
            topic=data["topic"],
            payload=data.get("payload"),
            key=data.get("key"),
            headers=data.get("headers") or {},
            partition=data.get("partition"),
            offset=data.get("offset"),
            timestamp=data.get("timestamp"),
        )


@dataclass
class BusConsumeItem:
    """One item from :meth:`consume_bus_subscription`. Exactly one of
    ``message`` or ``error`` is populated; both ``None`` is a keep-alive.
    Inspect via :attr:`is_message` / :attr:`is_error` / :attr:`is_keep_alive`.
    """

    message: Optional[BusConsumedMessage] = None
    error: Optional[str] = None

    @property
    def is_message(self) -> bool:
        return self.message is not None

    @property
    def is_error(self) -> bool:
        return self.error is not None

    @property
    def is_keep_alive(self) -> bool:
        return self.message is None and self.error is None


@dataclass
class StreamChunkEnvelope:
    """`StreamChunk` envelope as it appears on the SSE feed. Mirrors
    ``acteon_core::StreamChunk``."""

    stream_id: str
    chunk_seq: int
    body: Any = None
    sender: Optional[str] = None
    metadata: dict[str, str] = field(default_factory=dict)
    created_at: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "StreamChunkEnvelope":
        return cls(
            stream_id=data["stream_id"],
            chunk_seq=data["chunk_seq"],
            body=data.get("body"),
            sender=data.get("sender"),
            metadata=data.get("metadata") or {},
            created_at=data.get("created_at"),
        )


@dataclass
class StreamEndEnvelope:
    """`StreamEnd` envelope as it appears on the SSE feed. Mirrors
    ``acteon_core::StreamEnd``."""

    stream_id: str
    chunk_seq: int
    status: str
    error_message: Optional[str] = None
    sender: Optional[str] = None
    metadata: dict[str, str] = field(default_factory=dict)
    created_at: Optional[str] = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "StreamEndEnvelope":
        return cls(
            stream_id=data["stream_id"],
            chunk_seq=data["chunk_seq"],
            status=data["status"],
            error_message=data.get("error_message"),
            sender=data.get("sender"),
            metadata=data.get("metadata") or {},
            created_at=data.get("created_at"),
        )


@dataclass
class BusStreamItem:
    """One item from :meth:`consume_bus_stream`. Exactly one of
    ``chunk`` / ``end`` / ``error`` is populated; all ``None`` is a
    keep-alive. The consumer closes once an ``end`` lands."""

    chunk: Optional[StreamChunkEnvelope] = None
    end: Optional[StreamEndEnvelope] = None
    error: Optional[str] = None

    @property
    def is_chunk(self) -> bool:
        return self.chunk is not None

    @property
    def is_end(self) -> bool:
        return self.end is not None

    @property
    def is_error(self) -> bool:
        return self.error is not None

    @property
    def is_keep_alive(self) -> bool:
        return self.chunk is None and self.end is None and self.error is None
