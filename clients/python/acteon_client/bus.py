"""Agentic bus surface for the Python ActeonClient (Phase 8a).

A ``_BusClientMixin`` that bolts every bus-side method shipped in
Phases 1-6c onto :class:`ActeonClient`. The methods mirror the Rust
client's flat naming (``create_bus_topic``, ``post_bus_tool_call``,
``approve_bus_approval``, ...) so cross-language code reads the
same.

The mixin assumes the host class exposes ``self._request(method,
path, json=, params=)`` returning an ``httpx.Response``. The base
:class:`~acteon_client.client.ActeonClient` already provides this.
"""

from __future__ import annotations

import asyncio
import json
import time
from typing import TYPE_CHECKING, Any, AsyncIterator, Iterator, Optional
from urllib.parse import quote

from .bus_models import (
    AppendBusConversationMessage,
    BusAgent,
    BusApprovalDecision,
    BusApprovalDecisionResponse,
    BusApprovalParkedReceipt,
    BusApprovalView,
    BusConsumeItem,
    BusConsumedMessage,
    BusConversation,
    BusLag,
    BusReplayResponse,
    BusSchema,
    BusStreamEnvelopeReceipt,
    BusStreamItem,
    BusSubscription,
    BusToolEnvelopeReceipt,
    BusToolResultLookup,
    BusToolResultLookupParams,
    BusTopic,
    CreateBusConversation,
    CreateBusSubscription,
    CreateBusTopic,
    PostBusStreamChunk,
    PostBusStreamEnd,
    PostBusToolCall,
    PostBusToolCallOutcome,
    PostBusToolResult,
    PublishBusMessage,
    PublishReceipt,
    ReconnectConfig,
    ReconnectedInfo,
    RegisterBusAgent,
    RegisterBusSchema,
    SetBusAgentAdminState,
    StreamChunkEnvelope,
    StreamEndEnvelope,
)
from .errors import ApiError, HttpError

if TYPE_CHECKING:
    import httpx


def _seg(s: str) -> str:
    """Percent-encode a path segment exactly as the Rust client
    does — the bus REST surface treats the namespace / tenant /
    name path slots as opaque strings, so reserved characters
    like ``/`` need escaping rather than being silently passed
    through.
    """
    return quote(s, safe="")


def _raise_for_status(resp: "httpx.Response") -> None:
    if resp.status_code < 200 or resp.status_code >= 300:
        # Try to surface an Acteon-shaped error body; fall back to a
        # plain HttpError if the body isn't structured.
        try:
            data = resp.json()
            raise ApiError(
                code=data.get("code", "BUS"),
                message=data.get("error") or data.get("message") or "bus error",
                retryable=False,
            )
        except (ValueError, KeyError):
            raise HttpError(resp.status_code, resp.text or "bus error")


class _BusClientMixin:
    """Mixin providing the agentic bus REST surface."""

    # The mixin doesn't define its own __init__; these attributes are
    # set by the concrete ``ActeonClient`` it gets mixed into. Stub
    # the types so ``mypy`` (and humans) understand the contract.
    if TYPE_CHECKING:
        def _request(  # noqa: D401
            self,
            method: str,
            path: str,
            *,
            json: Optional[dict] = None,
            params: Optional[dict] = None,
        ) -> "httpx.Response": ...

        def _headers(self) -> dict[str, str]: ...

        _client: "httpx.Client"
        base_url: str

    # --------------- Phase 1: Topics + publish ---------------

    def create_bus_topic(self, req: CreateBusTopic) -> BusTopic:
        resp = self._request("POST", "/v1/bus/topics", json=req.to_dict())
        _raise_for_status(resp)
        return BusTopic.from_dict(resp.json())

    def list_bus_topics(
        self,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
    ) -> list[BusTopic]:
        params: dict[str, Any] = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        resp = self._request("GET", "/v1/bus/topics", params=params or None)
        _raise_for_status(resp)
        return [BusTopic.from_dict(t) for t in resp.json().get("topics", [])]

    def get_bus_topic(self, namespace: str, tenant: str, name: str) -> BusTopic:
        resp = self._request(
            "GET",
            f"/v1/bus/topics/{_seg(namespace)}/{_seg(tenant)}/{_seg(name)}",
        )
        _raise_for_status(resp)
        return BusTopic.from_dict(resp.json())

    def delete_bus_topic(self, namespace: str, tenant: str, name: str) -> None:
        resp = self._request(
            "DELETE",
            f"/v1/bus/topics/{_seg(namespace)}/{_seg(tenant)}/{_seg(name)}",
        )
        _raise_for_status(resp)

    def publish_bus_message(self, req: PublishBusMessage) -> PublishReceipt:
        resp = self._request("POST", "/v1/bus/publish", json=req.to_dict())
        _raise_for_status(resp)
        return PublishReceipt.from_dict(resp.json())

    # --------------- Phase 2: Subscriptions + lag ---------------

    def create_bus_subscription(self, req: CreateBusSubscription) -> BusSubscription:
        resp = self._request("POST", "/v1/bus/subscriptions", json=req.to_dict())
        _raise_for_status(resp)
        return BusSubscription.from_dict(resp.json())

    def list_bus_subscriptions(
        self,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
        topic: Optional[str] = None,
    ) -> list[BusSubscription]:
        params: dict[str, Any] = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        if topic is not None:
            params["topic"] = topic
        resp = self._request("GET", "/v1/bus/subscriptions", params=params or None)
        _raise_for_status(resp)
        return [BusSubscription.from_dict(s) for s in resp.json().get("subscriptions", [])]

    def get_bus_subscription(self, sub_id: str) -> BusSubscription:
        resp = self._request("GET", f"/v1/bus/subscriptions/{_seg(sub_id)}")
        _raise_for_status(resp)
        return BusSubscription.from_dict(resp.json())

    def delete_bus_subscription(self, sub_id: str) -> None:
        resp = self._request("DELETE", f"/v1/bus/subscriptions/{_seg(sub_id)}")
        _raise_for_status(resp)

    def get_bus_subscription_lag(self, sub_id: str) -> BusLag:
        resp = self._request("GET", f"/v1/bus/subscriptions/{_seg(sub_id)}/lag")
        _raise_for_status(resp)
        return BusLag.from_dict(resp.json())

    # --------------- Phase 3: Schemas ---------------

    def register_bus_schema(self, req: RegisterBusSchema) -> BusSchema:
        resp = self._request("POST", "/v1/bus/schemas", json=req.to_dict())
        _raise_for_status(resp)
        return BusSchema.from_dict(resp.json())

    def list_bus_schemas(
        self,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
        subject: Optional[str] = None,
        latest_only: bool = False,
    ) -> list[BusSchema]:
        params: dict[str, Any] = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        if subject is not None:
            params["subject"] = subject
        if latest_only:
            params["latest_only"] = "true"
        resp = self._request("GET", "/v1/bus/schemas", params=params or None)
        _raise_for_status(resp)
        return [BusSchema.from_dict(s) for s in resp.json().get("schemas", [])]

    def get_bus_schema(
        self, namespace: str, tenant: str, subject: str, version: int,
    ) -> BusSchema:
        resp = self._request(
            "GET",
            f"/v1/bus/schemas/{_seg(namespace)}/{_seg(tenant)}/{_seg(subject)}/{version}",
        )
        _raise_for_status(resp)
        return BusSchema.from_dict(resp.json())

    def delete_bus_schema(
        self, namespace: str, tenant: str, subject: str, version: int,
    ) -> None:
        resp = self._request(
            "DELETE",
            f"/v1/bus/schemas/{_seg(namespace)}/{_seg(tenant)}/{_seg(subject)}/{version}",
        )
        _raise_for_status(resp)

    # --------------- Phase 4: Agents + heartbeat ---------------

    def register_bus_agent(self, req: RegisterBusAgent) -> BusAgent:
        resp = self._request("POST", "/v1/bus/agents", json=req.to_dict())
        _raise_for_status(resp)
        return BusAgent.from_dict(resp.json())

    def list_bus_agents(
        self,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
    ) -> list[BusAgent]:
        params: dict[str, Any] = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        resp = self._request("GET", "/v1/bus/agents", params=params or None)
        _raise_for_status(resp)
        return [BusAgent.from_dict(a) for a in resp.json().get("agents", [])]

    def get_bus_agent(self, namespace: str, tenant: str, agent_id: str) -> BusAgent:
        resp = self._request(
            "GET",
            f"/v1/bus/agents/{_seg(namespace)}/{_seg(tenant)}/{_seg(agent_id)}",
        )
        _raise_for_status(resp)
        return BusAgent.from_dict(resp.json())

    def delete_bus_agent(self, namespace: str, tenant: str, agent_id: str) -> None:
        resp = self._request(
            "DELETE",
            f"/v1/bus/agents/{_seg(namespace)}/{_seg(tenant)}/{_seg(agent_id)}",
        )
        _raise_for_status(resp)

    def heartbeat_bus_agent(
        self, namespace: str, tenant: str, agent_id: str,
    ) -> BusAgent:
        resp = self._request(
            "PATCH",
            f"/v1/bus/agents/{_seg(namespace)}/{_seg(tenant)}/{_seg(agent_id)}/heartbeat",
        )
        _raise_for_status(resp)
        return BusAgent.from_dict(resp.json())

    def set_bus_agent_admin_state(
        self,
        namespace: str,
        tenant: str,
        agent_id: str,
        req: SetBusAgentAdminState,
    ) -> BusAgent:
        """Set the operator admin state on an agent.

        Requires the ``ManageAgent`` permission. The server returns
        400 if ``req.expires_at`` is set on anything other than
        ``"suspended"``.
        """
        resp = self._request(
            "PUT",
            f"/v1/bus/agents/{_seg(namespace)}/{_seg(tenant)}/{_seg(agent_id)}/admin-state",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        return BusAgent.from_dict(resp.json())

    # --------------- Phase 5: Conversations ---------------

    def create_bus_conversation(self, req: CreateBusConversation) -> BusConversation:
        resp = self._request("POST", "/v1/bus/conversations", json=req.to_dict())
        _raise_for_status(resp)
        return BusConversation.from_dict(resp.json())

    def list_bus_conversations(
        self,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
        state: Optional[str] = None,
        participant: Optional[str] = None,
    ) -> list[BusConversation]:
        params: dict[str, Any] = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        if state is not None:
            params["state"] = state
        if participant is not None:
            params["participant"] = participant
        resp = self._request("GET", "/v1/bus/conversations", params=params or None)
        _raise_for_status(resp)
        return [BusConversation.from_dict(c) for c in resp.json().get("conversations", [])]

    def get_bus_conversation(
        self, namespace: str, tenant: str, conversation_id: str,
    ) -> BusConversation:
        resp = self._request(
            "GET",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}",
        )
        _raise_for_status(resp)
        return BusConversation.from_dict(resp.json())

    def delete_bus_conversation(
        self, namespace: str, tenant: str, conversation_id: str,
    ) -> None:
        resp = self._request(
            "DELETE",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}",
        )
        _raise_for_status(resp)

    def transition_bus_conversation(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        *,
        target_state: str,
    ) -> BusConversation:
        resp = self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/transition",
            json={"target_state": target_state},
        )
        _raise_for_status(resp)
        return BusConversation.from_dict(resp.json())

    def append_bus_conversation_message(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        req: AppendBusConversationMessage,
    ) -> dict[str, Any]:
        resp = self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/messages",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        return resp.json()

    def replay_bus_conversation_messages(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        *,
        limit: Optional[int] = None,
        cursor: Optional[str] = None,
    ) -> BusReplayResponse:
        params: dict[str, Any] = {}
        if limit is not None:
            params["limit"] = limit
        if cursor is not None:
            params["cursor"] = cursor
        resp = self._request(
            "GET",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/messages",
            params=params or None,
        )
        _raise_for_status(resp)
        return BusReplayResponse.from_dict(resp.json())

    # --------------- Phase 6a: Tool envelopes ---------------

    def post_bus_tool_call(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        req: PostBusToolCall,
    ) -> PostBusToolCallOutcome:
        """Append a tool-call envelope.

        When ``req.require_approval`` is set (Phase 6c), the server
        parks the envelope and returns 202 with a
        :class:`BusApprovalParkedReceipt`. Otherwise it produces
        immediately and returns a :class:`BusToolEnvelopeReceipt`.
        Inspect the returned outcome's ``was_parked`` to branch.
        """
        resp = self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/tool-calls",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        body = resp.json()
        if resp.status_code == 202:
            return PostBusToolCallOutcome(
                parked=BusApprovalParkedReceipt.from_dict(body),
            )
        return PostBusToolCallOutcome(
            produced=BusToolEnvelopeReceipt.from_dict(body),
        )

    def post_bus_tool_result(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        req: PostBusToolResult,
    ) -> BusToolEnvelopeReceipt:
        resp = self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/tool-results",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        return BusToolEnvelopeReceipt.from_dict(resp.json())

    def lookup_bus_tool_result(
        self,
        namespace: str,
        tenant: str,
        call_id: str,
        params: BusToolResultLookupParams,
    ) -> BusToolResultLookup:
        resp = self._request(
            "GET",
            f"/v1/bus/tool-calls/{_seg(namespace)}/{_seg(tenant)}/{_seg(call_id)}/result",
            params=params.to_query(),
        )
        _raise_for_status(resp)
        return BusToolResultLookup.from_dict(resp.json())

    # --------------- Phase 6b: Stream envelopes ---------------

    def post_bus_stream_chunk(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        req: PostBusStreamChunk,
    ) -> BusStreamEnvelopeReceipt:
        resp = self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/stream-chunks",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        return BusStreamEnvelopeReceipt.from_dict(resp.json())

    def post_bus_stream_end(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        req: PostBusStreamEnd,
    ) -> BusStreamEnvelopeReceipt:
        resp = self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/stream-end",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        return BusStreamEnvelopeReceipt.from_dict(resp.json())

    def bus_stream_consume_url(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        stream_id: str,
    ) -> str:
        """Return the SSE consume URL for a stream. Plug the URL into
        whichever SSE client your runtime prefers (``httpx-sse``,
        ``aiohttp-sse-client2``, ``sseclient``, etc.). Path
        segments are encoded the same way the Rust SDK encodes
        them.
        """
        return (
            f"{self.base_url}/v1/bus/streams/"
            f"{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/{_seg(stream_id)}"
        )

    def consume_bus_subscription(
        self,
        subscription_id: str,
        *,
        topic: str,
        from_offset: Optional[str] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ) -> Iterator[BusConsumeItem]:
        """Consume a bus subscription via SSE
        (``GET /v1/bus/subscribe/{subscription_id}``). Yields one item
        per Kafka record on the underlying topic. Server-side errors
        surface as :attr:`BusConsumeItem.error`; SSE keep-alive
        comments surface as :attr:`BusConsumeItem.is_keep_alive` so
        callers can use them as a liveness signal.

        When ``reconnect`` is set, a clean disconnect triggers
        exponential backoff and a fresh subscribe call from
        ``latest`` — yielding a :attr:`BusConsumeItem.is_reconnected`
        item so callers can resync state. Note that resume from
        ``latest`` means messages produced during the disconnect
        window are dropped; use Phase 2 durable subscriptions with
        manual ack for lossless delivery.

        Args:
            subscription_id: Subscription id (Kafka consumer group).
            topic: Full Kafka topic name (``namespace.tenant.name``).
            from_offset: ``earliest`` or ``latest`` (server default).
            reconnect: Opt-in :class:`ReconnectConfig` for best-effort
                reconnect on disconnect.

        Yields:
            :class:`BusConsumeItem` per record.
        """
        if reconnect is None:
            params: dict[str, Any] = {"topic": topic}
            if from_offset is not None:
                params["from"] = from_offset
            url = f"{self.base_url}/v1/bus/subscribe/{_seg(subscription_id)}"
            for env in _open_bus_sse_stream(self._client, url, params, self._headers()):
                yield _envelope_to_consume_item(env)
            return

        # Reconnect path: open the first stream with the caller's
        # `from_offset`, then resume from `latest` after each
        # disconnect. The attempt counter resets on a successful read.
        first_params: dict[str, Any] = {"topic": topic}
        if from_offset is not None:
            first_params["from"] = from_offset
        resume_params: dict[str, Any] = {"topic": topic, "from": "latest"}
        url = f"{self.base_url}/v1/bus/subscribe/{_seg(subscription_id)}"
        attempt = 0
        params_for_open = first_params
        while True:
            try:
                for env in _open_bus_sse_stream(
                    self._client, url, params_for_open, self._headers()
                ):
                    attempt = 0
                    yield _envelope_to_consume_item(env)
            except (ConnectionError, HttpError):
                # Reconnect path swallows these — they're the
                # disconnect signal we're meant to recover from.
                pass
            if (
                reconnect.max_attempts is not None
                and attempt >= reconnect.max_attempts
            ):
                return
            backoff_ms = _reconnect_backoff_ms(attempt, reconnect)
            time.sleep(backoff_ms / 1000.0)
            attempt += 1
            yield BusConsumeItem(
                reconnected=ReconnectedInfo(backoff_ms=backoff_ms, attempt=attempt)
            )
            params_for_open = resume_params

    def consume_bus_stream(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        stream_id: str,
    ) -> Iterator[BusStreamItem]:
        """Consume a typed stream via SSE
        (``GET /v1/bus/streams/{ns}/{tenant}/{conv}/{stream_id}``). The
        server filters by ``(envelope_kind, conversation_id, stream_id)``,
        so this stream only emits chunks for the requested stream id and
        closes after the terminal :class:`StreamEndEnvelope`.

        Yields:
            :class:`BusStreamItem` per chunk plus the terminal end marker.
        """
        url = self.bus_stream_consume_url(namespace, tenant, conversation_id, stream_id)
        for env in _open_bus_sse_stream(self._client, url, None, self._headers()):
            item = _envelope_to_stream_item(env)
            yield item
            if item.is_end:
                break

    # --------------- Phase 6c: HITL approvals ---------------

    def list_bus_approvals(
        self,
        namespace: str,
        tenant: str,
        *,
        status: Optional[str] = None,
        conversation_id: Optional[str] = None,
    ) -> list[BusApprovalView]:
        params: dict[str, Any] = {}
        if status is not None:
            params["status"] = status
        if conversation_id is not None:
            params["conversation_id"] = conversation_id
        resp = self._request(
            "GET",
            f"/v1/bus/approvals/{_seg(namespace)}/{_seg(tenant)}",
            params=params or None,
        )
        _raise_for_status(resp)
        return [BusApprovalView.from_dict(a) for a in resp.json().get("approvals", [])]

    def get_bus_approval(
        self, namespace: str, tenant: str, approval_id: str,
    ) -> BusApprovalView:
        resp = self._request(
            "GET",
            f"/v1/bus/approvals/{_seg(namespace)}/{_seg(tenant)}/{_seg(approval_id)}",
        )
        _raise_for_status(resp)
        return BusApprovalView.from_dict(resp.json())

    def approve_bus_approval(
        self,
        namespace: str,
        tenant: str,
        approval_id: str,
        decision: BusApprovalDecision,
    ) -> BusApprovalDecisionResponse:
        resp = self._request(
            "POST",
            f"/v1/bus/approvals/{_seg(namespace)}/{_seg(tenant)}/{_seg(approval_id)}/approve",
            json=decision.to_dict(),
        )
        _raise_for_status(resp)
        return BusApprovalDecisionResponse.from_dict(resp.json())

    def reject_bus_approval(
        self,
        namespace: str,
        tenant: str,
        approval_id: str,
        decision: BusApprovalDecision,
    ) -> BusApprovalDecisionResponse:
        resp = self._request(
            "POST",
            f"/v1/bus/approvals/{_seg(namespace)}/{_seg(tenant)}/{_seg(approval_id)}/reject",
            json=decision.to_dict(),
        )
        _raise_for_status(resp)
        return BusApprovalDecisionResponse.from_dict(resp.json())


# ============================================================================
# Async mixin
#
# Mirrors `_BusClientMixin` 1:1 but each method awaits `self._request`.
# Mounted onto `AsyncActeonClient` so callers in asyncio runtimes
# (FastAPI handlers, agent loops, etc.) don't block the event loop on
# bus traffic. The two mixins share zero implementation because the
# blocking and non-blocking call sites are syntactically distinct in
# Python — duck-typing one to "auto-await" would either pollute the
# sync API with awaitables or hide event-loop blocking behind a
# helper layer. Explicit two-mixin code is shorter and clearer.
# ============================================================================


class _AsyncBusClientMixin:
    """Async mixin providing the agentic bus REST surface."""

    if TYPE_CHECKING:
        async def _request(  # noqa: D401
            self,
            method: str,
            path: str,
            *,
            json: Optional[dict] = None,
            params: Optional[dict] = None,
        ) -> "httpx.Response": ...

        def _headers(self) -> dict[str, str]: ...

        _client: "httpx.AsyncClient"
        base_url: str

    # --------------- Phase 1: Topics + publish ---------------

    async def create_bus_topic(self, req: CreateBusTopic) -> BusTopic:
        resp = await self._request("POST", "/v1/bus/topics", json=req.to_dict())
        _raise_for_status(resp)
        return BusTopic.from_dict(resp.json())

    async def list_bus_topics(
        self,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
    ) -> list[BusTopic]:
        params: dict[str, Any] = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        resp = await self._request("GET", "/v1/bus/topics", params=params or None)
        _raise_for_status(resp)
        return [BusTopic.from_dict(t) for t in resp.json().get("topics", [])]

    async def get_bus_topic(self, namespace: str, tenant: str, name: str) -> BusTopic:
        resp = await self._request(
            "GET",
            f"/v1/bus/topics/{_seg(namespace)}/{_seg(tenant)}/{_seg(name)}",
        )
        _raise_for_status(resp)
        return BusTopic.from_dict(resp.json())

    async def delete_bus_topic(self, namespace: str, tenant: str, name: str) -> None:
        resp = await self._request(
            "DELETE",
            f"/v1/bus/topics/{_seg(namespace)}/{_seg(tenant)}/{_seg(name)}",
        )
        _raise_for_status(resp)

    async def publish_bus_message(self, req: PublishBusMessage) -> PublishReceipt:
        resp = await self._request("POST", "/v1/bus/publish", json=req.to_dict())
        _raise_for_status(resp)
        return PublishReceipt.from_dict(resp.json())

    # --------------- Phase 2: Subscriptions + lag ---------------

    async def create_bus_subscription(self, req: CreateBusSubscription) -> BusSubscription:
        resp = await self._request("POST", "/v1/bus/subscriptions", json=req.to_dict())
        _raise_for_status(resp)
        return BusSubscription.from_dict(resp.json())

    async def list_bus_subscriptions(
        self,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
        topic: Optional[str] = None,
    ) -> list[BusSubscription]:
        params: dict[str, Any] = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        if topic is not None:
            params["topic"] = topic
        resp = await self._request("GET", "/v1/bus/subscriptions", params=params or None)
        _raise_for_status(resp)
        return [BusSubscription.from_dict(s) for s in resp.json().get("subscriptions", [])]

    async def get_bus_subscription(self, sub_id: str) -> BusSubscription:
        resp = await self._request("GET", f"/v1/bus/subscriptions/{_seg(sub_id)}")
        _raise_for_status(resp)
        return BusSubscription.from_dict(resp.json())

    async def delete_bus_subscription(self, sub_id: str) -> None:
        resp = await self._request("DELETE", f"/v1/bus/subscriptions/{_seg(sub_id)}")
        _raise_for_status(resp)

    async def get_bus_subscription_lag(self, sub_id: str) -> BusLag:
        resp = await self._request("GET", f"/v1/bus/subscriptions/{_seg(sub_id)}/lag")
        _raise_for_status(resp)
        return BusLag.from_dict(resp.json())

    # --------------- Phase 3: Schemas ---------------

    async def register_bus_schema(self, req: RegisterBusSchema) -> BusSchema:
        resp = await self._request("POST", "/v1/bus/schemas", json=req.to_dict())
        _raise_for_status(resp)
        return BusSchema.from_dict(resp.json())

    async def list_bus_schemas(
        self,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
        subject: Optional[str] = None,
        latest_only: bool = False,
    ) -> list[BusSchema]:
        params: dict[str, Any] = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        if subject is not None:
            params["subject"] = subject
        if latest_only:
            params["latest_only"] = "true"
        resp = await self._request("GET", "/v1/bus/schemas", params=params or None)
        _raise_for_status(resp)
        return [BusSchema.from_dict(s) for s in resp.json().get("schemas", [])]

    async def get_bus_schema(
        self, namespace: str, tenant: str, subject: str, version: int,
    ) -> BusSchema:
        resp = await self._request(
            "GET",
            f"/v1/bus/schemas/{_seg(namespace)}/{_seg(tenant)}/{_seg(subject)}/{version}",
        )
        _raise_for_status(resp)
        return BusSchema.from_dict(resp.json())

    async def delete_bus_schema(
        self, namespace: str, tenant: str, subject: str, version: int,
    ) -> None:
        resp = await self._request(
            "DELETE",
            f"/v1/bus/schemas/{_seg(namespace)}/{_seg(tenant)}/{_seg(subject)}/{version}",
        )
        _raise_for_status(resp)

    # --------------- Phase 4: Agents + heartbeat ---------------

    async def register_bus_agent(self, req: RegisterBusAgent) -> BusAgent:
        resp = await self._request("POST", "/v1/bus/agents", json=req.to_dict())
        _raise_for_status(resp)
        return BusAgent.from_dict(resp.json())

    async def list_bus_agents(
        self,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
    ) -> list[BusAgent]:
        params: dict[str, Any] = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        resp = await self._request("GET", "/v1/bus/agents", params=params or None)
        _raise_for_status(resp)
        return [BusAgent.from_dict(a) for a in resp.json().get("agents", [])]

    async def get_bus_agent(self, namespace: str, tenant: str, agent_id: str) -> BusAgent:
        resp = await self._request(
            "GET",
            f"/v1/bus/agents/{_seg(namespace)}/{_seg(tenant)}/{_seg(agent_id)}",
        )
        _raise_for_status(resp)
        return BusAgent.from_dict(resp.json())

    async def delete_bus_agent(
        self, namespace: str, tenant: str, agent_id: str,
    ) -> None:
        resp = await self._request(
            "DELETE",
            f"/v1/bus/agents/{_seg(namespace)}/{_seg(tenant)}/{_seg(agent_id)}",
        )
        _raise_for_status(resp)

    async def heartbeat_bus_agent(
        self, namespace: str, tenant: str, agent_id: str,
    ) -> BusAgent:
        resp = await self._request(
            "PATCH",
            f"/v1/bus/agents/{_seg(namespace)}/{_seg(tenant)}/{_seg(agent_id)}/heartbeat",
        )
        _raise_for_status(resp)
        return BusAgent.from_dict(resp.json())

    async def set_bus_agent_admin_state(
        self,
        namespace: str,
        tenant: str,
        agent_id: str,
        req: SetBusAgentAdminState,
    ) -> BusAgent:
        """Async counterpart of the sync method of the same name."""
        resp = await self._request(
            "PUT",
            f"/v1/bus/agents/{_seg(namespace)}/{_seg(tenant)}/{_seg(agent_id)}/admin-state",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        return BusAgent.from_dict(resp.json())

    # --------------- Phase 5: Conversations ---------------

    async def create_bus_conversation(self, req: CreateBusConversation) -> BusConversation:
        resp = await self._request("POST", "/v1/bus/conversations", json=req.to_dict())
        _raise_for_status(resp)
        return BusConversation.from_dict(resp.json())

    async def list_bus_conversations(
        self,
        *,
        namespace: Optional[str] = None,
        tenant: Optional[str] = None,
        state: Optional[str] = None,
        participant: Optional[str] = None,
    ) -> list[BusConversation]:
        params: dict[str, Any] = {}
        if namespace is not None:
            params["namespace"] = namespace
        if tenant is not None:
            params["tenant"] = tenant
        if state is not None:
            params["state"] = state
        if participant is not None:
            params["participant"] = participant
        resp = await self._request("GET", "/v1/bus/conversations", params=params or None)
        _raise_for_status(resp)
        return [BusConversation.from_dict(c) for c in resp.json().get("conversations", [])]

    async def get_bus_conversation(
        self, namespace: str, tenant: str, conversation_id: str,
    ) -> BusConversation:
        resp = await self._request(
            "GET",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}",
        )
        _raise_for_status(resp)
        return BusConversation.from_dict(resp.json())

    async def delete_bus_conversation(
        self, namespace: str, tenant: str, conversation_id: str,
    ) -> None:
        resp = await self._request(
            "DELETE",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}",
        )
        _raise_for_status(resp)

    async def transition_bus_conversation(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        *,
        target_state: str,
    ) -> BusConversation:
        resp = await self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/transition",
            json={"target_state": target_state},
        )
        _raise_for_status(resp)
        return BusConversation.from_dict(resp.json())

    async def append_bus_conversation_message(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        req: AppendBusConversationMessage,
    ) -> dict[str, Any]:
        resp = await self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/messages",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        return resp.json()

    async def replay_bus_conversation_messages(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        *,
        limit: Optional[int] = None,
        cursor: Optional[str] = None,
        as_agent: Optional[str] = None,
    ) -> BusReplayResponse:
        params: dict[str, Any] = {}
        if limit is not None:
            params["limit"] = limit
        if cursor is not None:
            params["cursor"] = cursor
        if as_agent is not None:
            params["as_agent"] = as_agent
        resp = await self._request(
            "GET",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/messages",
            params=params or None,
        )
        _raise_for_status(resp)
        return BusReplayResponse.from_dict(resp.json())

    # --------------- Phase 6a: Tool envelopes ---------------

    async def post_bus_tool_call(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        req: PostBusToolCall,
    ) -> PostBusToolCallOutcome:
        """See :meth:`_BusClientMixin.post_bus_tool_call` — same
        produced/parked sum-type return.
        """
        resp = await self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/tool-calls",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        body = resp.json()
        if resp.status_code == 202:
            return PostBusToolCallOutcome(
                parked=BusApprovalParkedReceipt.from_dict(body),
            )
        return PostBusToolCallOutcome(
            produced=BusToolEnvelopeReceipt.from_dict(body),
        )

    async def post_bus_tool_result(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        req: PostBusToolResult,
    ) -> BusToolEnvelopeReceipt:
        resp = await self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/tool-results",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        return BusToolEnvelopeReceipt.from_dict(resp.json())

    async def lookup_bus_tool_result(
        self,
        namespace: str,
        tenant: str,
        call_id: str,
        params: BusToolResultLookupParams,
    ) -> BusToolResultLookup:
        resp = await self._request(
            "GET",
            f"/v1/bus/tool-calls/{_seg(namespace)}/{_seg(tenant)}/{_seg(call_id)}/result",
            params=params.to_query(),
        )
        _raise_for_status(resp)
        return BusToolResultLookup.from_dict(resp.json())

    # --------------- Phase 6b: Stream envelopes ---------------

    async def post_bus_stream_chunk(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        req: PostBusStreamChunk,
    ) -> BusStreamEnvelopeReceipt:
        resp = await self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/stream-chunks",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        return BusStreamEnvelopeReceipt.from_dict(resp.json())

    async def post_bus_stream_end(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        req: PostBusStreamEnd,
    ) -> BusStreamEnvelopeReceipt:
        resp = await self._request(
            "POST",
            f"/v1/bus/conversations/{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/stream-end",
            json=req.to_dict(),
        )
        _raise_for_status(resp)
        return BusStreamEnvelopeReceipt.from_dict(resp.json())

    def bus_stream_consume_url(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        stream_id: str,
    ) -> str:
        """Return the SSE consume URL for a stream. Synchronous on
        the async client too — this is a pure URL builder, no
        I/O happens.
        """
        return (
            f"{self.base_url}/v1/bus/streams/"
            f"{_seg(namespace)}/{_seg(tenant)}/{_seg(conversation_id)}/{_seg(stream_id)}"
        )

    async def consume_bus_subscription(
        self,
        subscription_id: str,
        *,
        topic: str,
        from_offset: Optional[str] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ) -> AsyncIterator[BusConsumeItem]:
        """Async version of :meth:`_BusClientMixin.consume_bus_subscription`.

        See the sync counterpart for the reconnect contract: best-
        effort, resumes from ``latest``, yields a typed
        ``Reconnected`` boundary item between attempts.
        """
        if reconnect is None:
            params: dict[str, Any] = {"topic": topic}
            if from_offset is not None:
                params["from"] = from_offset
            url = f"{self.base_url}/v1/bus/subscribe/{_seg(subscription_id)}"
            async for env in _async_open_bus_sse_stream(
                self._client, url, params, self._headers()
            ):
                yield _envelope_to_consume_item(env)
            return

        first_params: dict[str, Any] = {"topic": topic}
        if from_offset is not None:
            first_params["from"] = from_offset
        resume_params: dict[str, Any] = {"topic": topic, "from": "latest"}
        url = f"{self.base_url}/v1/bus/subscribe/{_seg(subscription_id)}"
        attempt = 0
        params_for_open = first_params
        while True:
            try:
                async for env in _async_open_bus_sse_stream(
                    self._client, url, params_for_open, self._headers()
                ):
                    attempt = 0
                    yield _envelope_to_consume_item(env)
            except (ConnectionError, HttpError):
                pass
            if (
                reconnect.max_attempts is not None
                and attempt >= reconnect.max_attempts
            ):
                return
            backoff_ms = _reconnect_backoff_ms(attempt, reconnect)
            await asyncio.sleep(backoff_ms / 1000.0)
            attempt += 1
            yield BusConsumeItem(
                reconnected=ReconnectedInfo(backoff_ms=backoff_ms, attempt=attempt)
            )
            params_for_open = resume_params

    async def consume_bus_stream(
        self,
        namespace: str,
        tenant: str,
        conversation_id: str,
        stream_id: str,
    ) -> AsyncIterator[BusStreamItem]:
        """Async version of :meth:`_BusClientMixin.consume_bus_stream`."""
        url = self.bus_stream_consume_url(namespace, tenant, conversation_id, stream_id)
        async for env in _async_open_bus_sse_stream(
            self._client, url, None, self._headers()
        ):
            item = _envelope_to_stream_item(env)
            yield item
            if item.is_end:
                break

    # --------------- Phase 6c: HITL approvals ---------------

    async def list_bus_approvals(
        self,
        namespace: str,
        tenant: str,
        *,
        status: Optional[str] = None,
        conversation_id: Optional[str] = None,
    ) -> list[BusApprovalView]:
        params: dict[str, Any] = {}
        if status is not None:
            params["status"] = status
        if conversation_id is not None:
            params["conversation_id"] = conversation_id
        resp = await self._request(
            "GET",
            f"/v1/bus/approvals/{_seg(namespace)}/{_seg(tenant)}",
            params=params or None,
        )
        _raise_for_status(resp)
        return [BusApprovalView.from_dict(a) for a in resp.json().get("approvals", [])]

    async def get_bus_approval(
        self, namespace: str, tenant: str, approval_id: str,
    ) -> BusApprovalView:
        resp = await self._request(
            "GET",
            f"/v1/bus/approvals/{_seg(namespace)}/{_seg(tenant)}/{_seg(approval_id)}",
        )
        _raise_for_status(resp)
        return BusApprovalView.from_dict(resp.json())

    async def approve_bus_approval(
        self,
        namespace: str,
        tenant: str,
        approval_id: str,
        decision: BusApprovalDecision,
    ) -> BusApprovalDecisionResponse:
        resp = await self._request(
            "POST",
            f"/v1/bus/approvals/{_seg(namespace)}/{_seg(tenant)}/{_seg(approval_id)}/approve",
            json=decision.to_dict(),
        )
        _raise_for_status(resp)
        return BusApprovalDecisionResponse.from_dict(resp.json())

    async def reject_bus_approval(
        self,
        namespace: str,
        tenant: str,
        approval_id: str,
        decision: BusApprovalDecision,
    ) -> BusApprovalDecisionResponse:
        resp = await self._request(
            "POST",
            f"/v1/bus/approvals/{_seg(namespace)}/{_seg(tenant)}/{_seg(approval_id)}/reject",
            json=decision.to_dict(),
        )
        _raise_for_status(resp)
        return BusApprovalDecisionResponse.from_dict(resp.json())


# ============================================================================
# SSE protocol helpers — shared by sync + async consumers
# ============================================================================
#
# The dispatch SSE parser in `models.py` silently drops keep-alive
# comments. The bus consumers want to surface them as a liveness
# signal, so we use a small private envelope type below and a
# matching parser instead of layering on `_parse_sse_stream`.
#
# Sync vs async parser duplication: Python's sync and async generators
# don't share an underlying state machine cleanly — `for line in lines`
# and `async for line in aiter_lines` differ at the syntactic level,
# and a unified class-based implementation would add more LOC and
# indirection than two short copies. Any frame-level bug fix has to
# touch both functions; keep them in lock-step.


class _SseFrame:
    """One parsed SSE frame: ``event`` (None means default ``message``),
    ``id``, and ``data`` (the joined ``data:`` lines)."""

    __slots__ = ("event", "id", "data")

    def __init__(
        self, event: Optional[str], id_: Optional[str], data: str,
    ) -> None:
        self.event = event
        self.id = id_
        self.data = data


class _KeepAlive:
    """Sentinel value yielded by the parsers for SSE comment lines."""

    __slots__ = ()


_KEEP_ALIVE = _KeepAlive()


def _emit_frame(
    event_type: Optional[str],
    event_id: Optional[str],
    data_parts: list[str],
) -> Optional[_SseFrame]:
    if not data_parts and event_type is None and event_id is None:
        return None
    return _SseFrame(event_type, event_id, "\n".join(data_parts))


def _parse_sse_envelopes(lines: Iterator[str]) -> Iterator[Any]:
    """Yields :class:`_SseFrame` for each frame and :data:`_KEEP_ALIVE`
    for each comment. Mirrors the Rust client's ``sse_envelope_stream``."""
    event_type: Optional[str] = None
    event_id: Optional[str] = None
    data_parts: list[str] = []
    for line in lines:
        if line.startswith(":"):
            yield _KEEP_ALIVE
            continue
        if line == "":
            frame = _emit_frame(event_type, event_id, data_parts)
            if frame is not None:
                yield frame
            event_type = None
            event_id = None
            data_parts = []
            continue
        if line.startswith("event:"):
            event_type = line[len("event:") :].strip()
        elif line.startswith("id:"):
            event_id = line[len("id:") :].strip()
        elif line.startswith("data:"):
            data_parts.append(line[len("data:") :].strip())


async def _async_parse_sse_envelopes(
    aiter_lines: AsyncIterator[str],
) -> AsyncIterator[Any]:
    """Async mirror of :func:`_parse_sse_envelopes`."""
    event_type: Optional[str] = None
    event_id: Optional[str] = None
    data_parts: list[str] = []
    async for line in aiter_lines:
        if line.startswith(":"):
            yield _KEEP_ALIVE
            continue
        if line == "":
            frame = _emit_frame(event_type, event_id, data_parts)
            if frame is not None:
                yield frame
            event_type = None
            event_id = None
            data_parts = []
            continue
        if line.startswith("event:"):
            event_type = line[len("event:") :].strip()
        elif line.startswith("id:"):
            event_id = line[len("id:") :].strip()
        elif line.startswith("data:"):
            data_parts.append(line[len("data:") :].strip())


def _open_bus_sse_stream(
    client: "httpx.Client",
    url: str,
    params: Optional[dict[str, Any]],
    headers: dict[str, str],
) -> Iterator[Any]:
    import httpx as _httpx

    sse_headers = {**headers, "Accept": "text/event-stream"}
    sse_headers.pop("Content-Type", None)
    try:
        with client.stream("GET", url, params=params, headers=sse_headers) as resp:
            if resp.status_code != 200:
                resp.read()
                raise HttpError(resp.status_code, resp.text or "bus consume failed")
            yield from _parse_sse_envelopes(resp.iter_lines())
    except _httpx.ConnectError as e:  # pragma: no cover — network shape
        raise ConnectionError(str(e)) from e
    except _httpx.TimeoutException as e:  # pragma: no cover — network shape
        raise ConnectionError(f"Request timed out: {e}") from e


async def _async_open_bus_sse_stream(
    client: "httpx.AsyncClient",
    url: str,
    params: Optional[dict[str, Any]],
    headers: dict[str, str],
) -> AsyncIterator[Any]:
    import httpx as _httpx

    sse_headers = {**headers, "Accept": "text/event-stream"}
    sse_headers.pop("Content-Type", None)
    try:
        async with client.stream("GET", url, params=params, headers=sse_headers) as resp:
            if resp.status_code != 200:
                await resp.aread()
                raise HttpError(resp.status_code, resp.text or "bus consume failed")
            async for env in _async_parse_sse_envelopes(resp.aiter_lines()):
                yield env
    except _httpx.ConnectError as e:  # pragma: no cover — network shape
        raise ConnectionError(str(e)) from e
    except _httpx.TimeoutException as e:  # pragma: no cover — network shape
        raise ConnectionError(f"Request timed out: {e}") from e


def _decode_data_or_passthrough(data: str) -> Any:
    try:
        return json.loads(data)
    except (json.JSONDecodeError, ValueError):
        return data


def _extract_error_message(data: str) -> str:
    decoded = _decode_data_or_passthrough(data)
    if isinstance(decoded, dict) and isinstance(decoded.get("error"), str):
        return decoded["error"]
    return data


def _envelope_to_consume_item(env: Any) -> BusConsumeItem:
    if isinstance(env, _KeepAlive):
        return BusConsumeItem()
    frame: _SseFrame = env
    name = frame.event or "message"
    if name in ("bus.message", "message"):
        decoded = _decode_data_or_passthrough(frame.data)
        if not isinstance(decoded, dict):
            raise ValueError(f"invalid bus.message payload: {frame.data!r}")
        return BusConsumeItem(message=BusConsumedMessage.from_dict(decoded))
    if name == "bus.error":
        return BusConsumeItem(error=_extract_error_message(frame.data))
    raise ValueError(f"unexpected SSE event '{name}' on bus subscribe stream")


def _envelope_to_stream_item(env: Any) -> BusStreamItem:
    if isinstance(env, _KeepAlive):
        return BusStreamItem()
    frame: _SseFrame = env
    name = frame.event or "message"
    if name == "bus.stream.chunk":
        decoded = _decode_data_or_passthrough(frame.data)
        if not isinstance(decoded, dict):
            raise ValueError(f"invalid stream chunk payload: {frame.data!r}")
        return BusStreamItem(chunk=StreamChunkEnvelope.from_dict(decoded))
    if name == "bus.stream.end":
        decoded = _decode_data_or_passthrough(frame.data)
        if not isinstance(decoded, dict):
            raise ValueError(f"invalid stream end payload: {frame.data!r}")
        return BusStreamItem(end=StreamEndEnvelope.from_dict(decoded))
    if name == "bus.stream.error":
        return BusStreamItem(error=_extract_error_message(frame.data))
    raise ValueError(f"unexpected SSE event '{name}' on bus stream consumer")


def _reconnect_backoff_ms(attempt: int, cfg: ReconnectConfig) -> int:
    """Exponential backoff capped at ``cfg.max_backoff_ms``.

    The shift is bounded at 20 so wild attempt counters can't
    overflow the ``int`` arithmetic Python falls back to here.
    """
    shift = min(attempt, 20)
    exp = cfg.initial_backoff_ms * (1 << shift)
    return min(exp, cfg.max_backoff_ms)
