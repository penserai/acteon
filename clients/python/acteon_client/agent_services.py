"""Host-owned receipts for authenticated individual-agent services."""

from __future__ import annotations

import json
import unicodedata
from collections.abc import AsyncIterator, Iterator
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Literal, TypeGuard, cast
from uuid import UUID

import httpx

from .a2a import _A2A_HEADERS, A2A_PROTOCOL_VERSION, _seg
from .errors import ActeonError, HttpError
from .models import PermitReference

AGENT_SOURCE_CONTEXT_HEADER = "x-acteon-agent-source-context"
AGENT_EXECUTION_CONTEXT_HEADER = "x-acteon-execution-context"
EXECUTION_PERMITS_HEADER = "x-acteon-execution-permits"


def _source(value: str | None) -> str:
    if (
        not value
        or len(value) > 8192
        or not all(c.isascii() and (c.isalnum() or c in "-_") for c in value)
    ):
        raise ActeonError("agent service source context missing or malformed")
    return value


def _segment(value: str) -> str:
    if value in ("", ".", ".."):
        raise ValueError("invalid agent service path segment")
    return _seg(value)


@dataclass(frozen=True)
class AgentServiceReceipt:
    """Persist in host state separately from model data; acceptance is not execution."""

    namespace: str
    tenant: str
    agent: str
    task_id: str
    source_context: str = field(repr=False)
    task: dict[str, Any]


@dataclass(frozen=True)
class AgentServiceA2aFrame:
    """One official StreamResponse plus out-of-band admission provenance."""

    result: dict[str, Any]
    source_context: str | None = field(default=None, repr=False)


@dataclass(frozen=True)
class AgentServiceParent:
    """Opaque verified context plus explicit permit references for delegation."""

    execution_context: str = field(repr=False)
    permits: tuple[PermitReference, ...]


def _parent_headers(parent: AgentServiceParent | None) -> dict[str, str]:
    if parent is None:
        return dict(_A2A_HEADERS)
    context = parent.execution_context
    if (
        not context
        or len(context) > 8192
        or not all(c.isascii() and (c.isalnum() or c in "-_") for c in context)
        or not parent.permits
    ):
        raise ActeonError("delegated agent service parent is malformed")
    return {
        **_A2A_HEADERS,
        AGENT_EXECUTION_CONTEXT_HEADER: context,
        EXECUTION_PERMITS_HEADER: json.dumps(
            [permit.to_dict() for permit in parent.permits], separators=(",", ":")
        ),
    }


@dataclass(frozen=True)
class AgentServiceStopReceipt:
    """Cancellation intent with confirmed finality reported separately."""

    task: dict[str, Any]
    future_starts_blocked: bool
    cancellation_confirmed: bool
    provider_abort: AgentServiceProviderAbort | None = None


@dataclass(frozen=True)
class AgentServiceProviderAbort:
    """Honest provider-side abort state for the registered attempt."""

    state: Literal["restricted_only", "uncertain", "reconciled"]
    attempt_id: str | None = None
    proof_digest: str | None = None


@dataclass(frozen=True)
class AgentPeerSendReceipt:
    """Durable send result; uncertain does not prove rejection."""

    submission_id: str
    state: Literal["uncertain", "accepted", "rejected"]
    task: dict[str, Any] | None = None
    code: str | None = None


@dataclass(frozen=True)
class AgentPeerCancelReceipt:
    """Durable at-most-once cancellation outcome."""

    submission_id: str
    cancellation_id: str
    state: Literal["unsupported", "rejected", "uncertain", "restricted", "reconciled"]
    task: dict[str, Any] | None = None
    code: str | None = None


@dataclass(frozen=True)
class AgentPeerContinuationReceipt:
    """Durable at-most-once response to one exact peer input challenge."""

    submission_id: str
    continuation_id: str
    state: Literal["uncertain", "accepted", "rejected"]
    task: dict[str, Any] | None = None
    progress_cursor: str | None = None
    code: str | None = None


@dataclass(frozen=True)
class AgentPeerAuthorizationReceipt:
    """Durable handoff for one exact remote authorization challenge."""

    submission_id: str
    authorization_id: str
    state: Literal["uncertain", "resolved", "rejected"]
    task: dict[str, Any] | None = None
    progress_cursor: str | None = None
    code: str | None = None


@dataclass(frozen=True)
class AgentPeerSelectionOption:
    """Safe registry data; description_untrusted is never a host instruction."""

    agent_id: str
    skill: str
    description_untrusted: str | None
    card_version: str
    binding_digest: str
    checked_at_ms: int


def _peer_token(value: Any) -> TypeGuard[str]:
    return (
        isinstance(value, str)
        and 0 < len(value) <= 120
        and all(char.isascii() and (char.isalnum() or char in "-_.") for char in value)
    )


def _challenge_id(value: Any) -> TypeGuard[str]:
    return (
        isinstance(value, str)
        and 0 < len(value) <= 1024
        and value.strip() == value
        and value != "*"
        and not any(unicodedata.category(char) == "Cc" for char in value)
    )


def _peer_options(response: httpx.Response, skill: str) -> tuple[AgentPeerSelectionOption, ...]:
    value = _response_value(response)
    if not isinstance(value, dict) or set(value) != {"peers"}:
        raise ActeonError("agent peer discovery response missing or malformed")
    peers = value.get("peers")
    if not isinstance(peers, list) or len(peers) > 128:
        raise ActeonError("agent peer discovery response missing or malformed")
    parsed = []
    seen = set()
    expected = {
        "agent_id",
        "skill",
        "description_untrusted",
        "card_version",
        "binding_digest",
        "checked_at_ms",
    }
    for peer in peers:
        if not isinstance(peer, dict) or set(peer) != expected:
            raise ActeonError("agent peer discovery response missing or malformed")
        agent_id = peer.get("agent_id")
        description = peer.get("description_untrusted")
        digest = peer.get("binding_digest")
        checked_at_ms = peer.get("checked_at_ms")
        if (
            not _peer_token(agent_id)
            or agent_id in seen
            or peer.get("skill") != skill
            or not _peer_token(peer.get("skill"))
            or (
                description is not None
                and (not isinstance(description, str) or len(description.encode("utf-8")) > 2048)
            )
            or not _peer_token(peer.get("card_version"))
            or not isinstance(digest, str)
            or len(digest) != 64
            or any(char not in "0123456789abcdef" for char in digest)
            or type(checked_at_ms) is not int
            or checked_at_ms < 0
            or checked_at_ms > 9_223_372_036_854_775_807
        ):
            raise ActeonError("agent peer discovery response missing or malformed")
        seen.add(agent_id)
        parsed.append(
            AgentPeerSelectionOption(
                agent_id,
                skill,
                description,
                peer["card_version"],
                digest,
                checked_at_ms,
            )
        )
    return tuple(parsed)


def _submission(value: Any) -> str:
    try:
        parsed = UUID(value) if isinstance(value, str) else None
    except ValueError:
        parsed = None
    if parsed is None or parsed.version != 5 or str(parsed) != value:
        raise ActeonError("agent peer submission identity missing or malformed")
    return cast(str, value)


def _peer_receipt(response: httpx.Response, namespace: str, tenant: str) -> AgentPeerSendReceipt:
    value = _response_value(response)
    if not isinstance(value, dict) or set(value) != {"submission_id", "status"}:
        raise ActeonError("agent peer receipt missing or malformed")
    submission_id = _submission(value.get("submission_id"))
    status = value.get("status")
    if not isinstance(status, dict):
        raise ActeonError("agent peer receipt missing or malformed")
    state = status.get("state")
    if state == "uncertain" and set(status) == {"state"}:
        return AgentPeerSendReceipt(submission_id, state)
    if state == "accepted" and set(status) == {"state", "task"}:
        return AgentPeerSendReceipt(
            submission_id, state, task=_task_value(status.get("task"), namespace, tenant)
        )
    if state == "rejected" and set(status) == {"state", "code"}:
        code = status.get("code")
        if (
            isinstance(code, str)
            and 0 < len(code) <= 1024
            and code.strip() == code
            and not any(unicodedata.category(char) == "Cc" for char in code)
        ):
            return AgentPeerSendReceipt(submission_id, state, code=code)
    raise ActeonError("agent peer receipt missing or malformed")


def _peer_cancel_receipt(
    response: httpx.Response,
    source: AgentServiceReceipt,
    peer: AgentPeerSendReceipt,
) -> AgentPeerCancelReceipt:
    value = _response_value(response)
    if (
        peer.state != "accepted"
        or peer.task is None
        or not isinstance(value, dict)
        or set(value) != {"submission_id", "cancellation_id", "status"}
        or value.get("submission_id") != peer.submission_id
    ):
        raise ActeonError("agent peer cancellation receipt missing or malformed")
    accepted_task = _task_value(peer.task, source.namespace, source.tenant)
    accepted_task_id = accepted_task["id"]
    cancellation_id = _submission(value.get("cancellation_id"))
    status = value.get("status")
    if not isinstance(status, dict):
        raise ActeonError("agent peer cancellation receipt missing or malformed")
    state = status.get("state")
    if state in ("unsupported", "uncertain") and set(status) == {"state"}:
        return AgentPeerCancelReceipt(peer.submission_id, cancellation_id, state)
    if state == "rejected" and set(status) == {"state", "code"}:
        code = status.get("code")
        if (
            isinstance(code, str)
            and 0 < len(code) <= 1024
            and code.strip() == code
            and not any(unicodedata.category(char) == "Cc" for char in code)
        ):
            return AgentPeerCancelReceipt(peer.submission_id, cancellation_id, state, code=code)
    if state in ("restricted", "reconciled") and set(status) == {"state", "task"}:
        task = _task_value(status.get("task"), source.namespace, source.tenant, accepted_task_id)
        task_status = task.get("status")
        task_state = task_status.get("state") if isinstance(task_status, dict) else None
        if task_state not in {
            "submitted",
            "working",
            "completed",
            "failed",
            "canceled",
            "input_required",
            "auth_required",
            "rejected",
        }:
            raise ActeonError("agent peer cancellation receipt missing or malformed")
        terminal = task_state in {
            "completed",
            "failed",
            "canceled",
            "rejected",
        }
        if (state == "restricted" and not terminal) or (state == "reconciled" and terminal):
            return AgentPeerCancelReceipt(peer.submission_id, cancellation_id, state, task=task)
    raise ActeonError("agent peer cancellation receipt missing or malformed")


def _peer_continuation_receipt(
    response: httpx.Response,
    source: AgentServiceReceipt,
    peer: AgentPeerSendReceipt,
) -> AgentPeerContinuationReceipt:
    value = _response_value(response)
    if (
        peer.state != "accepted"
        or peer.task is None
        or not isinstance(value, dict)
        or set(value) != {"submission_id", "continuation_id", "status"}
        or value.get("submission_id") != peer.submission_id
    ):
        raise ActeonError("agent peer continuation receipt missing or malformed")
    accepted = _task_value(peer.task, source.namespace, source.tenant)
    continuation_id = _submission(value.get("continuation_id"))
    status = value.get("status")
    if not isinstance(status, dict):
        raise ActeonError("agent peer continuation receipt missing or malformed")
    state = status.get("state")
    if state == "uncertain" and set(status) == {"state"}:
        return AgentPeerContinuationReceipt(peer.submission_id, continuation_id, state)
    if state == "rejected" and set(status) == {"state", "code"}:
        code = status.get("code")
        if (
            isinstance(code, str)
            and 0 < len(code) <= 1024
            and code.strip() == code
            and not any(unicodedata.category(char) == "Cc" for char in code)
        ):
            return AgentPeerContinuationReceipt(
                peer.submission_id, continuation_id, state, code=code
            )
    if state == "accepted" and set(status) == {"state", "task", "progress_cursor"}:
        cursor = status.get("progress_cursor")
        if (
            isinstance(cursor, str)
            and len(cursor) <= 512
            and cursor.startswith('"')
            and cursor.endswith('"')
            and cursor[1:-1]
            and all(char.isascii() and (char.isalnum() or char in "-_.:") for char in cursor[1:-1])
        ):
            task = _task_value(status.get("task"), source.namespace, source.tenant, accepted["id"])
            return AgentPeerContinuationReceipt(
                peer.submission_id,
                continuation_id,
                state,
                task=task,
                progress_cursor=cursor,
            )
    raise ActeonError("agent peer continuation receipt missing or malformed")


def _peer_authorization_receipt(
    response: httpx.Response,
    source: AgentServiceReceipt,
    peer: AgentPeerSendReceipt,
    challenge_id: str,
) -> AgentPeerAuthorizationReceipt:
    value = _response_value(response)
    if (
        peer.state != "accepted"
        or peer.task is None
        or not isinstance(value, dict)
        or set(value) != {"submission_id", "authorization_id", "status"}
        or value.get("submission_id") != peer.submission_id
    ):
        raise ActeonError("agent peer authorization receipt missing or malformed")
    accepted = _task_value(peer.task, source.namespace, source.tenant)
    authorization_id = _submission(value.get("authorization_id"))
    status = value.get("status")
    if not isinstance(status, dict):
        raise ActeonError("agent peer authorization receipt missing or malformed")
    state = status.get("state")
    if state == "uncertain" and set(status) == {"state"}:
        return AgentPeerAuthorizationReceipt(peer.submission_id, authorization_id, state)
    if state == "rejected" and set(status) == {"state", "code"}:
        code = status.get("code")
        if _challenge_id(code):
            return AgentPeerAuthorizationReceipt(
                peer.submission_id, authorization_id, state, code=code
            )
    if state == "resolved" and set(status) == {"state", "task", "progress_cursor"}:
        cursor = status.get("progress_cursor")
        if (
            isinstance(cursor, str)
            and len(cursor) <= 512
            and cursor.startswith('"')
            and cursor.endswith('"')
            and cursor[1:-1]
            and all(char.isascii() and (char.isalnum() or char in "-_.:") for char in cursor[1:-1])
        ):
            task = _task_value(status.get("task"), source.namespace, source.tenant, accepted["id"])
            task_status = task.get("status")
            task_state = task_status.get("state") if isinstance(task_status, dict) else None
            if task_state not in {"working", "completed", "input_required", "auth_required"} or (
                task_state == "auth_required" and task.get("pendingApprovalId") == challenge_id
            ):
                raise ActeonError("agent peer authorization receipt missing or malformed")
            return AgentPeerAuthorizationReceipt(
                peer.submission_id,
                authorization_id,
                state,
                task=task,
                progress_cursor=cursor,
            )
    raise ActeonError("agent peer authorization receipt missing or malformed")


def _unbound_peer_response(message: dict[str, Any]) -> None:
    metadata = message.get("metadata")
    if (
        message.get("role") != "user"
        or message.get("taskId") is not None
        or message.get("contextId") is not None
        or (isinstance(metadata, dict) and "acteon.challengeId" in metadata)
    ):
        raise ValueError("agent peer continuation requires an unbound user response")


def _provider_abort(value: Any) -> AgentServiceProviderAbort | None:
    if value is None:
        return None
    if not isinstance(value, dict) or not isinstance(value.get("state"), str):
        raise ActeonError("agent service provider abort status malformed")
    state = value["state"]
    if state == "restricted_only" and set(value) == {"state"}:
        return AgentServiceProviderAbort(state)
    if state == "uncertain" and set(value) == {"state", "attempt_id"}:
        attempt_id = value.get("attempt_id")
        if isinstance(attempt_id, str):
            try:
                parsed = UUID(attempt_id)
            except ValueError:
                pass
            else:
                if parsed.version == 5 and str(parsed) == attempt_id:
                    return AgentServiceProviderAbort(state, attempt_id=attempt_id)
    if state == "reconciled" and set(value) == {"state", "proof_digest"}:
        digest = value.get("proof_digest")
        if (
            isinstance(digest, str)
            and len(digest) == 64
            and all(c in "0123456789abcdef" for c in digest)
        ):
            return AgentServiceProviderAbort(state, proof_digest=digest)
    raise ActeonError("agent service provider abort status malformed")


def _task_value(
    task: Any, namespace: str, tenant: str, task_id: str | None = None
) -> dict[str, Any]:
    if (
        not isinstance(task, dict)
        or not isinstance(task.get("id"), str)
        or not task["id"]
        or task.get("namespace") != namespace
        or task.get("tenant") != tenant
        or (task_id is not None and task["id"] != task_id)
    ):
        raise ActeonError("agent service task identity mismatch")
    return task


def _response_value(response: httpx.Response) -> Any:
    if not 200 <= response.status_code < 300:
        raise HttpError(response.status_code, response.text)
    if response.headers.get("a2a-version") != A2A_PROTOCOL_VERSION:
        raise ActeonError("agent service response version missing or unsupported")
    return response.json()


def _stop(response: httpx.Response, receipt: AgentServiceReceipt) -> AgentServiceStopReceipt:
    value = _response_value(response)
    if (
        not isinstance(value, dict)
        or value.get("future_starts_blocked") is not True
        or not isinstance(value.get("cancellation_confirmed"), bool)
    ):
        raise ActeonError("agent service stop acknowledgement missing or malformed")
    task = _task_value(value.get("task"), receipt.namespace, receipt.tenant, receipt.task_id)
    if value["cancellation_confirmed"] and task.get("status", {}).get("state") != "canceled":
        raise ActeonError("agent service stop acknowledgement missing or malformed")
    return AgentServiceStopReceipt(
        task,
        True,
        value["cancellation_confirmed"],
        _provider_abort(value.get("provider_abort")),
    )


def _task(
    response: httpx.Response, namespace: str, tenant: str, task_id: str | None = None
) -> dict[str, Any]:
    return _task_value(_response_value(response), namespace, tenant, task_id)


def _receipt(
    response: httpx.Response, namespace: str, tenant: str, agent: str
) -> AgentServiceReceipt:
    task = _task(response, namespace, tenant)
    return AgentServiceReceipt(
        namespace,
        tenant,
        agent,
        task["id"],
        _source(response.headers.get(AGENT_SOURCE_CONTEXT_HEADER)),
        task,
    )


def _base(namespace: str, tenant: str, agent: str) -> str:
    return f"/a2a/{_segment(namespace)}/{_segment(tenant)}/agents/{_segment(agent)}/v1"


def _rpc_base(namespace: str, tenant: str, agent: str) -> str:
    return f"/a2a/{_segment(namespace)}/{_segment(tenant)}/agents/{_segment(agent)}"


def _a2a_result(data: str, request_id: str, source: str | None) -> AgentServiceA2aFrame:
    try:
        value = json.loads(data)
    except (json.JSONDecodeError, ValueError) as decode_error:
        raise ActeonError(f"invalid A2A SSE frame: {decode_error}") from decode_error
    if (
        not isinstance(value, dict)
        or value.get("jsonrpc") != "2.0"
        or value.get("id") != request_id
    ):
        raise ActeonError("A2A SSE frame request identity mismatch")
    if isinstance(value.get("error"), dict):
        error = value["error"]
        raise ActeonError(
            f"A2A error {error.get('code', 'A2A')}: {error.get('message', 'stream failed')}"
        )
    result = value.get("result")
    if not isinstance(result, dict):
        raise ActeonError("A2A SSE frame has no object result")
    return AgentServiceA2aFrame(result, source)


def _sse_data(lines: Iterator[str]) -> Iterator[str]:
    data: list[str] = []
    size = 0
    for line in lines:
        if line == "":
            if data:
                yield "\n".join(data)
            data = []
            size = 0
        elif line.startswith("data:"):
            value = line[len("data:") :].lstrip()
            size += len(value.encode("utf-8"))
            if size > 2 * 1024 * 1024:
                raise ActeonError("A2A SSE frame exceeds 2 MiB")
            data.append(value)
    if data:
        yield "\n".join(data)


async def _async_sse_data(lines: AsyncIterator[str]) -> AsyncIterator[str]:
    data: list[str] = []
    size = 0
    async for line in lines:
        if line == "":
            if data:
                yield "\n".join(data)
            data = []
            size = 0
        elif line.startswith("data:"):
            value = line[len("data:") :].lstrip()
            size += len(value.encode("utf-8"))
            if size > 2 * 1024 * 1024:
                raise ActeonError("A2A SSE frame exceeds 2 MiB")
            data.append(value)
    if data:
        yield "\n".join(data)


class _AgentServicesMixin:
    if TYPE_CHECKING:
        base_url: str
        _client: httpx.Client

        def _headers(self) -> dict[str, str]: ...

        def _request(
            self,
            method: str,
            path: str,
            *,
            json: Any = None,
            params: dict[str, Any] | None = None,
            extra_headers: dict[str, str] | None = None,
        ) -> httpx.Response: ...

    def agent_service_send_message(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        message: dict[str, Any],
        *,
        parent: AgentServiceParent | None = None,
    ) -> AgentServiceReceipt:
        """Submit once; reuse the same message ID after response loss. No automatic retry."""
        response = self._request(
            "POST",
            _base(namespace, tenant, agent) + "/message:send",
            json={"message": message},
            extra_headers=_parent_headers(parent),
        )
        return _receipt(response, namespace, tenant, agent)

    def agent_service_send_streaming_message(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        message: dict[str, Any],
        *,
        parent: AgentServiceParent | None = None,
    ) -> Iterator[AgentServiceA2aFrame]:
        """Invoke one exact governed agent using official A2A SSE."""
        message_id = message.get("messageId")
        if not isinstance(message_id, str) or not message_id:
            raise ValueError("streaming message requires messageId")
        request_id = f"stream:{message_id}"
        headers = {**self._headers(), **_parent_headers(parent), "Accept": "text/event-stream"}
        headers.pop("Content-Type", None)
        envelope = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "SendStreamingMessage",
            "params": {"message": message},
        }
        with self._client.stream(
            "POST",
            f"{self.base_url}{_rpc_base(namespace, tenant, agent)}",
            json=envelope,
            headers=headers,
            follow_redirects=False,
            timeout=None,
        ) as response:
            if response.status_code != 200:
                response.read()
                raise HttpError(response.status_code, response.text or "agent A2A stream failed")
            if response.headers.get("a2a-version") != A2A_PROTOCOL_VERSION:
                raise ActeonError("agent service A2A stream version missing or unsupported")
            if not response.headers.get("content-type", "").lower().startswith("text/event-stream"):
                response.read()
                raise ActeonError(response.text or "agent service did not return an A2A SSE stream")
            source = _source(response.headers.get(AGENT_SOURCE_CONTEXT_HEADER))
            for data in _sse_data(response.iter_lines()):
                yield _a2a_result(data, request_id, source)

    def agent_service_subscribe_task(
        self, receipt: AgentServiceReceipt
    ) -> Iterator[AgentServiceA2aFrame]:
        """Subscribe to one retained governed Task using official A2A SSE."""
        request_id = f"subscribe:{receipt.task_id}"
        headers = {
            **self._headers(),
            **_A2A_HEADERS,
            "Accept": "text/event-stream",
            AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
        }
        headers.pop("Content-Type", None)
        envelope = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "SubscribeToTask",
            "params": {"id": receipt.task_id},
        }
        with self._client.stream(
            "POST",
            f"{self.base_url}{_rpc_base(receipt.namespace, receipt.tenant, receipt.agent)}",
            json=envelope,
            headers=headers,
            follow_redirects=False,
            timeout=None,
        ) as response:
            if response.status_code != 200:
                response.read()
                raise HttpError(response.status_code, response.text or "agent A2A subscribe failed")
            if response.headers.get("a2a-version") != A2A_PROTOCOL_VERSION:
                raise ActeonError("agent service A2A stream version missing or unsupported")
            if not response.headers.get("content-type", "").lower().startswith("text/event-stream"):
                response.read()
                raise ActeonError(response.text or "agent service did not return an A2A SSE stream")
            for data in _sse_data(response.iter_lines()):
                yield _a2a_result(data, request_id, None)

    def agent_service_cancel_task(self, receipt: AgentServiceReceipt) -> dict[str, Any]:
        """Request official A2A cancellation and preserve pending finality metadata."""
        request_id = f"cancel:{receipt.task_id}"
        response = self._client.post(
            f"{self.base_url}{_rpc_base(receipt.namespace, receipt.tenant, receipt.agent)}",
            json={
                "jsonrpc": "2.0",
                "id": request_id,
                "method": "CancelTask",
                "params": {"id": receipt.task_id},
            },
            headers={
                **self._headers(),
                **_A2A_HEADERS,
                AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
            },
            follow_redirects=False,
        )
        if response.status_code != 200:
            raise HttpError(response.status_code, response.text)
        if response.headers.get("a2a-version") != A2A_PROTOCOL_VERSION:
            raise ActeonError("agent service A2A response version missing or unsupported")
        task = _a2a_result(response.text, request_id, None).result
        if task.get("id") != receipt.task_id or not isinstance(task.get("status"), dict):
            raise ActeonError("agent service A2A cancellation task identity mismatch")
        return task

    def agent_service_send_peer(
        self,
        source: AgentServiceReceipt,
        target: str,
        skill: str,
        message: dict[str, Any],
    ) -> AgentPeerSendReceipt:
        """Submit once from an accepted source task; sends no authority fields."""
        response = self._request(
            "POST",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers/"
            + _segment(target)
            + "/"
            + _segment(skill)
            + "/message:send",
            json={"message": message},
            extra_headers=dict(_A2A_HEADERS),
        )
        return _peer_receipt(response, source.namespace, source.tenant)

    def agent_service_discover_peers(
        self, source: AgentServiceReceipt, skill: str
    ) -> tuple[AgentPeerSelectionOption, ...]:
        """List safe current options; registry descriptions remain untrusted."""
        if not _peer_token(skill) or skill == "*":
            raise ValueError("invalid exact peer skill")
        response = self._request(
            "GET",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers",
            params={"skill": skill},
            extra_headers=dict(_A2A_HEADERS),
        )
        return _peer_options(response, skill)

    def agent_service_refresh_peer(
        self,
        source: AgentServiceReceipt,
        target: str,
        skill: str,
        peer: AgentPeerSendReceipt,
    ) -> AgentPeerSendReceipt:
        """Refresh one accepted remote task without resubmitting its message."""
        submission = _submission(peer.submission_id)
        response = self._request(
            "POST",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers/"
            + _segment(target)
            + "/"
            + _segment(skill)
            + "/submissions/"
            + submission
            + ":refresh",
            extra_headers=dict(_A2A_HEADERS),
        )
        refreshed = _peer_receipt(response, source.namespace, source.tenant)
        if refreshed.submission_id != submission:
            raise ActeonError("agent peer submission identity mismatch")
        same_disposition = (
            peer.state == refreshed.state == "uncertain"
            or (
                peer.state == refreshed.state == "accepted"
                and peer.task is not None
                and refreshed.task is not None
                and peer.task.get("id") == refreshed.task.get("id")
            )
            or (peer.state == refreshed.state == "rejected" and peer.code == refreshed.code)
        )
        if not same_disposition:
            raise ActeonError("agent peer refresh changed durable disposition")
        return refreshed

    def agent_service_cancel_peer(
        self,
        source: AgentServiceReceipt,
        target: str,
        skill: str,
        peer: AgentPeerSendReceipt,
    ) -> AgentPeerCancelReceipt:
        """Persist and deliver at most one cancellation; never retries ambiguity."""
        submission = _submission(peer.submission_id)
        if peer.state != "accepted" or peer.task is None:
            raise ActeonError("agent peer cancellation requires an accepted peer receipt")
        _task_value(peer.task, source.namespace, source.tenant)
        response = self._request(
            "POST",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers/"
            + _segment(target)
            + "/"
            + _segment(skill)
            + "/submissions/"
            + submission
            + ":cancel",
            extra_headers=dict(_A2A_HEADERS),
        )
        return _peer_cancel_receipt(response, source, peer)

    def agent_service_continue_peer(
        self,
        source: AgentServiceReceipt,
        target: str,
        skill: str,
        peer: AgentPeerSendReceipt,
        message: dict[str, Any],
    ) -> AgentPeerContinuationReceipt:
        """Respond once to the accepted peer task's active challenge."""
        submission = _submission(peer.submission_id)
        if peer.state != "accepted" or peer.task is None:
            raise ActeonError("agent peer continuation requires an accepted peer receipt")
        _task_value(peer.task, source.namespace, source.tenant)
        _unbound_peer_response(message)
        response = self._request(
            "POST",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers/"
            + _segment(target)
            + "/"
            + _segment(skill)
            + "/submissions/"
            + submission
            + "/message:send",
            json={"message": message},
            extra_headers=dict(_A2A_HEADERS),
        )
        return _peer_continuation_receipt(response, source, peer)

    def agent_service_authorize_peer(
        self,
        source: AgentServiceReceipt,
        target: str,
        skill: str,
        peer: AgentPeerSendReceipt,
        challenge_id: str,
    ) -> AgentPeerAuthorizationReceipt:
        """Resolve one remote challenge without accepting credentials."""
        submission = _submission(peer.submission_id)
        if peer.state != "accepted" or peer.task is None or not _challenge_id(challenge_id):
            raise ActeonError("agent peer authorization requires an accepted peer and challenge")
        _task_value(peer.task, source.namespace, source.tenant)
        response = self._request(
            "POST",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers/"
            + _segment(target)
            + "/"
            + _segment(skill)
            + "/submissions/"
            + submission
            + "/authorization:resolve",
            json={"challengeId": challenge_id},
            extra_headers=dict(_A2A_HEADERS),
        )
        return _peer_authorization_receipt(response, source, peer, challenge_id)

    def agent_service_stop_task(self, receipt: AgentServiceReceipt) -> AgentServiceStopReceipt:
        """Stop future starts; repeat the same receipt explicitly after response loss."""
        response = self._request(
            "POST",
            _base(receipt.namespace, receipt.tenant, receipt.agent)
            + "/tasks/"
            + _segment(receipt.task_id)
            + "/stop",
            extra_headers={
                **_A2A_HEADERS,
                AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
            },
        )
        return _stop(response, receipt)

    def agent_service_get_task(self, receipt: AgentServiceReceipt) -> dict[str, Any]:
        """Observe the retained job without starting work or changing client defaults."""
        response = self._request(
            "GET",
            _base(receipt.namespace, receipt.tenant, receipt.agent)
            + "/tasks/"
            + _segment(receipt.task_id),
            extra_headers={
                **_A2A_HEADERS,
                AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
            },
        )
        return _task(response, receipt.namespace, receipt.tenant, receipt.task_id)

    def agent_service_continue_task(
        self, receipt: AgentServiceReceipt, message: dict[str, Any]
    ) -> dict[str, Any]:
        """Continue the retained task with an exact challenge-bound response."""
        response = self._request(
            "POST",
            _base(receipt.namespace, receipt.tenant, receipt.agent)
            + "/tasks/"
            + _segment(receipt.task_id)
            + "/message:send",
            json={"message": message},
            extra_headers={
                **_A2A_HEADERS,
                AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
            },
        )
        return _task(response, receipt.namespace, receipt.tenant, receipt.task_id)

    def agent_service_request_authorization(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        task_id: str,
        authorization_request_id: str,
    ) -> dict[str, Any]:
        """Open the service's fixed authorization profile with an opaque handle."""
        response = self._request(
            "POST",
            _base(namespace, tenant, agent)
            + "/tasks/"
            + _segment(task_id)
            + "/authorization:request",
            json={"authorizationRequestId": authorization_request_id},
            extra_headers=_A2A_HEADERS,
        )
        return _task(response, namespace, tenant, task_id)

    def agent_service_resolve_authorization(
        self,
        receipt: AgentServiceReceipt,
        challenge_id: str,
    ) -> dict[str, Any]:
        """Resolve an exact challenge; verifier credentials never enter this call."""
        response = self._request(
            "POST",
            _base(receipt.namespace, receipt.tenant, receipt.agent)
            + "/tasks/"
            + _segment(receipt.task_id)
            + "/authorization:resolve",
            json={"challengeId": challenge_id},
            extra_headers={
                **_A2A_HEADERS,
                AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
            },
        )
        return _task(response, receipt.namespace, receipt.tenant, receipt.task_id)


class _AsyncAgentServicesMixin:
    if TYPE_CHECKING:
        base_url: str
        _client: httpx.AsyncClient

        def _headers(self) -> dict[str, str]: ...

        async def _request(
            self,
            method: str,
            path: str,
            *,
            json: Any = None,
            params: dict[str, Any] | None = None,
            extra_headers: dict[str, str] | None = None,
        ) -> httpx.Response: ...

    async def agent_service_send_message(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        message: dict[str, Any],
        *,
        parent: AgentServiceParent | None = None,
    ) -> AgentServiceReceipt:
        """Submit once and retain the original message identity on response loss."""
        response = await self._request(
            "POST",
            _base(namespace, tenant, agent) + "/message:send",
            json={"message": message},
            extra_headers=_parent_headers(parent),
        )
        return _receipt(response, namespace, tenant, agent)

    async def agent_service_send_streaming_message(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        message: dict[str, Any],
        *,
        parent: AgentServiceParent | None = None,
    ) -> AsyncIterator[AgentServiceA2aFrame]:
        """Invoke one exact governed agent using official A2A SSE."""
        message_id = message.get("messageId")
        if not isinstance(message_id, str) or not message_id:
            raise ValueError("streaming message requires messageId")
        request_id = f"stream:{message_id}"
        headers = {**self._headers(), **_parent_headers(parent), "Accept": "text/event-stream"}
        headers.pop("Content-Type", None)
        envelope = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "SendStreamingMessage",
            "params": {"message": message},
        }
        async with self._client.stream(
            "POST",
            f"{self.base_url}{_rpc_base(namespace, tenant, agent)}",
            json=envelope,
            headers=headers,
            follow_redirects=False,
            timeout=None,
        ) as response:
            if response.status_code != 200:
                await response.aread()
                raise HttpError(response.status_code, response.text or "agent A2A stream failed")
            if response.headers.get("a2a-version") != A2A_PROTOCOL_VERSION:
                raise ActeonError("agent service A2A stream version missing or unsupported")
            if not response.headers.get("content-type", "").lower().startswith("text/event-stream"):
                await response.aread()
                raise ActeonError(response.text or "agent service did not return an A2A SSE stream")
            source = _source(response.headers.get(AGENT_SOURCE_CONTEXT_HEADER))
            async for data in _async_sse_data(response.aiter_lines()):
                yield _a2a_result(data, request_id, source)

    async def agent_service_subscribe_task(
        self, receipt: AgentServiceReceipt
    ) -> AsyncIterator[AgentServiceA2aFrame]:
        """Subscribe to one retained governed Task using official A2A SSE."""
        request_id = f"subscribe:{receipt.task_id}"
        headers = {
            **self._headers(),
            **_A2A_HEADERS,
            "Accept": "text/event-stream",
            AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
        }
        headers.pop("Content-Type", None)
        envelope = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "SubscribeToTask",
            "params": {"id": receipt.task_id},
        }
        async with self._client.stream(
            "POST",
            f"{self.base_url}{_rpc_base(receipt.namespace, receipt.tenant, receipt.agent)}",
            json=envelope,
            headers=headers,
            follow_redirects=False,
            timeout=None,
        ) as response:
            if response.status_code != 200:
                await response.aread()
                raise HttpError(response.status_code, response.text or "agent A2A subscribe failed")
            if response.headers.get("a2a-version") != A2A_PROTOCOL_VERSION:
                raise ActeonError("agent service A2A stream version missing or unsupported")
            if not response.headers.get("content-type", "").lower().startswith("text/event-stream"):
                await response.aread()
                raise ActeonError(response.text or "agent service did not return an A2A SSE stream")
            async for data in _async_sse_data(response.aiter_lines()):
                yield _a2a_result(data, request_id, None)

    async def agent_service_cancel_task(self, receipt: AgentServiceReceipt) -> dict[str, Any]:
        """Request official A2A cancellation and preserve pending finality metadata."""
        request_id = f"cancel:{receipt.task_id}"
        response = await self._client.post(
            f"{self.base_url}{_rpc_base(receipt.namespace, receipt.tenant, receipt.agent)}",
            json={
                "jsonrpc": "2.0",
                "id": request_id,
                "method": "CancelTask",
                "params": {"id": receipt.task_id},
            },
            headers={
                **self._headers(),
                **_A2A_HEADERS,
                AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
            },
            follow_redirects=False,
        )
        if response.status_code != 200:
            raise HttpError(response.status_code, response.text)
        if response.headers.get("a2a-version") != A2A_PROTOCOL_VERSION:
            raise ActeonError("agent service A2A response version missing or unsupported")
        task = _a2a_result(response.text, request_id, None).result
        if task.get("id") != receipt.task_id or not isinstance(task.get("status"), dict):
            raise ActeonError("agent service A2A cancellation task identity mismatch")
        return task

    async def agent_service_send_peer(
        self,
        source: AgentServiceReceipt,
        target: str,
        skill: str,
        message: dict[str, Any],
    ) -> AgentPeerSendReceipt:
        response = await self._request(
            "POST",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers/"
            + _segment(target)
            + "/"
            + _segment(skill)
            + "/message:send",
            json={"message": message},
            extra_headers=dict(_A2A_HEADERS),
        )
        return _peer_receipt(response, source.namespace, source.tenant)

    async def agent_service_discover_peers(
        self, source: AgentServiceReceipt, skill: str
    ) -> tuple[AgentPeerSelectionOption, ...]:
        if not _peer_token(skill) or skill == "*":
            raise ValueError("invalid exact peer skill")
        response = await self._request(
            "GET",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers",
            params={"skill": skill},
            extra_headers=dict(_A2A_HEADERS),
        )
        return _peer_options(response, skill)

    async def agent_service_refresh_peer(
        self,
        source: AgentServiceReceipt,
        target: str,
        skill: str,
        peer: AgentPeerSendReceipt,
    ) -> AgentPeerSendReceipt:
        submission = _submission(peer.submission_id)
        response = await self._request(
            "POST",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers/"
            + _segment(target)
            + "/"
            + _segment(skill)
            + "/submissions/"
            + submission
            + ":refresh",
            extra_headers=dict(_A2A_HEADERS),
        )
        refreshed = _peer_receipt(response, source.namespace, source.tenant)
        if refreshed.submission_id != submission:
            raise ActeonError("agent peer submission identity mismatch")
        same_disposition = (
            peer.state == refreshed.state == "uncertain"
            or (
                peer.state == refreshed.state == "accepted"
                and peer.task is not None
                and refreshed.task is not None
                and peer.task.get("id") == refreshed.task.get("id")
            )
            or (peer.state == refreshed.state == "rejected" and peer.code == refreshed.code)
        )
        if not same_disposition:
            raise ActeonError("agent peer refresh changed durable disposition")
        return refreshed

    async def agent_service_cancel_peer(
        self,
        source: AgentServiceReceipt,
        target: str,
        skill: str,
        peer: AgentPeerSendReceipt,
    ) -> AgentPeerCancelReceipt:
        submission = _submission(peer.submission_id)
        if peer.state != "accepted" or peer.task is None:
            raise ActeonError("agent peer cancellation requires an accepted peer receipt")
        _task_value(peer.task, source.namespace, source.tenant)
        response = await self._request(
            "POST",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers/"
            + _segment(target)
            + "/"
            + _segment(skill)
            + "/submissions/"
            + submission
            + ":cancel",
            extra_headers=dict(_A2A_HEADERS),
        )
        return _peer_cancel_receipt(response, source, peer)

    async def agent_service_continue_peer(
        self,
        source: AgentServiceReceipt,
        target: str,
        skill: str,
        peer: AgentPeerSendReceipt,
        message: dict[str, Any],
    ) -> AgentPeerContinuationReceipt:
        submission = _submission(peer.submission_id)
        if peer.state != "accepted" or peer.task is None:
            raise ActeonError("agent peer continuation requires an accepted peer receipt")
        _task_value(peer.task, source.namespace, source.tenant)
        _unbound_peer_response(message)
        response = await self._request(
            "POST",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers/"
            + _segment(target)
            + "/"
            + _segment(skill)
            + "/submissions/"
            + submission
            + "/message:send",
            json={"message": message},
            extra_headers=dict(_A2A_HEADERS),
        )
        return _peer_continuation_receipt(response, source, peer)

    async def agent_service_authorize_peer(
        self,
        source: AgentServiceReceipt,
        target: str,
        skill: str,
        peer: AgentPeerSendReceipt,
        challenge_id: str,
    ) -> AgentPeerAuthorizationReceipt:
        submission = _submission(peer.submission_id)
        if peer.state != "accepted" or peer.task is None or not _challenge_id(challenge_id):
            raise ActeonError("agent peer authorization requires an accepted peer and challenge")
        _task_value(peer.task, source.namespace, source.tenant)
        response = await self._request(
            "POST",
            _base(source.namespace, source.tenant, source.agent)
            + "/tasks/"
            + _segment(source.task_id)
            + "/peers/"
            + _segment(target)
            + "/"
            + _segment(skill)
            + "/submissions/"
            + submission
            + "/authorization:resolve",
            json={"challengeId": challenge_id},
            extra_headers=dict(_A2A_HEADERS),
        )
        return _peer_authorization_receipt(response, source, peer, challenge_id)

    async def agent_service_stop_task(
        self, receipt: AgentServiceReceipt
    ) -> AgentServiceStopReceipt:
        """Stop future starts; repeat the same receipt explicitly after response loss."""
        response = await self._request(
            "POST",
            _base(receipt.namespace, receipt.tenant, receipt.agent)
            + "/tasks/"
            + _segment(receipt.task_id)
            + "/stop",
            extra_headers={
                **_A2A_HEADERS,
                AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
            },
        )
        return _stop(response, receipt)

    async def agent_service_get_task(self, receipt: AgentServiceReceipt) -> dict[str, Any]:
        response = await self._request(
            "GET",
            _base(receipt.namespace, receipt.tenant, receipt.agent)
            + "/tasks/"
            + _segment(receipt.task_id),
            extra_headers={
                **_A2A_HEADERS,
                AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
            },
        )
        return _task(response, receipt.namespace, receipt.tenant, receipt.task_id)

    async def agent_service_continue_task(
        self, receipt: AgentServiceReceipt, message: dict[str, Any]
    ) -> dict[str, Any]:
        response = await self._request(
            "POST",
            _base(receipt.namespace, receipt.tenant, receipt.agent)
            + "/tasks/"
            + _segment(receipt.task_id)
            + "/message:send",
            json={"message": message},
            extra_headers={
                **_A2A_HEADERS,
                AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
            },
        )
        return _task(response, receipt.namespace, receipt.tenant, receipt.task_id)

    async def agent_service_request_authorization(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        task_id: str,
        authorization_request_id: str,
    ) -> dict[str, Any]:
        response = await self._request(
            "POST",
            _base(namespace, tenant, agent)
            + "/tasks/"
            + _segment(task_id)
            + "/authorization:request",
            json={"authorizationRequestId": authorization_request_id},
            extra_headers=_A2A_HEADERS,
        )
        return _task(response, namespace, tenant, task_id)

    async def agent_service_resolve_authorization(
        self,
        receipt: AgentServiceReceipt,
        challenge_id: str,
    ) -> dict[str, Any]:
        response = await self._request(
            "POST",
            _base(receipt.namespace, receipt.tenant, receipt.agent)
            + "/tasks/"
            + _segment(receipt.task_id)
            + "/authorization:resolve",
            json={"challengeId": challenge_id},
            extra_headers={
                **_A2A_HEADERS,
                AGENT_SOURCE_CONTEXT_HEADER: _source(receipt.source_context),
            },
        )
        return _task(response, receipt.namespace, receipt.tenant, receipt.task_id)
