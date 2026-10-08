"""Host-owned receipts for authenticated individual-agent services."""

from __future__ import annotations

from dataclasses import dataclass, field
import json
import unicodedata
from typing import TYPE_CHECKING, Any, Literal
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
    """Future starts are blocked; provider finality is reported separately."""

    task: dict[str, Any]
    future_starts_blocked: bool
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
    state: Literal["unsupported", "rejected", "uncertain", "reconciled"]
    task: dict[str, Any] | None = None
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


def _peer_token(value: Any) -> bool:
    return (
        isinstance(value, str)
        and 0 < len(value) <= 120
        and all(
            char.isascii() and (char.isalnum() or char in "-_.") for char in value
        )
    )


def _peer_options(
    response: httpx.Response, skill: str
) -> tuple[AgentPeerSelectionOption, ...]:
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
            or (description is not None and (not isinstance(description, str) or len(description.encode("utf-8")) > 2048))
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
    return value


def _peer_receipt(
    response: httpx.Response, namespace: str, tenant: str
) -> AgentPeerSendReceipt:
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
            return AgentPeerCancelReceipt(
                peer.submission_id, cancellation_id, state, code=code
            )
    if state == "reconciled" and set(status) == {"state", "task"}:
        task = _task_value(
            status.get("task"), source.namespace, source.tenant, accepted_task_id
        )
        task_status = task.get("status")
        if isinstance(task_status, dict) and task_status.get("state") in {
            "completed",
            "failed",
            "canceled",
            "rejected",
        }:
            return AgentPeerCancelReceipt(
                peer.submission_id, cancellation_id, state, task=task
            )
    raise ActeonError("agent peer cancellation receipt missing or malformed")


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
    if not isinstance(value, dict) or value.get("future_starts_blocked") is not True:
        raise ActeonError("agent service stop acknowledgement missing or malformed")
    return AgentServiceStopReceipt(
        _task_value(value.get("task"), receipt.namespace, receipt.tenant, receipt.task_id),
        True,
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


class _AgentServicesMixin:
    if TYPE_CHECKING:

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
            or (
                peer.state == refreshed.state == "rejected"
                and peer.code == refreshed.code
            )
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


class _AsyncAgentServicesMixin:
    if TYPE_CHECKING:

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
            or (
                peer.state == refreshed.state == "rejected"
                and peer.code == refreshed.code
            )
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
