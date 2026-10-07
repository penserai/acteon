"""Native receipt/header contracts through the real HTTP client construction."""

import asyncio
import json
from pathlib import Path

import httpx
import pytest

from acteon_client import ActeonClient, AsyncActeonClient
from acteon_client.agent_services import AGENT_SOURCE_CONTEXT_HEADER
from acteon_client.errors import ActeonError, HttpError

FIXTURE = json.loads(
    (Path(__file__).parents[2] / "contract-fixtures/agent-services.json").read_text()
)


def handler(request):
    assert request.headers["authorization"] == "Bearer caller-key"
    assert request.headers["a2a-version"] == "1.0"
    if request.method == "POST":
        body = json.loads(request.content)
        index = int(body["message"]["messageId"][-1]) - 1
        job = FIXTURE["jobs"][index]
        assert AGENT_SOURCE_CONTEXT_HEADER not in request.headers
    else:
        assert not request.content
        index = int(request.url.path[-1]) - 1
        job = FIXTURE["jobs"][index]
        assert request.headers[AGENT_SOURCE_CONTEXT_HEADER] == job["source_context"]
    return httpx.Response(
        200,
        json=job["task"],
        headers={"a2a-version": "1.0", AGENT_SOURCE_CONTEXT_HEADER: job["source_context"]},
    )


def test_sync_receipts_keep_jobs_separate_and_original_identity():
    client = ActeonClient("http://acteon", api_key="caller-key")
    client._client = httpx.Client(transport=httpx.MockTransport(handler))
    try:
        first = client.agent_service_send_message("prod", "acme", "notifier", {"messageId": "m1"})
        second = client.agent_service_send_message("prod", "acme", "notifier", {"messageId": "m2"})
        first.task["id"] = "tampered-model-id"
        assert client.agent_service_get_task(first)["id"] == "job-1"
        assert client.agent_service_get_task(second)["id"] == "job-2"
        assert first.source_context == FIXTURE["jobs"][0]["source_context"]
        assert first.source_context not in repr(first)
    finally:
        client.close()


@pytest.mark.asyncio
async def test_async_concurrent_jobs_use_request_local_context():
    client = AsyncActeonClient("http://acteon", api_key="caller-key")
    client._client = httpx.AsyncClient(transport=httpx.MockTransport(handler))
    try:
        receipts = await asyncio.gather(
            *(
                client.agent_service_send_message(
                    "prod", "acme", "notifier", {"messageId": f"m{i}"}
                )
                for i in [1, 2]
            )
        )
        tasks = await asyncio.gather(
            *(client.agent_service_get_task(receipt) for receipt in reversed(receipts))
        )
        assert [task["id"] for task in tasks] == ["job-2", "job-1"]
    finally:
        await client.close()


@pytest.mark.parametrize("source", [None, "", "bad\r\nheader", "x" * 8193])
def test_missing_or_malformed_header_never_falls_back_to_task_metadata(source):
    calls = []

    def respond(request):
        calls.append(request)
        headers = {"a2a-version": "1.0"}
        if source is not None:
            headers[AGENT_SOURCE_CONTEXT_HEADER] = source
        return httpx.Response(200, json=FIXTURE["jobs"][0]["task"], headers=headers)

    client = ActeonClient("http://acteon")
    client._client = httpx.Client(transport=httpx.MockTransport(respond))
    try:
        with pytest.raises(ActeonError):
            client.agent_service_send_message("prod", "acme", "notifier", {})
        assert len(calls) == 1
    finally:
        client.close()


@pytest.mark.parametrize("status", FIXTURE["error_statuses"] + [307])
def test_http_errors_and_redirects_are_not_retried(status):
    calls = []

    def respond(request):
        calls.append(request)
        return httpx.Response(
            status,
            json={"error": "agent_services_unavailable"},
            headers={"location": "http://other/steal"},
        )

    client = ActeonClient("http://acteon")
    client._client = httpx.Client(transport=httpx.MockTransport(respond), follow_redirects=True)
    try:
        with pytest.raises(HttpError) as error:
            client.agent_service_send_message("prod", "acme", "notifier", {})
        assert error.value.status == status
        assert len(calls) == 1
    finally:
        client.close()
