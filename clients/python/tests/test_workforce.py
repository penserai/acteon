"""All workforce variants share one native-SDK wire and refusal contract."""

import json
from dataclasses import asdict
from pathlib import Path

import httpx
import pytest

from acteon_client import ActeonClient, AsyncActeonClient
from acteon_client.errors import HttpError
from acteon_client.workforce import WorkforceChangeRequest, WorkforceScopeView

FIXTURE = json.loads(
    (Path(__file__).parents[2] / "contract-fixtures/workforce-management.json").read_text()
)


def test_workforce_models_round_trip_all_variants():
    assert asdict(WorkforceScopeView.from_dict(FIXTURE["scope"])) == FIXTURE["scope"]
    for wire in FIXTURE["changes"]:
        assert asdict(WorkforceChangeRequest.from_dict(wire)) == wire


@pytest.mark.parametrize("asynchronous", [False, True])
@pytest.mark.asyncio
async def test_workforce_transport_and_refusals(asynchronous):
    calls = []
    status = 200

    def handler(request):
        assert request.headers["authorization"] == "Bearer operator-key"
        calls.append(request)
        if request.method == "GET":
            assert request.url.path == "/v1/workforce"
            assert dict(request.url.params) == {"namespace": "prod", "tenant": "acme"}
            return httpx.Response(status, json=FIXTURE["scope"])
        assert request.url.path == "/v1/workforce/changes"
        return httpx.Response(
            status, json=FIXTURE["receipt"] if status == 200 else {"error": "denied"}
        )

    transport = httpx.MockTransport(handler)
    if asynchronous:
        client = AsyncActeonClient("http://city", api_key="operator-key")
        await client._client.aclose()
        client._client = httpx.AsyncClient(
            base_url="http://city",
            transport=transport,
            headers={"Authorization": "Bearer operator-key"},
        )
        scope = await client.workforce("prod", "acme")
    else:
        client = ActeonClient("http://city", api_key="operator-key")
        client._client.close()
        client._client = httpx.Client(
            base_url="http://city",
            transport=transport,
            headers={"Authorization": "Bearer operator-key"},
        )
        scope = client.workforce("prod", "acme")
    assert asdict(scope) == FIXTURE["scope"]
    for wire in FIXTURE["changes"]:
        request = WorkforceChangeRequest.from_dict(wire)
        receipt = (
            await client.change_workforce(request)
            if asynchronous
            else client.change_workforce(request)
        )
        assert asdict(receipt) == FIXTURE["receipt"]
        assert json.loads(calls[-1].content) == wire
    for status in [401, 403, 409, 503]:
        count = len(calls)
        with pytest.raises(HttpError) as error:
            if asynchronous:
                await client.change_workforce(request)
            else:
                client.change_workforce(request)
        assert error.value.status == status
        assert len(calls) == count + 1
    if asynchronous:
        await client.close()
    else:
        client.close()
