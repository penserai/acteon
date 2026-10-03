"""Complete finite HTTP API access using server wire names and response envelopes.

No operation is retried automatically. In particular, preserve caller-generated
request IDs for session opens, stage controls, and quarantine repair retries.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any
from urllib.parse import quote

from .errors import HttpError
from .platform_catalog import OPERATIONS, PlatformOperation

if TYPE_CHECKING:
    import httpx


def platform_path(
    operation: PlatformOperation, path: dict[str, str] | None
) -> tuple[str, str, str]:
    method, template, parameters, response = OPERATIONS[operation]
    values = path or {}
    if set(values) != set(parameters):
        raise ValueError(f"Expected path parameters: {parameters}")
    for name, value in values.items():
        if not value or value in (".", ".."):
            raise ValueError("Empty and dot path segments are not allowed")
        template = template.replace("{" + name + "}", quote(value, safe=""))
    return method, template, response


def platform_response(response: httpx.Response, kind: str) -> Any:
    if not 200 <= response.status_code < 300:
        raise HttpError(response.status_code, response.text)
    if response.status_code == 204:
        return None
    return response.text if kind == "text" else response.json()


class _PlatformMixin:
    if TYPE_CHECKING:

        def _request(
            self, method: str, path: str, *, json: Any = None, params: dict[str, Any] | None = None
        ) -> httpx.Response: ...

    def platform_request(
        self,
        operation: PlatformOperation,
        *,
        path: dict[str, str] | None = None,
        query: dict[str, Any] | None = None,
        body: Any = None,
    ) -> Any:
        """Call a registered operation, preserving snake_case wire fields.

        JSON responses retain their server envelope. Text endpoints return str;
        HTTP 204 returns None. HTTP errors retain their status code.
        """
        method, url, kind = platform_path(operation, path)
        if method == "GET" and body is not None:
            raise ValueError("GET operations do not accept a body")
        return platform_response(self._request(method, url, params=query, json=body), kind)


class _AsyncPlatformMixin:
    if TYPE_CHECKING:

        async def _request(
            self, method: str, path: str, *, json: Any = None, params: dict[str, Any] | None = None
        ) -> httpx.Response: ...

    async def platform_request(
        self,
        operation: PlatformOperation,
        *,
        path: dict[str, str] | None = None,
        query: dict[str, Any] | None = None,
        body: Any = None,
    ) -> Any:
        """Async counterpart of ActeonClient.platform_request; never auto-retries."""
        method, url, kind = platform_path(operation, path)
        if method == "GET" and body is not None:
            raise ValueError("GET operations do not accept a body")
        return platform_response(await self._request(method, url, params=query, json=body), kind)
