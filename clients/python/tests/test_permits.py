import json
from pathlib import Path

import httpx
import pytest

from acteon_client import ActeonClient, Action, AsyncActeonClient, PermitReference
from acteon_client.errors import HttpError

FIXTURE = json.loads(
    (Path(__file__).parents[2] / "contract-fixtures/execution-permits.json").read_text()
)


def handler():
    seen = []

    def handle(req):
        assert req.headers["authorization"] == "Bearer test-key"
        if not seen:
            assert "x-acteon-execution-permits" not in req.headers
        else:
            assert json.loads(req.headers["x-acteon-execution-permits"]) == FIXTURE
        body = json.loads(req.content)
        assert (body[0] if isinstance(body, list) else body)["payload"] == {"incident": 42}
        seen.append(req.url.path)
        return httpx.Response(
            200, json=["Deduplicated"] if isinstance(body, list) else "Deduplicated"
        )

    return handle, seen


def test_sync_permit_headers_preserve_auth_and_legacy_calls():
    handle, seen = handler()
    action = Action("prod", "acme", "incident", "execute", {"incident": 42})
    with ActeonClient("http://localhost", api_key="test-key") as client:
        client._client.close()
        client._client = httpx.Client(transport=httpx.MockTransport(handle))
        client.dispatch(action)
        client.dispatch(action, permits=[PermitReference(**p) for p in FIXTURE])
        client.dispatch_batch([action], permits=[PermitReference(**p) for p in FIXTURE])
    assert len(seen) == 3


@pytest.mark.asyncio
async def test_async_permit_headers_preserve_auth_and_legacy_calls():
    handle, seen = handler()
    action = Action("prod", "acme", "incident", "execute", {"incident": 42})
    async with AsyncActeonClient("http://localhost", api_key="test-key") as client:
        await client._client.aclose()
        client._client = httpx.AsyncClient(transport=httpx.MockTransport(handle))
        await client.dispatch(action)
        await client.dispatch(action, permits=[PermitReference(**p) for p in FIXTURE])
        await client.dispatch_batch([action], permits=[PermitReference(**p) for p in FIXTURE])
    assert len(seen) == 3


@pytest.mark.parametrize("status", [400, 403, 409])
def test_dispatch_refusals_preserve_http_status(status):
    def handle(req):
        return httpx.Response(status, json={"error": "permit refused"})

    action = Action("prod", "acme", "incident", "execute", {})
    with ActeonClient("http://localhost") as client:
        client._client.close()
        client._client = httpx.Client(transport=httpx.MockTransport(handle))
        for call in [
            lambda: client.dispatch(action, permits=[]),
            lambda: client.dispatch_batch([action], permits=[]),
        ]:
            with pytest.raises(HttpError) as error:
                call()
            assert error.value.status == status


@pytest.mark.asyncio
async def test_async_dispatch_refusal_preserves_http_status():
    action = Action("prod", "acme", "incident", "execute", {})
    async with AsyncActeonClient("http://localhost") as client:
        await client._client.aclose()
        client._client = httpx.AsyncClient(
            transport=httpx.MockTransport(
                lambda req: httpx.Response(403, json={"error": "permit refused"})
            )
        )
        with pytest.raises(HttpError) as error:
            await client.dispatch(action, permits=[])
        assert error.value.status == 403
        with pytest.raises(HttpError):
            await client.dispatch_batch([action], permits=[])
