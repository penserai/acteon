"""Host-owned receipts for authenticated individual-agent services."""

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

import httpx

from .a2a import _A2A_HEADERS, A2A_PROTOCOL_VERSION, _seg
from .errors import ActeonError, HttpError

AGENT_SOURCE_CONTEXT_HEADER = "x-acteon-agent-source-context"


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
class AgentServiceStopReceipt:
    """Future starts are blocked; an external effect may still complete."""

    task: dict[str, Any]
    future_starts_blocked: bool


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
        _task_value(value.get("task"), receipt.namespace, receipt.tenant, receipt.task_id), True
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
            extra_headers: dict[str, str] | None = None,
        ) -> httpx.Response: ...

    def agent_service_send_message(
        self, namespace: str, tenant: str, agent: str, message: dict[str, Any]
    ) -> AgentServiceReceipt:
        """Submit once; reuse the same message ID after response loss. No automatic retry."""
        response = self._request(
            "POST",
            _base(namespace, tenant, agent) + "/message:send",
            json={"message": message},
            extra_headers=_A2A_HEADERS,
        )
        return _receipt(response, namespace, tenant, agent)

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
            extra_headers: dict[str, str] | None = None,
        ) -> httpx.Response: ...

    async def agent_service_send_message(
        self, namespace: str, tenant: str, agent: str, message: dict[str, Any]
    ) -> AgentServiceReceipt:
        """Submit once and retain the original message identity on response loss."""
        response = await self._request(
            "POST",
            _base(namespace, tenant, agent) + "/message:send",
            json={"message": message},
            extra_headers=_A2A_HEADERS,
        )
        return _receipt(response, namespace, tenant, agent)

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
