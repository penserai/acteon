"""Native receipt/header contracts through the real HTTP client construction."""

import asyncio
import json
from pathlib import Path

import httpx
import pytest

from acteon_client import ActeonClient, AgentServiceParent, AgentServiceReceipt, AsyncActeonClient, PermitReference
from acteon_client.agent_services import AGENT_EXECUTION_CONTEXT_HEADER, AGENT_SOURCE_CONTEXT_HEADER, _provider_abort
from acteon_client.errors import ActeonError, HttpError

FIXTURE = json.loads(
    (Path(__file__).parents[2] / "contract-fixtures/agent-services.json").read_text()
)


def test_parent_context_and_permits_are_request_local():
    calls = []

    def respond(request):
        calls.append(request)
        assert request.headers[AGENT_EXECUTION_CONTEXT_HEADER] == FIXTURE["parent"]["execution_context"]
        assert json.loads(request.headers["x-acteon-execution-permits"]) == FIXTURE["parent"]["permits"]
        assert AGENT_SOURCE_CONTEXT_HEADER not in request.headers
        job = FIXTURE["jobs"][0]
        return httpx.Response(200, json=job["task"], headers={"a2a-version": "1.0", AGENT_SOURCE_CONTEXT_HEADER: job["source_context"]})

    permit = FIXTURE["parent"]["permits"][0]
    parent = AgentServiceParent(FIXTURE["parent"]["execution_context"], (PermitReference(permit["id"], permit["accepted_revision"]),))
    client = ActeonClient("http://acteon")
    client._client = httpx.Client(transport=httpx.MockTransport(respond))
    try:
        client.agent_service_send_message("prod", "acme", "notifier", {}, parent=parent)
        assert len(calls) == 1
    finally:
        client.close()


def test_peer_tool_sends_no_authority_fields_and_validates_receipt():
    source = AgentServiceReceipt(
        "prod", "acme", "notifier", "job-1", FIXTURE["jobs"][0]["source_context"], FIXTURE["jobs"][0]["task"]
    )

    def respond(request):
        assert "/tasks/job-1/peers/team/resolver/diagnose/" in request.url.path
        assert AGENT_SOURCE_CONTEXT_HEADER not in request.headers
        assert AGENT_EXECUTION_CONTEXT_HEADER not in request.headers
        assert "x-acteon-execution-permits" not in request.headers
        if request.url.path.endswith(":refresh"):
            assert not request.content
        else:
            assert json.loads(request.content) == {"message": {"messageId": "peer-1"}}
        return httpx.Response(
            200,
            json={
                "submission_id": "f47ac10b-58cc-5372-a567-0e02b2c3d479",
                "status": {"state": "accepted", "task": FIXTURE["jobs"][1]["task"]},
            },
            headers={"a2a-version": "1.0"},
        )

    client = ActeonClient("http://acteon")
    client._client = httpx.Client(transport=httpx.MockTransport(respond))
    try:
        receipt = client.agent_service_send_peer(
            source, "team/resolver", "diagnose", {"messageId": "peer-1"}
        )
        assert receipt.state == "accepted"
        assert receipt.task == FIXTURE["jobs"][1]["task"]
        refreshed = client.agent_service_refresh_peer(
            source, "team/resolver", "diagnose", receipt
        )
        assert refreshed.submission_id == receipt.submission_id
    finally:
        client.close()


def handler(request):
    assert request.headers["authorization"] == "Bearer caller-key"
    assert request.headers["a2a-version"] == "1.0"
    if request.url.path.endswith("/stop"):
        assert not request.content
        index = int(request.url.path.split("/")[-2][-1]) - 1
        job = FIXTURE["jobs"][index]
        assert request.headers[AGENT_SOURCE_CONTEXT_HEADER] == job["source_context"]
        return httpx.Response(200, json=job["stop_response"], headers={"a2a-version": "1.0"})
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
        stopped_receipts = []
        for receipt in [second, first]:
            stopped = client.agent_service_stop_task(receipt)
            stopped_receipts.append(stopped)
            assert stopped.future_starts_blocked is True
            assert stopped.task["id"] == receipt.task_id
            assert stopped.task["status"]["state"] == "submitted"
        assert [r.provider_abort.state for r in stopped_receipts if r.provider_abort] == [
            "uncertain",
            "restricted_only",
        ]
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
        stopped = await asyncio.gather(
            *(client.agent_service_stop_task(r) for r in reversed(receipts))
        )
        assert [r.task["id"] for r in stopped] == ["job-2", "job-1"]
        assert all(r.future_starts_blocked for r in stopped)
        assert [r.provider_abort.state for r in stopped if r.provider_abort] == [
            "uncertain",
            "restricted_only",
        ]
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


@pytest.mark.parametrize("value", [False, "true", 1, None])
def test_stop_rejects_non_acknowledgements_with_one_request(value):
    calls = []

    def respond(request):
        calls.append(request)
        return httpx.Response(
            200,
            json={"task": FIXTURE["jobs"][0]["task"], "future_starts_blocked": value},
            headers={"a2a-version": "1.0"},
        )

    job = FIXTURE["jobs"][0]
    receipt = AgentServiceReceipt(
        "prod", "acme", "notifier", "job-1", job["source_context"], job["task"]
    )
    client = ActeonClient("http://acteon")
    client._client = httpx.Client(transport=httpx.MockTransport(respond))
    try:
        with pytest.raises(ActeonError):
            client.agent_service_stop_task(receipt)
        assert len(calls) == 1
    finally:
        client.close()


def test_stop_rejects_malformed_provider_abort_with_one_request():
    calls = []

    def respond(request):
        calls.append(request)
        return httpx.Response(
            200,
            json={
                "task": FIXTURE["jobs"][0]["task"],
                "future_starts_blocked": True,
                "provider_abort": {"state": "reconciled", "proof_digest": "bad"},
            },
            headers={"a2a-version": "1.0"},
        )

    job = FIXTURE["jobs"][0]
    receipt = AgentServiceReceipt(
        "prod", "acme", "notifier", "job-1", job["source_context"], job["task"]
    )
    client = ActeonClient("http://acteon")
    client._client = httpx.Client(transport=httpx.MockTransport(respond))
    try:
        with pytest.raises(ActeonError):
            client.agent_service_stop_task(receipt)
        assert len(calls) == 1
    finally:
        client.close()


@pytest.mark.parametrize(
    "attempt_id",
    [
        "F47AC10B-58CC-5372-A567-0E02B2C3D479",
        "f47ac10b-58cc-4372-a567-0e02b2c3d479",
    ],
)
def test_provider_abort_requires_canonical_uuid_v5(attempt_id):
    with pytest.raises(ActeonError):
        _provider_abort({"state": "uncertain", "attempt_id": attempt_id})


@pytest.mark.parametrize("status", FIXTURE["error_statuses"] + [307])
def test_stop_failed_acknowledgements_preserve_status_and_never_retry(status):
    calls = []

    def respond(request):
        calls.append(request)
        return httpx.Response(
            status, json={"error": "denied"}, headers={"location": "http://other/steal"}
        )

    job = FIXTURE["jobs"][0]
    receipt = AgentServiceReceipt(
        "prod", "acme", "notifier", "job-1", job["source_context"], job["task"]
    )
    client = ActeonClient("http://acteon")
    client._client = httpx.Client(transport=httpx.MockTransport(respond), follow_redirects=True)
    try:
        with pytest.raises(HttpError) as error:
            client.agent_service_stop_task(receipt)
        assert error.value.status == status
        assert len(calls) == 1
    finally:
        client.close()


@pytest.mark.parametrize("field,value", [("id", "foreign-job"), ("tenant", "other-tenant")])
def test_stop_rejects_foreign_task_identity(field, value):
    calls = []
    job = FIXTURE["jobs"][0]
    receipt = AgentServiceReceipt(
        "prod", "acme", "notifier", "job-1", job["source_context"], job["task"]
    )

    def respond(request):
        calls.append(request)
        task = {**job["task"], field: value}
        return httpx.Response(
            200, json={"task": task, "future_starts_blocked": True}, headers={"a2a-version": "1.0"}
        )

    client = ActeonClient("http://acteon")
    client._client = httpx.Client(transport=httpx.MockTransport(respond))
    try:
        with pytest.raises(ActeonError, match="identity mismatch"):
            client.agent_service_stop_task(receipt)
        assert len(calls) == 1
    finally:
        client.close()
