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
