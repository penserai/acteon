"""Check the complete catalog through real client request construction."""

import json
from pathlib import Path
from urllib.parse import quote

import httpx
import pytest

from acteon_client import ActeonClient, AsyncActeonClient, PlatformOperation
from acteon_client.bus_models import BusSubscription, CreateBusSubscription
from acteon_client.errors import HttpError
from acteon_client.platform_catalog import OPERATIONS


def test_catalog_matches_registered_route_fixture():
    fixture = json.loads(
        (Path(__file__).parents[2] / "contract-fixtures/platform-api.json").read_text()
    )
    assert {o["name"] for o in fixture["operations"] if o["response"] != "stream"} == {
        o.value for o in PlatformOperation
    }


def test_all_operations_auth_body_query_and_envelopes():
    for operation, (method, template, parameters, kind) in OPERATIONS.items():
        path = {key: "team/child ?#%" for key in parameters}
        expected = template
        for key, value in path.items():
            expected = expected.replace("{" + key + "}", quote(value, safe=""))

        def handle(req):
            assert req.method == method
            assert req.url.raw_path.decode().split("?")[0] == expected
            assert req.headers["Authorization"] == "Bearer local-test"
            assert req.url.params.get_list("filter") == ["a b", "c&d"]
            if method != "GET":
                assert json.loads(req.content) == {"request_id": "stable", "payload": {"n": 7}}
            return (
                httpx.Response(200, text="metric 1\n")
                if kind == "text"
                else httpx.Response(200, json={"opaque": [1, None]})
            )

        with ActeonClient("http://localhost", api_key="local-test") as client:
            client._client.close()
            client._client = httpx.Client(transport=httpx.MockTransport(handle))
            result = client.platform_request(
                operation,
                path=path,
                query={"filter": ["a b", "c&d"]},
                body=None if method == "GET" else {"request_id": "stable", "payload": {"n": 7}},
            )
            assert result == ("metric 1\n" if kind == "text" else {"opaque": [1, None]})


def test_error_empty_response_and_invalid_paths():
    with ActeonClient("http://localhost") as client:
        client._client.close()
        for status in (204, 403, 409, 429, 503):
            client._client = httpx.Client(
                transport=httpx.MockTransport(
                    lambda req: httpx.Response(status, text="conflict" if status != 204 else "")
                )
            )
            if status == 204:
                assert client.platform_request(PlatformOperation.AUTH_LOGOUT) is None
            else:
                with pytest.raises(HttpError) as error:
                    client.platform_request(PlatformOperation.AUTH_LOGOUT)
                assert error.value.status == status
            client._client.close()
        for path in (
            {},
            {"namespace": "n", "tenant": "t", "id": ".."},
            {"namespace": "n", "tenant": "t", "id": "s", "extra": "x"},
        ):
            with pytest.raises(ValueError):
                client.platform_request(PlatformOperation.BUS_STAGES_STATUS, path=path)


@pytest.mark.asyncio
async def test_async_sessions_keep_receipt_capabilities():
    async with AsyncActeonClient("http://localhost", api_key="local-test") as client:
        await client._client.aclose()

        def handle(req):
            assert req.method == "POST"
            assert json.loads(req.content) == {"receipt_ids": ["opaque-receipt"]}
            return httpx.Response(200, json={"remaining_in_flight": 0})

        client._client = httpx.AsyncClient(transport=httpx.MockTransport(handle))
        result = await client.platform_request(
            PlatformOperation.BUS_SESSIONS_ACK,
            path={"namespace": "n", "tenant": "t", "id": "sub", "session": "session"},
            body={"receipt_ids": ["opaque-receipt"]},
        )
        assert result["remaining_in_flight"] == 0


def test_receipt_required_subscription_roundtrip():
    req = CreateBusSubscription("s", "n.t.logs", "n", "t", receipt_required=True)
    assert req.to_dict()["receipt_required"] is True
    wire = {
        **req.to_dict(),
        "starting_offset": "earliest",
        "ack_mode": "manual",
        "ack_timeout_ms": 30000,
        "created_at": "now",
        "updated_at": "now",
        "consumer_group": "scoped-group",
    }
    sub = BusSubscription.from_dict(wire)
    assert sub.receipt_required and sub.consumer_group == "scoped-group"


def test_governance_outcomes_and_batch_preserve_all_fields():
    from acteon_client.models import ActionOutcome, BatchResult

    fixtures = json.loads(
        (Path(__file__).parents[2] / "contract-fixtures/dispatch-outcomes.json").read_text()
    )
    for fixture in fixtures:
        outcome = ActionOutcome.from_dict(fixture["wire"])
        assert outcome.outcome_type == fixture["type"]
        assert not outcome.is_executed()
        assert not outcome.is_failed()
        batch = BatchResult.from_dict(fixture["wire"])
        assert batch.success and batch.outcome.outcome_type == fixture["type"]
        if isinstance(fixture["wire"], dict):
            for key, value in next(iter(fixture["wire"].values())).items():
                assert getattr(outcome, key) == value


def test_typed_bus_lookup_and_delete_use_registered_routes():
    topic = {
        "name": "logs",
        "namespace": "n",
        "tenant": "t",
        "kafka_name": "actual.logs",
        "partitions": 1,
        "replication_factor": 1,
        "created_at": "now",
        "updated_at": "now",
    }
    seen = []

    def handler(request):
        seen.append((request.method, request.url.path))
        if request.method == "GET":
            assert request.url.params["namespace"] == "n"
            assert request.url.params["tenant"] == "t"
            return httpx.Response(200, json={"topics": [{**topic, "tenant": "other"}, topic]})
        return httpx.Response(204)

    with ActeonClient("http://localhost") as client:
        client._client.close()
        client._client = httpx.Client(transport=httpx.MockTransport(handler))
        assert client.get_bus_topic("n", "t", "logs").tenant == "t"
        client.delete_bus_topic("n", "t", "logs")
    assert seen == [
        ("GET", "/v1/bus/topics"),
        ("GET", "/v1/bus/topics"),
        ("DELETE", "/v1/bus/topics/actual.logs"),
    ]
