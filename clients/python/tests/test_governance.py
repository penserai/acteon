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
    assert asdict(client.governance("prod", "acme")) == FIXTURE["scope"]
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
        assert asdict(await client.governance("prod", "acme")) == FIXTURE["scope"]
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
