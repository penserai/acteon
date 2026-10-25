"""Transport fixture for helpers that verify server-owned response context."""

import json
from pathlib import Path

import httpx

IDENTITY = json.loads((Path(__file__).parents[2] / "contract-fixtures/identity.json").read_text())[
    0
]


def credential_transport(handler):
    def respond(request):
        if request.url.path == "/v1/auth/identity":
            assert request.headers.get("authorization") or request.headers.get("x-api-key")
            assert not request.url.query
            return httpx.Response(200, json=IDENTITY)
        return handler(request)

    return httpx.MockTransport(respond)
