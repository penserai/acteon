import json
from dataclasses import asdict
from pathlib import Path

import httpx
import pytest

from acteon_client import (
    ActeonClient,
    AsyncActeonClient,
    GovernanceInterventionRequest,
    GovernanceLimits,
    GovernancePermitDeclaration,
    GovernanceResource,
    GovernanceResourceChange,
    GovernanceRoute,
    PrincipalIdentity,
    PublishGovernancePermitRequest,
)
from acteon_client.errors import HttpError

FIXTURE = json.loads(
    (Path(__file__).parents[2] / "contract-fixtures/governance-management.json").read_text()
)


def requests():
    p = FIXTURE["publication"]
    permit = p["permit"]
    publication = PublishGovernancePermitRequest(
        p["namespace"],
        p["tenant"],
        p["change_id"],
        p["expected_revision"],
        GovernancePermitDeclaration(
            permit["id"],
            permit["revision"],
            PrincipalIdentity(**permit["subject"]),
            [GovernanceRoute(**r) for r in permit["routes"]],
            permit["valid_from_ms"],
            GovernanceLimits(**permit["limits"]),
        ),
        p["reason"],
    )
    c = FIXTURE["intervention"]
    intervention = GovernanceInterventionRequest(
        c["namespace"],
        c["tenant"],
        c["change_id"],
        GovernanceResourceChange(
            c["change"]["kind"], GovernanceResource(**c["change"]["resource"])
        ),
        c["reason"],
    )
    return publication, intervention


def handler(seen):
    def respond(request):
        seen.append(request)
        return httpx.Response(
            200, json=FIXTURE["scope"] if request.method == "GET" else FIXTURE["receipt"]
        )

    return respond


def assert_wire(seen):
    assert [r.method for r in seen] == ["GET", "POST", "POST"]
    assert dict(seen[0].url.params) == {"namespace": "prod", "tenant": "acme"}
    assert json.loads(seen[1].content) == FIXTURE["publication"]
    assert json.loads(seen[2].content) == FIXTURE["intervention"]
    assert all(r.headers["authorization"] == "Bearer operator-key" for r in seen)


def test_sync_governance():
    seen = []
    client = ActeonClient("http://example.test", api_key="operator-key")
    client._client = httpx.Client(transport=httpx.MockTransport(handler(seen)))
    publication, intervention = requests()
    assert asdict(client.governance("prod", "acme")) == {
        **FIXTURE["scope"],
        "management": {
            **FIXTURE["scope"]["management"],
            "can_read_history": False,
            "can_reconcile": False,
        },
    }
    assert asdict(client.publish_governance_permit(publication)) == FIXTURE["receipt"]
    assert asdict(client.intervene_governance(intervention)) == FIXTURE["receipt"]
    client.close()
    assert_wire(seen)


@pytest.mark.asyncio
async def test_async_governance():
    seen = []
    async with AsyncActeonClient("http://example.test", api_key="operator-key") as client:
        await client._client.aclose()
        client._client = httpx.AsyncClient(transport=httpx.MockTransport(handler(seen)))
        publication, intervention = requests()
        assert asdict(await client.governance("prod", "acme")) == {
            **FIXTURE["scope"],
            "management": {
                **FIXTURE["scope"]["management"],
                "can_read_history": False,
                "can_reconcile": False,
            },
        }
        assert asdict(await client.publish_governance_permit(publication)) == FIXTURE["receipt"]
        assert asdict(await client.intervene_governance(intervention)) == FIXTURE["receipt"]
    assert_wire(seen)


@pytest.mark.parametrize("status", [401, 403, 409, 503])
def test_refusal_preserves_status_without_retry(status):
    seen = []

    def fail(request):
        seen.append(request)
        return httpx.Response(status, json={"error": "governance_denied"})

    client = ActeonClient("http://example.test")
    client._client = httpx.Client(transport=httpx.MockTransport(fail))
    with pytest.raises(HttpError) as error:
        client.intervene_governance(requests()[1])
    assert error.value.status == status
    assert len(seen) == 1
    client.close()


@pytest.mark.parametrize("permission", [False, True])
def test_explicit_history_capability(permission):
    from acteon_client.governance import GovernanceManagementBounds

    data = {**FIXTURE["scope"]["management"], "can_read_history": permission}
    assert GovernanceManagementBounds.from_dict(data).can_read_history is permission
    assert not GovernanceManagementBounds.from_dict(FIXTURE["scope"]["management"]).can_read_history


HISTORIES = json.loads(
    (Path(__file__).parents[2] / "contract-fixtures/provider-history.json").read_text()
)


def history_transport():
    remaining = iter(HISTORIES)

    def handler(request):
        assert request.method == "GET"
        assert (
            request.url.path
            == "/v1/governance/executions/" + HISTORIES[0]["receipt"]["execution_id"]
        )
        assert dict(request.url.params) == {"namespace": "prod", "tenant": "acme"}
        assert request.headers["authorization"] == "Bearer operator-key"
        return httpx.Response(200, json=next(remaining))

    return httpx.MockTransport(handler)


def assert_history(history, wire):
    assert history.subject.id == "agent/maya"
    assert history.receipt.status.state == wire["receipt"]["status"]["state"]
    assert (
        history.metadata is None if wire["metadata"] is None else history.metadata.max_attempts == 3
    )
    if history.receipt.status.state == "completed":
        assert history.receipt.status.outcome.outcome_type == "executed"
        assert history.attempts[0].original_outcome.outcome_type == "failed"
        assert history.attempts[0].reconciliation.outcome.outcome_type == "executed"
        assert history.attempts[0].original_evidence.id == "original-result"
        assert history.attempts[0].reconciliation.resolution.id == "resolution"
        acceptance = history.attempts[0].reconciliation.acceptance
        expected = wire["attempts"][0]["reconciliation"].get("acceptance")
        if expected is None:
            assert acceptance is None
        else:
            assert acceptance.operator.id == expected["operator"]["id"]
            assert acceptance.authority.generation == expected["authority"]["generation"]
            assert acceptance.accepted_at_ms == expected["accepted_at_ms"]


def test_provider_history_sync():
    with ActeonClient("https://acteon.example", api_key="operator-key") as client:
        client._client.close()
        client._client = httpx.Client(transport=history_transport())
        for wire in HISTORIES:
            assert_history(
                client.provider_execution_history("prod", "acme", wire["receipt"]["execution_id"]),
                wire,
            )


@pytest.mark.asyncio
async def test_provider_history_async():
    async with AsyncActeonClient("https://acteon.example", api_key="operator-key") as client:
        await client._client.aclose()
        client._client = httpx.AsyncClient(transport=history_transport())
        for wire in HISTORIES:
            assert_history(
                await client.provider_execution_history(
                    "prod", "acme", wire["receipt"]["execution_id"]
                ),
                wire,
            )


@pytest.mark.parametrize("status", [401, 403, 404, 409, 503])
def test_history_refusal_preserves_status_without_retry(status):
    seen = []

    def fail(request):
        seen.append(request)
        return httpx.Response(status, json={"error": "history_denied"})

    with ActeonClient("https://acteon.example") as client:
        client._client.close()
        client._client = httpx.Client(transport=httpx.MockTransport(fail))
        with pytest.raises(HttpError) as error:
            client.provider_execution_history(
                "prod", "acme", HISTORIES[0]["receipt"]["execution_id"]
            )
        assert error.value.status == status
        assert len(seen) == 1


@pytest.mark.parametrize("permission", [False, True])
def test_reconciliation_capability_is_independent_and_defaults_denied(permission):
    from acteon_client.governance import GovernanceManagementBounds

    data = {**FIXTURE["scope"]["management"], "can_reconcile": permission}
    parsed = GovernanceManagementBounds.from_dict(data)
    assert parsed.can_reconcile is permission
    assert not parsed.can_read_history
    assert not GovernanceManagementBounds.from_dict(FIXTURE["scope"]["management"]).can_reconcile


FINALITY = json.loads(
    (Path(__file__).parents[2] / "contract-fixtures/provider-reconciliation.json").read_text()
)


def finality_transport():
    responses = iter([FINALITY["correlation"], FINALITY["receipt"], FINALITY["no_effect_receipt"]])
    calls = []

    def handler(request):
        suffix = "correlation" if not calls else "reconciliation"
        execution_id = FINALITY["correlation"]["context"]["execution_id"]
        assert request.url.path == f"/v1/governance/executions/{execution_id}/attempts/0/{suffix}"
        assert dict(request.url.params) == {"namespace": "prod", "tenant": "acme"}
        assert request.headers["authorization"] == "Bearer operator-key"
        assert request.method == ("GET" if not calls else "POST")
        if calls:
            assert json.loads(request.content) == FINALITY["request"]
        calls.append(request)
        return httpx.Response(200, json=next(responses))

    return httpx.MockTransport(handler), calls


def test_typed_reconciliation_transport_and_both_nested_outcomes():
    from acteon_client import ProviderReconciliationRequest

    transport, calls = finality_transport()
    client = ActeonClient("https://acteon.example", api_key="operator-key")
    client._client = httpx.Client(transport=transport)
    execution_id = FINALITY["correlation"]["context"]["execution_id"]
    result = client.provider_reconciliation_correlation("prod", "acme", execution_id, 0)
    assert asdict(result) == FINALITY["correlation"]
    request = ProviderReconciliationRequest(**FINALITY["request"])
    for expected in ["executed", "failed"]:
        result = client.accept_provider_reconciliation("prod", "acme", execution_id, 0, request)
        assert result.status.outcome.outcome_type == expected
    client.close()
    assert len(calls) == 3


@pytest.mark.asyncio
async def test_async_typed_reconciliation_transport_and_both_nested_outcomes():
    from acteon_client import ProviderReconciliationRequest

    transport, calls = finality_transport()
    async with AsyncActeonClient("https://acteon.example", api_key="operator-key") as client:
        await client._client.aclose()
        client._client = httpx.AsyncClient(transport=transport)
        execution_id = FINALITY["correlation"]["context"]["execution_id"]
        result = await client.provider_reconciliation_correlation("prod", "acme", execution_id, 0)
        assert asdict(result) == FINALITY["correlation"]
        request = ProviderReconciliationRequest(**FINALITY["request"])
        for expected in ["executed", "failed"]:
            result = await client.accept_provider_reconciliation(
                "prod", "acme", execution_id, 0, request
            )
            assert result.status.outcome.outcome_type == expected
    assert len(calls) == 3


REGISTRY = json.loads(
    (Path(__file__).parents[2] / "contract-fixtures/governance-registry.json").read_text()
)


@pytest.mark.parametrize("async_client", [False, True])
@pytest.mark.asyncio
async def test_registry_requests_receipts_and_refusals(async_client):
    from acteon_client import GovernanceRegistryMutationRequest

    request = GovernanceRegistryMutationRequest(**REGISTRY["request"])
    calls = []
    status = 200
    receipt = dict(REGISTRY["receipt"])
    view_response = dict(REGISTRY["view"])

    def handler(req):
        calls.append(req)
        assert req.headers["authorization"] == "Bearer operator-key"
        assert req.extensions.get("follow_redirects") is None
        if req.method == "GET":
            assert req.url.path == "/v1/governance/registry/maya"
            assert dict(req.url.params) == {
                "namespace": "prod",
                "tenant": "acme",
                "projection": "card",
            }
            return httpx.Response(200, json=view_response)
        assert json.loads(req.content) == REGISTRY["request"]
        return httpx.Response(status, json=receipt)

    transport = httpx.MockTransport(handler)
    cls = AsyncActeonClient if async_client else ActeonClient
    client = cls("http://example.test", api_key="operator-key")
    if async_client:
        await client._client.aclose()
        client._client = httpx.AsyncClient(transport=transport)
    else:
        client._client.close()
        client._client = httpx.Client(transport=transport)
    try:
        view_call = client.registry_projection("prod", "acme", "maya", "card")
        view = await view_call if async_client else view_call
        assert asdict(view) == REGISTRY["view"]
        for field, invalid in [
            ("tenant", "other"),
            ("version", 0),
            ("qualification_retired", None),
            ("registry_revision", 0),
            ("value", "invalid"),
        ]:
            view_response = {**REGISTRY["view"], field: invalid}
            before = len(calls)
            with pytest.raises(ValueError):
                if async_client:
                    await client.registry_projection("prod", "acme", "maya", "card")
                else:
                    client.registry_projection("prod", "acme", "maya", "card")
            assert len(calls) == before + 1
        for _ in range(2):
            result_call = client.mutate_registry(request)
            result = await result_call if async_client else result_call
            assert asdict(result) == REGISTRY["receipt"]
        for field, invalid in [
            ("delivery_complete", False),
            ("applied", False),
            ("change_id", "other"),
            ("tenant", "other"),
            ("input_digest", "bad"),
            ("delivery_complete", "true"),
        ]:
            receipt = {**REGISTRY["receipt"], field: invalid}
            before = len(calls)
            with pytest.raises(ValueError):
                if async_client:
                    await client.mutate_registry(request)
                else:
                    client.mutate_registry(request)
            assert len(calls) == before + 1
        for status in [401, 403, 409, 503, 307]:
            before = len(calls)
            with pytest.raises(HttpError) as error:
                if async_client:
                    await client.mutate_registry(request)
                else:
                    client.mutate_registry(request)
            assert error.value.status == status
            assert len(calls) == before + 1
    finally:
        if async_client:
            await client.close()
        else:
            client.close()


@pytest.mark.parametrize("async_client", [False, True])
@pytest.mark.asyncio
async def test_registry_receipt_checks_sent_identity_when_caller_mutates_request(async_client):
    from acteon_client import GovernanceRegistryMutationRequest

    request = GovernanceRegistryMutationRequest(**REGISTRY["request"])

    def handler(req):
        assert json.loads(req.content) == REGISTRY["request"]
        request.tenant = "caller-changed-after-send"
        return httpx.Response(200, json=REGISTRY["receipt"])

    if async_client:
        async with AsyncActeonClient("http://example.test") as client:
            await client._client.aclose()
            client._client = httpx.AsyncClient(transport=httpx.MockTransport(handler))
            assert asdict(await client.mutate_registry(request)) == REGISTRY["receipt"]
    else:
        with ActeonClient("http://example.test") as client:
            client._client.close()
            client._client = httpx.Client(transport=httpx.MockTransport(handler))
            assert asdict(client.mutate_registry(request)) == REGISTRY["receipt"]
