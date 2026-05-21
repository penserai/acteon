"""A2A Python SDK — factory + URL/header smoke.

Live HTTP tests would need a running Acteon server with A2A
enabled; instead these tests pin the wire surface of the new
``a2a.py`` module: the factory helpers produce the dict shapes the
server expects, and the mixin's per-method URL + header behaviour
is observable from a fake ``_request`` capture.
"""

import unittest
from typing import Any, Optional

from acteon_client import (
    A2A_PROTOCOL_VERSION,
    make_message,
    make_part_data,
    make_part_text,
    make_part_url,
    make_push_config,
)
from acteon_client.a2a import _A2AClientMixin


class _Captured:
    """One captured ``_request`` call."""

    def __init__(
        self,
        method: str,
        path: str,
        json: Optional[dict],
        params: Optional[dict],
        extra_headers: Optional[dict],
        skip_auth: bool,
    ):
        self.method = method
        self.path = path
        self.json = json
        self.params = params
        self.extra_headers = extra_headers
        self.skip_auth = skip_auth


class _FakeResponse:
    """Minimal ``httpx.Response`` stand-in covering only the bits the
    A2A mixin uses (``status_code``, ``json()``, ``text``)."""

    def __init__(self, status_code: int = 200, body: Any = None):
        self.status_code = status_code
        self._body = body if body is not None else {}
        self.text = ""

    def json(self):
        return self._body


class _StubClient(_A2AClientMixin):
    """Mixin host that records every ``_request`` call without
    hitting the network. Returns a canned body so the mixin's
    parsing code runs end-to-end.
    """

    def __init__(self, body: Any = None, status_code: int = 200):
        self.calls: list[_Captured] = []
        self._body = body if body is not None else {}
        self._status_code = status_code

    def _request(
        self,
        method: str,
        path: str,
        *,
        json: Optional[dict] = None,
        params: Optional[dict] = None,
        extra_headers: Optional[dict] = None,
        skip_auth: bool = False,
    ):
        self.calls.append(
            _Captured(method, path, json, params, extra_headers, skip_auth)
        )
        return _FakeResponse(status_code=self._status_code, body=self._body)


# ---------------------------------------------------------------------
# Factory helpers
# ---------------------------------------------------------------------


class TestFactories(unittest.TestCase):
    def test_make_part_text(self):
        self.assertEqual(make_part_text("hi"), {"text": "hi"})

    def test_make_part_url(self):
        self.assertEqual(
            make_part_url("https://x/y"),
            {"url": "https://x/y"},
        )

    def test_make_part_data_defaults_media_type_to_json(self):
        p = make_part_data({"k": 1})
        self.assertEqual(p["data"], {"k": 1})
        self.assertEqual(p["mediaType"], "application/json")

    def test_make_part_data_honors_custom_media_type(self):
        p = make_part_data({"k": 1}, media_type="application/cloudevents+json")
        self.assertEqual(p["mediaType"], "application/cloudevents+json")

    def test_make_message_minimal(self):
        msg = make_message("m-1", "user", [make_part_text("hi")])
        self.assertEqual(msg["messageId"], "m-1")
        self.assertEqual(msg["role"], "user")
        self.assertEqual(msg["parts"], [{"text": "hi"}])
        # Absent task_id / context_id must NOT appear in the dict —
        # the server treats absent vs. empty differently.
        self.assertNotIn("taskId", msg)
        self.assertNotIn("contextId", msg)

    def test_make_message_threads_taskid(self):
        msg = make_message(
            "m-2",
            "user",
            [make_part_text("yes")],
            task_id="task-alpha",
        )
        self.assertEqual(msg["taskId"], "task-alpha")

    def test_make_push_config_minimal(self):
        cfg = make_push_config("https://hook/x")
        self.assertEqual(cfg, {"url": "https://hook/x"})

    def test_make_push_config_full(self):
        cfg = make_push_config(
            "https://hook/x",
            id="cfg-1",
            token="t",
            authentication={"schemes": ["api-key"]},
        )
        self.assertEqual(cfg["id"], "cfg-1")
        self.assertEqual(cfg["token"], "t")
        self.assertEqual(cfg["authentication"], {"schemes": ["api-key"]})


# ---------------------------------------------------------------------
# Mixin URL + header behaviour
# ---------------------------------------------------------------------


class TestMixinUrlsAndHeaders(unittest.TestCase):
    def test_send_message_url_and_a2a_version_header(self):
        c = _StubClient(body={"id": "task-1", "status": {"state": "submitted"}})
        c.a2a_send_message("ns", "tnt", make_message("m-1", "user", [make_part_text("hi")]))
        self.assertEqual(len(c.calls), 1)
        call = c.calls[0]
        self.assertEqual(call.method, "POST")
        self.assertEqual(call.path, "/a2a/ns/tnt/v1/message:send")
        # The send is wrapped in {"message": ...} per spec.
        self.assertIn("message", call.json)
        self.assertEqual(call.json["message"]["messageId"], "m-1")
        # A2A-Version header is set on every authenticated call.
        self.assertEqual(call.extra_headers, {"A2A-Version": A2A_PROTOCOL_VERSION})
        self.assertFalse(call.skip_auth)

    def test_cancel_task_url_carries_cancel_verb_in_segment(self):
        c = _StubClient(body={"id": "task-1", "status": {"state": "canceled"}})
        c.a2a_cancel_task("ns", "tnt", "task-1")
        self.assertEqual(c.calls[0].method, "POST")
        # The :cancel verb must end up as part of the final segment,
        # not a separate path component.
        self.assertEqual(
            c.calls[0].path,
            "/a2a/ns/tnt/v1/tasks/task-1:cancel",
        )

    def test_delete_push_config_url(self):
        c = _StubClient(body=None, status_code=204)
        c.a2a_delete_push_config("ns", "tnt", "task-1", "cfg-a")
        self.assertEqual(c.calls[0].method, "DELETE")
        self.assertEqual(
            c.calls[0].path,
            "/a2a/ns/tnt/v1/tasks/task-1/pushNotificationConfigs/cfg-a",
        )

    def test_discover_agent_is_unauthenticated(self):
        c = _StubClient(body={"agent_id": "tenant"})
        c.a2a_discover_agent("ns", "tnt")
        call = c.calls[0]
        self.assertEqual(call.method, "GET")
        self.assertEqual(call.path, "/a2a/ns/tnt/.well-known/agent.json")
        # The discovery endpoint is unauthenticated per the A2A spec
        # — the mixin must request without the API-key header.
        self.assertTrue(
            call.skip_auth,
            "discovery must skip auth headers",
        )

    def test_extended_card_uses_jsonrpc_envelope(self):
        c = _StubClient(
            body={
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"agent_id": "tenant", "capabilities": {}},
            }
        )
        card = c.a2a_get_authenticated_extended_card("ns", "tnt")
        call = c.calls[0]
        self.assertEqual(call.path, "/a2a/ns/tnt")
        self.assertEqual(call.json["jsonrpc"], "2.0")
        self.assertEqual(call.json["method"], "agent/getAuthenticatedExtendedCard")
        # The mixin unwraps the JSON-RPC envelope on the way out.
        self.assertEqual(card, {"agent_id": "tenant", "capabilities": {}})

    def test_path_segments_are_percent_encoded(self):
        c = _StubClient(body={"id": "t"})
        # A tenant name with a slash must be percent-encoded so it
        # cannot leak into additional path components.
        c.a2a_get_task("ns/escape", "tnt", "t")
        self.assertIn("/a2a/ns%2Fescape/tnt/v1/tasks/t", c.calls[0].path)


# ---------------------------------------------------------------------
# JSON-RPC error unwrap surfaces ApiError
# ---------------------------------------------------------------------


class TestJsonRpcErrorUnwrap(unittest.TestCase):
    def test_jsonrpc_error_body_raises_api_error(self):
        from acteon_client import ApiError

        c = _StubClient(
            body={
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32001, "message": "task not found"},
            }
        )
        with self.assertRaises(ApiError) as ctx:
            c.a2a_get_authenticated_extended_card("ns", "tnt")
        self.assertIn("task not found", ctx.exception.message)
        self.assertEqual(ctx.exception.code, "-32001")


if __name__ == "__main__":
    unittest.main()
