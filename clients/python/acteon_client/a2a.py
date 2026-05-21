"""A2A protocol surface for the Python ActeonClient.

Two mixins — ``_A2AClientMixin`` for sync and ``_AsyncA2AClientMixin``
for async — bolt the A2A REST surface plus the one JSON-RPC-only
method onto :class:`~acteon_client.client.ActeonClient` and
:class:`~acteon_client.client.AsyncActeonClient`. Methods mirror the
Rust client at ``crates/client/src/a2a.rs`` so cross-language code
reads the same.

Wire payloads are kept as ``dict[str, Any]`` matching the A2A JSON
shapes verbatim. A2A spec evolves and the schema is JSON-native; the
explicit dataclass route the bus module uses would force a translation
layer for every field change. The factory helpers
(:func:`make_message`, :func:`make_part_text`, etc.) cover the common
construction cases without the dataclass boilerplate.

Every authenticated call sends ``A2A-Version: 1.0`` so the server's
version negotiation honours version-pinned callers. The discovery
endpoint (``GET /.well-known/agent.json``) is issued *without* the
API-key header — the A2A spec requires it to be unauthenticated.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, Optional
from urllib.parse import quote

from .errors import ApiError, HttpError

if TYPE_CHECKING:
    import httpx

# ---------------------------------------------------------------------
# Wire constants
# ---------------------------------------------------------------------

#: A2A protocol version this client speaks. Matches
#: ``A2A_PROTOCOL_VERSION`` in the Rust client and the server.
A2A_PROTOCOL_VERSION = "1.0"

_A2A_VERSION_HEADER = "A2A-Version"
_A2A_HEADERS = {_A2A_VERSION_HEADER: A2A_PROTOCOL_VERSION}


def _seg(s: str) -> str:
    """Percent-encode a path segment opaquely (no ``/`` passthrough).

    Mirrors ``_seg`` in ``bus.py``: a tenant or task id that happens
    to contain a slash must be escaped, not silently split into
    additional path components.
    """
    return quote(s, safe="")


def _raise_for_status(resp: "httpx.Response") -> None:
    """Translate a non-2xx response into either ``ApiError`` (with the
    server's structured error envelope) or ``HttpError`` (raw body).

    The A2A REST binding uses the same ``{"error": "..."}`` envelope
    Acteon's other handlers use, so this helper covers both A2A and
    cross-handler errors uniformly.
    """
    if 200 <= resp.status_code < 300:
        return
    try:
        data = resp.json()
        message = (
            data.get("error")
            or data.get("message")
            or f"a2a error (status {resp.status_code})"
        )
        raise ApiError(
            code=data.get("code", "A2A"),
            message=message,
            retryable=resp.status_code in (408, 425, 429) or resp.status_code >= 500,
        )
    except ValueError:
        # Not JSON — fall back to a raw HTTP error with the body text.
        raise HttpError(
            status=resp.status_code,
            message=resp.text or "a2a error",
        ) from None


# ---------------------------------------------------------------------
# Factory helpers — the common construction shapes
# ---------------------------------------------------------------------


def make_part_text(text: str) -> dict[str, Any]:
    """Build a text ``Part`` payload — the lightest A2A part shape.

    Caps: the server rejects text > 256 KiB (``MAX_PART_TEXT_BYTES``)
    at validation time.
    """
    return {"text": text}


def make_part_url(href: str) -> dict[str, Any]:
    """Build a URL-reference ``Part``. Use this for payloads that
    exceed the 256 KiB inline cap — the URL points at an external
    store the receiver fetches separately.
    """
    return {"url": href}


def make_part_data(value: Any, media_type: str = "application/json") -> dict[str, Any]:
    """Build a structured-data ``Part``. The server JSON-encodes the
    value to measure against ``MAX_PART_DATA_BYTES = 256 KiB``.
    """
    return {"data": value, "mediaType": media_type}


def make_message(
    message_id: str,
    role: str,
    parts: list[dict[str, Any]],
    *,
    task_id: Optional[str] = None,
    context_id: Optional[str] = None,
) -> dict[str, Any]:
    """Build a ``TaskMessage`` payload.

    Set ``task_id`` to thread this message into an existing Task's
    history; leave it ``None`` to mint a fresh Task on ``send_message``.

    ``role`` must be ``"user"`` or ``"agent"`` — the server validates.
    """
    msg: dict[str, Any] = {
        "messageId": message_id,
        "role": role,
        "parts": parts,
    }
    if task_id is not None:
        msg["taskId"] = task_id
    if context_id is not None:
        msg["contextId"] = context_id
    return msg


def make_push_config(
    url: str,
    *,
    id: Optional[str] = None,  # noqa: A002 - mirrors the spec field
    token: Optional[str] = None,
    authentication: Optional[dict[str, Any]] = None,
) -> dict[str, Any]:
    """Build a ``PushNotificationConfig`` body.

    ``url`` must be ``http://`` or ``https://`` — the server denies
    other schemes at registration time. ``token`` (if set) is sent
    as ``Authorization: Bearer <token>`` on every push POST and is
    treated as a secret server-side.
    """
    body: dict[str, Any] = {"url": url}
    if id is not None:
        body["id"] = id
    if token is not None:
        body["token"] = token
    if authentication is not None:
        body["authentication"] = authentication
    return body


# ---------------------------------------------------------------------
# Sync mixin
# ---------------------------------------------------------------------


class _A2AClientMixin:
    """Mixin providing the A2A protocol surface (sync)."""

    # The mixin doesn't define ``__init__``; these attributes are set
    # by the concrete client class. Stubbed for type-checkers.
    if TYPE_CHECKING:
        def _request(  # noqa: D401
            self,
            method: str,
            path: str,
            *,
            json: Optional[dict] = None,
            params: Optional[dict] = None,
            extra_headers: Optional[dict[str, str]] = None,
            skip_auth: bool = False,
        ) -> "httpx.Response": ...

    # ---- Task lifecycle ----

    def a2a_send_message(
        self,
        namespace: str,
        tenant: str,
        message: dict[str, Any],
    ) -> dict[str, Any]:
        """``POST /a2a/{namespace}/{tenant}/v1/message:send``.

        Start a new A2A Task or continue an existing one. Set
        ``message["taskId"]`` to thread into an existing Task's
        history.
        """
        resp = self._request(
            "POST",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/v1/message:send",
            json={"message": message},
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    def a2a_get_task(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
    ) -> dict[str, Any]:
        """``GET /a2a/{namespace}/{tenant}/v1/tasks/{id}`` — read a
        Task by id. Raises ``ApiError`` with HTTP 404 when the task
        does not exist for the caller.
        """
        resp = self._request(
            "GET",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/v1/tasks/{_seg(task_id)}",
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    def a2a_cancel_task(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
    ) -> dict[str, Any]:
        """``POST /a2a/{namespace}/{tenant}/v1/tasks/{id}:cancel``.

        The ``:cancel`` verb is part of the URL (spec §11) — the
        server splits it off in-handler. Raises ``ApiError`` with
        HTTP 409 ``TaskNotCancelable`` when the task is terminal.
        """
        resp = self._request(
            "POST",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/v1/tasks/{_seg(task_id)}:cancel",
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    # ---- Push-notification configs ----

    def a2a_set_push_config(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
        config: dict[str, Any],
    ) -> dict[str, Any]:
        """``POST .../v1/tasks/{id}/pushNotificationConfigs`` — register
        or upsert a push-notification webhook for a Task.

        Use :func:`make_push_config` to build ``config``.
        """
        resp = self._request(
            "POST",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}"
            f"/v1/tasks/{_seg(task_id)}/pushNotificationConfigs",
            json=config,
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    def a2a_list_push_configs(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
    ) -> list[dict[str, Any]]:
        """``GET .../v1/tasks/{id}/pushNotificationConfigs`` — list
        every config registered for the task.
        """
        resp = self._request(
            "GET",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}"
            f"/v1/tasks/{_seg(task_id)}/pushNotificationConfigs",
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    def a2a_get_push_config(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
        config_id: str,
    ) -> dict[str, Any]:
        """``GET …/pushNotificationConfigs/{cfgId}`` — read one
        config.
        """
        resp = self._request(
            "GET",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/v1/tasks/{_seg(task_id)}"
            f"/pushNotificationConfigs/{_seg(config_id)}",
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    def a2a_delete_push_config(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
        config_id: str,
    ) -> None:
        """``DELETE …/pushNotificationConfigs/{cfgId}``. Raises
        ``ApiError`` with HTTP 404 when the config doesn't exist —
        the server never silently no-ops.
        """
        resp = self._request(
            "DELETE",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/v1/tasks/{_seg(task_id)}"
            f"/pushNotificationConfigs/{_seg(config_id)}",
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)

    # ---- Discovery ----

    def a2a_discover_agent(
        self,
        namespace: str,
        tenant: str,
    ) -> dict[str, Any]:
        """``GET /a2a/{namespace}/{tenant}/.well-known/agent.json`` —
        unauthenticated discovery endpoint.

        Sent **without** the API-key header per the A2A spec. Returns
        the tenant's ``AgentCard`` (single-card verbatim or
        aggregated across registered agents). Raises ``ApiError``
        with HTTP 404 when no agent has published a card.
        """
        resp = self._request(
            "GET",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/.well-known/agent.json",
            skip_auth=True,
        )
        _raise_for_status(resp)
        return resp.json()

    def a2a_get_authenticated_extended_card(
        self,
        namespace: str,
        tenant: str,
    ) -> dict[str, Any]:
        """JSON-RPC ``agent/getAuthenticatedExtendedCard`` — the
        authenticated discovery variant.

        Issued through the JSON-RPC envelope against
        ``POST /a2a/{ns}/{tenant}`` (the A2A spec defines no REST
        counterpart for this method). The returned card has
        ``capabilities.extendedAgentCard = True``.
        """
        envelope = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "agent/getAuthenticatedExtendedCard",
        }
        resp = self._request(
            "POST",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}",
            json=envelope,
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return _unwrap_jsonrpc(resp.json())


# ---------------------------------------------------------------------
# Async mixin (mirrors the sync one method-for-method)
# ---------------------------------------------------------------------


class _AsyncA2AClientMixin:
    """Mixin providing the A2A protocol surface (async)."""

    if TYPE_CHECKING:
        async def _request(  # noqa: D401
            self,
            method: str,
            path: str,
            *,
            json: Optional[dict] = None,
            params: Optional[dict] = None,
            extra_headers: Optional[dict[str, str]] = None,
            skip_auth: bool = False,
        ) -> "httpx.Response": ...

    async def a2a_send_message(
        self,
        namespace: str,
        tenant: str,
        message: dict[str, Any],
    ) -> dict[str, Any]:
        """Async counterpart of :meth:`_A2AClientMixin.a2a_send_message`."""
        resp = await self._request(
            "POST",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/v1/message:send",
            json={"message": message},
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    async def a2a_get_task(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
    ) -> dict[str, Any]:
        """Async counterpart of :meth:`_A2AClientMixin.a2a_get_task`."""
        resp = await self._request(
            "GET",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/v1/tasks/{_seg(task_id)}",
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    async def a2a_cancel_task(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
    ) -> dict[str, Any]:
        """Async counterpart of :meth:`_A2AClientMixin.a2a_cancel_task`."""
        resp = await self._request(
            "POST",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/v1/tasks/{_seg(task_id)}:cancel",
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    async def a2a_set_push_config(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
        config: dict[str, Any],
    ) -> dict[str, Any]:
        """Async counterpart of :meth:`_A2AClientMixin.a2a_set_push_config`."""
        resp = await self._request(
            "POST",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}"
            f"/v1/tasks/{_seg(task_id)}/pushNotificationConfigs",
            json=config,
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    async def a2a_list_push_configs(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
    ) -> list[dict[str, Any]]:
        """Async counterpart of :meth:`_A2AClientMixin.a2a_list_push_configs`."""
        resp = await self._request(
            "GET",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}"
            f"/v1/tasks/{_seg(task_id)}/pushNotificationConfigs",
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    async def a2a_get_push_config(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
        config_id: str,
    ) -> dict[str, Any]:
        """Async counterpart of :meth:`_A2AClientMixin.a2a_get_push_config`."""
        resp = await self._request(
            "GET",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/v1/tasks/{_seg(task_id)}"
            f"/pushNotificationConfigs/{_seg(config_id)}",
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return resp.json()

    async def a2a_delete_push_config(
        self,
        namespace: str,
        tenant: str,
        task_id: str,
        config_id: str,
    ) -> None:
        """Async counterpart of :meth:`_A2AClientMixin.a2a_delete_push_config`."""
        resp = await self._request(
            "DELETE",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/v1/tasks/{_seg(task_id)}"
            f"/pushNotificationConfigs/{_seg(config_id)}",
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)

    async def a2a_discover_agent(
        self,
        namespace: str,
        tenant: str,
    ) -> dict[str, Any]:
        """Async counterpart of :meth:`_A2AClientMixin.a2a_discover_agent`."""
        resp = await self._request(
            "GET",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}/.well-known/agent.json",
            skip_auth=True,
        )
        _raise_for_status(resp)
        return resp.json()

    async def a2a_get_authenticated_extended_card(
        self,
        namespace: str,
        tenant: str,
    ) -> dict[str, Any]:
        """Async counterpart of
        :meth:`_A2AClientMixin.a2a_get_authenticated_extended_card`.
        """
        envelope = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "agent/getAuthenticatedExtendedCard",
        }
        resp = await self._request(
            "POST",
            f"/a2a/{_seg(namespace)}/{_seg(tenant)}",
            json=envelope,
            extra_headers=_A2A_HEADERS,
        )
        _raise_for_status(resp)
        return _unwrap_jsonrpc(resp.json())


# ---------------------------------------------------------------------
# JSON-RPC envelope helper (shared by sync + async paths)
# ---------------------------------------------------------------------


def _unwrap_jsonrpc(body: dict[str, Any]) -> dict[str, Any]:
    """Unwrap a JSON-RPC 2.0 reply envelope. Raises ``ApiError`` when
    the envelope carries an ``error`` member.
    """
    if "error" in body and body["error"] is not None:
        err = body["error"]
        raise ApiError(
            code=str(err.get("code", "JSONRPC")),
            message=err.get("message", "JSON-RPC error"),
            retryable=False,
        )
    result = body.get("result")
    if result is None:
        raise ApiError(
            code="JSONRPC",
            message="JSON-RPC reply had neither result nor error",
            retryable=False,
        )
    return result
