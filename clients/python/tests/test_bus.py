"""Phase 8a Python bus surface — model serde + URL builder smoke.

Live HTTP tests would need a running Acteon instance with the bus
feature enabled; instead these tests exercise the bus-side
serialization (round-trip via ``to_dict`` / ``from_dict``) and the
``bus_stream_consume_url`` builder. The contract under test is:
the SDK round-trips every wire field the server expects, and
encodes path segments correctly.
"""

import unittest

from acteon_client import (
    AppendBusConversationMessage,
    BusAgent,
    BusApprovalDecision,
    BusApprovalDecisionResponse,
    BusApprovalView,
    BusConversation,
    BusLag,
    BusReplayResponse,
    BusSchema,
    BusStreamEnvelopeReceipt,
    BusSubscription,
    BusToolEnvelopeReceipt,
    BusToolResult,
    BusToolResultLookup,
    BusToolResultLookupParams,
    BusTopic,
    CreateBusConversation,
    CreateBusSubscription,
    CreateBusTopic,
    PostBusStreamChunk,
    PostBusStreamEnd,
    PostBusToolCall,
    PostBusToolResult,
    PublishBusMessage,
    RegisterBusAgent,
    RegisterBusSchema,
)


class TestRequestSerde(unittest.TestCase):
    def test_create_topic_minimal(self):
        req = CreateBusTopic(name="t", namespace="n", tenant="te")
        d = req.to_dict()
        self.assertEqual(d, {"name": "t", "namespace": "n", "tenant": "te"})

    def test_create_topic_full(self):
        req = CreateBusTopic(
            name="t", namespace="n", tenant="te",
            partitions=4, replication_factor=2, retention_ms=86_400_000,
            description="demo", labels={"env": "prod"},
        )
        d = req.to_dict()
        self.assertEqual(d["partitions"], 4)
        self.assertEqual(d["labels"], {"env": "prod"})

    def test_publish_message_payload_required(self):
        req = PublishBusMessage(topic="ns.te.t", payload={"x": 1})
        self.assertEqual(req.to_dict()["payload"], {"x": 1})

    def test_subscription_request(self):
        req = CreateBusSubscription(
            id="s1", topic="ns.te.t", namespace="ns", tenant="te",
            ack_mode="manual", ack_timeout_ms=30_000,
        )
        d = req.to_dict()
        self.assertEqual(d["ack_mode"], "manual")
        self.assertEqual(d["ack_timeout_ms"], 30_000)

    def test_register_schema(self):
        req = RegisterBusSchema(
            subject="orders", namespace="ns", tenant="te",
            body={"type": "object"},
        )
        self.assertEqual(req.to_dict()["body"], {"type": "object"})

    def test_register_agent(self):
        req = RegisterBusAgent(
            agent_id="a1", namespace="ns", tenant="te",
            capabilities=["tools.calendar"],
            heartbeat_ttl_ms=30_000,
        )
        d = req.to_dict()
        self.assertEqual(d["capabilities"], ["tools.calendar"])
        self.assertEqual(d["heartbeat_ttl_ms"], 30_000)

    def test_create_conversation(self):
        req = CreateBusConversation(
            conversation_id="c1", namespace="ns", tenant="te",
            participants=["a1", "a2"],
        )
        self.assertEqual(req.to_dict()["participants"], ["a1", "a2"])

    def test_append_message(self):
        req = AppendBusConversationMessage(
            payload={"text": "hi"}, sender="a1",
        )
        self.assertEqual(req.to_dict()["sender"], "a1")

    def test_post_tool_call_basic(self):
        req = PostBusToolCall(call_id="call-1", tool="calendar.list")
        d = req.to_dict()
        self.assertEqual(d["arguments"], {})
        self.assertNotIn("require_approval", d)

    def test_post_tool_call_with_approval_gate(self):
        req = PostBusToolCall(
            call_id="call-1", tool="billing.charge",
            arguments={"usd": 42}, sender="planner-1",
            require_approval=True,
            approval_reason="paid action",
            approval_ttl_ms=600_000,
        )
        d = req.to_dict()
        self.assertTrue(d["require_approval"])
        self.assertEqual(d["approval_reason"], "paid action")
        self.assertEqual(d["approval_ttl_ms"], 600_000)

    def test_post_tool_result_error_case(self):
        req = PostBusToolResult(
            call_id="call-1", status="error",
            error_message="upstream gave up", sender="calendar-svc",
        )
        d = req.to_dict()
        self.assertEqual(d["status"], "error")
        self.assertEqual(d["error_message"], "upstream gave up")

    def test_lookup_params_query(self):
        # Phase 10 dropped `as_agent` — read-side identity comes
        # from the API-key grant now, not a query parameter.
        p = BusToolResultLookupParams(
            conversation_id="c1", cursor="abc", timeout_ms=5_000,
        )
        q = p.to_query()
        self.assertEqual(q["conversation_id"], "c1")
        self.assertEqual(q["cursor"], "abc")
        self.assertEqual(q["timeout_ms"], 5_000)
        self.assertNotIn("as_agent", q)

    def test_stream_chunk_serializes_body(self):
        req = PostBusStreamChunk(
            stream_id="s1", chunk_seq=0,
            body={"token": "Once "},
        )
        self.assertEqual(req.to_dict()["body"], {"token": "Once "})

    def test_stream_end_complete(self):
        req = PostBusStreamEnd(stream_id="s1", chunk_seq=5, status="complete")
        self.assertEqual(req.to_dict()["status"], "complete")

    def test_approval_decision(self):
        d = BusApprovalDecision(
            decided_by="ops-1", decision_note="verified PO",
        ).to_dict()
        self.assertEqual(d["decided_by"], "ops-1")
        self.assertEqual(d["decision_note"], "verified PO")


class TestResponseSerde(unittest.TestCase):
    def test_topic_round_trip_optional_fields(self):
        t = BusTopic.from_dict({
            "name": "t", "namespace": "n", "tenant": "te",
            "kafka_name": "n.te.t", "partitions": 4, "replication_factor": 2,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
        })
        self.assertEqual(t.kafka_name, "n.te.t")
        # Server omits these when not bound; SDK stores None instead
        # of raising.
        self.assertIsNone(t.schema_subject)
        self.assertEqual(t.labels, {})

    def test_subscription_full(self):
        s = BusSubscription.from_dict({
            "id": "s1", "topic": "n.te.t", "namespace": "n", "tenant": "te",
            "starting_offset": "latest", "ack_mode": "manual",
            "dead_letter_topic": "n.te.t-dlq", "ack_timeout_ms": 30_000,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
        })
        self.assertEqual(s.ack_timeout_ms, 30_000)
        self.assertEqual(s.dead_letter_topic, "n.te.t-dlq")

    def test_lag_partitions(self):
        lag = BusLag.from_dict({
            "subscription_id": "s1", "topic": "n.te.t",
            "partitions": [
                {"partition": 0, "committed": 10, "high_water_mark": 12, "lag": 2},
                {"partition": 1, "committed": 0, "high_water_mark": 0, "lag": 0},
            ],
            "total_lag": 2,
        })
        self.assertEqual(lag.total_lag, 2)
        self.assertEqual(len(lag.partitions), 2)
        self.assertEqual(lag.partitions[0].lag, 2)

    def test_schema_round_trip(self):
        s = BusSchema.from_dict({
            "subject": "orders", "version": 3, "namespace": "n",
            "tenant": "te", "body": {"type": "object"},
            "created_at": "2026-01-01T00:00:00Z",
        })
        self.assertEqual(s.version, 3)
        self.assertEqual(s.body, {"type": "object"})

    def test_agent_heartbeat_can_be_null(self):
        a = BusAgent.from_dict({
            "agent_id": "a1", "namespace": "n", "tenant": "te",
            "capabilities": [], "inbox_topic": "n.te.agents.a1",
            "status": "registered", "heartbeat_ttl_ms": 30_000,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
        })
        self.assertIsNone(a.last_heartbeat_at)
        self.assertEqual(a.capabilities, [])

    def test_agent_admin_state_defaults_to_active_when_field_absent(self):
        # A server that pre-dates the admin-state surface omits the
        # field entirely; the dataclass must default to "active" so
        # operator dashboards don't render "None".
        a = BusAgent.from_dict({
            "agent_id": "a1", "namespace": "n", "tenant": "te",
            "capabilities": [], "inbox_topic": "n.te.agents.a1",
            "status": "registered", "heartbeat_ttl_ms": 30_000,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
        })
        self.assertEqual(a.admin_state, "active")
        self.assertIsNone(a.admin_reason)
        self.assertIsNone(a.admin_set_by)

    def test_agent_admin_state_round_trips_banned(self):
        a = BusAgent.from_dict({
            "agent_id": "a1", "namespace": "n", "tenant": "te",
            "capabilities": [], "inbox_topic": "n.te.agents.a1",
            "status": "online", "heartbeat_ttl_ms": 30_000,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "admin_state": "banned",
            "admin_reason": "exfiltration",
            "admin_set_by": "op@acme.io",
            "admin_set_at": "2026-05-23T10:00:00Z",
        })
        self.assertEqual(a.admin_state, "banned")
        self.assertEqual(a.admin_reason, "exfiltration")
        self.assertEqual(a.admin_set_by, "op@acme.io")

    def test_set_admin_state_request_drops_optional_nones(self):
        from acteon_client import SetBusAgentAdminState
        # Minimal — only admin_state.
        d = SetBusAgentAdminState(admin_state="suspended").to_dict()
        self.assertEqual(d, {"admin_state": "suspended"})
        # Full — every field appears.
        d = SetBusAgentAdminState(
            admin_state="suspended",
            reason="flaky retries",
            expires_at="2026-05-23T12:00:00Z",
        ).to_dict()
        self.assertEqual(d, {
            "admin_state": "suspended",
            "reason": "flaky retries",
            "expires_at": "2026-05-23T12:00:00Z",
        })

    def test_conversation_default_participants(self):
        c = BusConversation.from_dict({
            "conversation_id": "c1", "namespace": "n", "tenant": "te",
            "participants": [], "state": "open",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
        })
        self.assertEqual(c.state, "open")
        self.assertEqual(c.participants, [])

    def test_replay_response(self):
        r = BusReplayResponse.from_dict({
            "conversation_id": "c1",
            "events_topic": "n.te.conversations-events",
            "messages": [
                {
                    "partition": 0, "offset": 7,
                    "produced_at": "2026-01-01T00:00:00Z",
                    "sender": "a1",
                    "payload": {"text": "hi"},
                    "headers": {"acteon.envelope.kind": "tool_call"},
                }
            ],
            "exit_reason": "limit",
        })
        self.assertEqual(len(r.messages), 1)
        self.assertEqual(r.messages[0].sender, "a1")

    def test_tool_envelope_receipt(self):
        r = BusToolEnvelopeReceipt.from_dict({
            "events_topic": "n.te.events",
            "conversation_id": "c1", "call_id": "call-1",
            "partition": 0, "offset": 42,
            "produced_at": "2026-01-01T00:00:00Z",
            "cursor": "eyIwIjogNDJ9",
        })
        self.assertEqual(r.cursor, "eyIwIjogNDJ9")

    def test_tool_result_lookup(self):
        l = BusToolResultLookup.from_dict({
            "call_id": "call-1",
            "events_topic": "n.te.events",
            "conversation_id": "c1",
            "partition": 0, "offset": 43,
            "produced_at": "2026-01-01T00:00:00Z",
            "result": {
                "call_id": "call-1", "status": "ok",
                "output": {"events": []},
                "created_at": "2026-01-01T00:00:00Z",
            },
        })
        self.assertEqual(l.result.status, "ok")

    def test_stream_receipt(self):
        r = BusStreamEnvelopeReceipt.from_dict({
            "events_topic": "n.te.events", "conversation_id": "c1",
            "stream_id": "s1", "chunk_seq": 0,
            "partition": 0, "offset": 5,
            "produced_at": "2026-01-01T00:00:00Z",
            "cursor": "abc",
        })
        self.assertEqual(r.stream_id, "s1")

    def test_approval_view_optional_decision(self):
        v = BusApprovalView.from_dict({
            "approval_id": "appr-1",
            "namespace": "n", "tenant": "te",
            "kind": "operator_approval",
            "conversation_id": "c1",
            "correlation_token": "call-1",
            "envelope_kind": "tool_call",
            "status": "pending",
            "created_at": "2026-01-01T00:00:00Z",
            "expires_at": "2026-01-02T00:00:00Z",
            "envelope": {"kind": "tool_call"},
        })
        self.assertEqual(v.status, "pending")
        self.assertEqual(v.kind, "operator_approval")
        self.assertEqual(v.conversation_id, "c1")
        self.assertIsNone(v.task_id)
        self.assertIsNone(v.decided_by)
        self.assertIsNone(v.produced_offset)

    def test_approval_decision_response_with_receipt(self):
        r = BusApprovalDecisionResponse.from_dict({
            "approval": {
                "approval_id": "appr-1", "namespace": "n", "tenant": "te",
                "kind": "operator_approval",
                "conversation_id": "c1", "correlation_token": "call-1",
                "envelope_kind": "tool_call", "status": "approved",
                "created_at": "2026-01-01T00:00:00Z",
                "expires_at": "2026-01-02T00:00:00Z",
                "envelope": {},
                "decided_by": "ops-1",
            },
            "receipt": {
                "events_topic": "n.te.events",
                "conversation_id": "c1", "call_id": "call-1",
                "partition": 0, "offset": 99,
                "produced_at": "2026-01-01T00:00:01Z",
                "cursor": "xx",
            },
        })
        self.assertEqual(r.approval.status, "approved")
        assert r.receipt is not None
        self.assertEqual(r.receipt.offset, 99)

    def test_approval_decision_response_without_receipt(self):
        r = BusApprovalDecisionResponse.from_dict({
            "approval": {
                "approval_id": "appr-1", "namespace": "n", "tenant": "te",
                "kind": "operator_approval",
                "conversation_id": "c1", "correlation_token": "call-1",
                "envelope_kind": "tool_call", "status": "rejected",
                "created_at": "2026-01-01T00:00:00Z",
                "expires_at": "2026-01-02T00:00:00Z",
                "envelope": {},
                "decided_by": "ops-1",
                "decision_note": "scope too broad",
            },
            "receipt": None,
        })
        self.assertEqual(r.approval.status, "rejected")
        self.assertIsNone(r.receipt)

    def test_tool_result_optional_error_message(self):
        r = BusToolResult.from_dict({
            "call_id": "call-1", "status": "ok",
            "output": {}, "metadata": {},
            "created_at": "2026-01-01T00:00:00Z",
        })
        self.assertIsNone(r.error_message)


class TestStreamConsumeUrl(unittest.TestCase):
    def test_segments_are_percent_encoded(self):
        from acteon_client import ActeonClient

        # Use a constructor that doesn't actually open a connection.
        c = ActeonClient("http://localhost:3000")
        url = c.bus_stream_consume_url(
            "agents/x", "demo", "thread/with/slashes", "story 1",
        )
        # The path slashes inside segments must be %2F-encoded so
        # they don't escape into the URL grammar.
        self.assertIn("agents%2Fx", url)
        self.assertIn("thread%2Fwith%2Fslashes", url)
        self.assertIn("story%201", url)

    def test_simple_segments_unchanged(self):
        from acteon_client import ActeonClient

        c = ActeonClient("http://localhost:3000")
        url = c.bus_stream_consume_url("agents", "demo", "thread-1", "stream-1")
        self.assertEqual(
            url,
            "http://localhost:3000/v1/bus/streams/agents/demo/thread-1/stream-1",
        )


class TestAsyncSurface(unittest.TestCase):
    """Smoke test that the async client carries an async bus surface.

    The runtime contract: every bus method on `AsyncActeonClient`
    must be a coroutine function so callers in asyncio runtimes
    don't accidentally block their event loop on a sync HTTP call.
    """

    def test_async_client_has_coroutine_bus_methods(self):
        import inspect

        from acteon_client import AsyncActeonClient

        # Sample of representative methods across phases — full
        # enumeration would be redundant once one fails.
        sentinels = [
            "create_bus_topic",
            "post_bus_tool_call",
            "post_bus_tool_result",
            "lookup_bus_tool_result",
            "post_bus_stream_chunk",
            "approve_bus_approval",
            "reject_bus_approval",
        ]
        for name in sentinels:
            method = getattr(AsyncActeonClient, name, None)
            self.assertIsNotNone(method, f"AsyncActeonClient.{name} missing")
            self.assertTrue(
                inspect.iscoroutinefunction(method),
                f"AsyncActeonClient.{name} must be `async def` to avoid "
                f"blocking the event loop",
            )

    def test_async_consume_url_is_sync(self):
        # Pure URL builder — explicitly sync on the async client too,
        # since there's no I/O. Callers don't need to `await` a
        # string-format helper.
        import inspect

        from acteon_client import AsyncActeonClient

        method = AsyncActeonClient.bus_stream_consume_url
        self.assertFalse(inspect.iscoroutinefunction(method))


class TestSseConsumerParsing(unittest.TestCase):
    """Round-trip the SSE protocol parser on synthetic frame streams,
    covering the four event shapes the bus emits plus keep-alives.
    """

    def test_envelope_parser_yields_frames_and_keep_alives(self):
        from acteon_client.bus import _KEEP_ALIVE, _SseFrame, _parse_sse_envelopes

        lines = [
            ":keep-alive",
            "event: bus.message",
            "id: 42",
            'data: {"topic":"agents.demo.events","offset":42}',
            "",
            "event: bus.error",
            'data: {"error":"broker disconnected"}',
            "",
        ]
        items = list(_parse_sse_envelopes(iter(lines)))
        self.assertIs(items[0], _KEEP_ALIVE)
        self.assertIsInstance(items[1], _SseFrame)
        self.assertEqual(items[1].event, "bus.message")
        self.assertEqual(items[1].id, "42")
        self.assertIn("agents.demo.events", items[1].data)
        self.assertIsInstance(items[2], _SseFrame)
        self.assertEqual(items[2].event, "bus.error")

    def test_subscribe_message_event(self):
        from acteon_client.bus import _SseFrame, _envelope_to_consume_item

        frame = _SseFrame(
            "bus.message",
            "1",
            '{"topic":"agents.demo.events","payload":{"k":"v"},"partition":0,"offset":7}',
        )
        item = _envelope_to_consume_item(frame)
        self.assertTrue(item.is_message)
        self.assertEqual(item.message.topic, "agents.demo.events")
        self.assertEqual(item.message.offset, 7)
        self.assertEqual(item.message.payload, {"k": "v"})

    def test_subscribe_error_event(self):
        from acteon_client.bus import _SseFrame, _envelope_to_consume_item

        frame = _SseFrame("bus.error", None, '{"error":"broker disconnected"}')
        item = _envelope_to_consume_item(frame)
        self.assertTrue(item.is_error)
        self.assertEqual(item.error, "broker disconnected")

    def test_subscribe_keep_alive(self):
        from acteon_client.bus import _KEEP_ALIVE, _envelope_to_consume_item

        item = _envelope_to_consume_item(_KEEP_ALIVE)
        self.assertTrue(item.is_keep_alive)

    def test_stream_chunk_and_end(self):
        from acteon_client.bus import _SseFrame, _envelope_to_stream_item

        chunk_frame = _SseFrame(
            "bus.stream.chunk",
            "0",
            '{"stream_id":"s1","chunk_seq":3,"body":{"token":"hi"},'
            '"created_at":"2026-05-02T12:00:00Z"}',
        )
        end_frame = _SseFrame(
            "bus.stream.end",
            "1",
            '{"stream_id":"s1","chunk_seq":4,"status":"complete",'
            '"created_at":"2026-05-02T12:00:01Z"}',
        )
        chunk_item = _envelope_to_stream_item(chunk_frame)
        self.assertTrue(chunk_item.is_chunk)
        self.assertEqual(chunk_item.chunk.stream_id, "s1")
        self.assertEqual(chunk_item.chunk.chunk_seq, 3)
        end_item = _envelope_to_stream_item(end_frame)
        self.assertTrue(end_item.is_end)
        self.assertEqual(end_item.end.status, "complete")

    def test_stream_error_event_with_plain_data(self):
        # Server emits `{"error": "..."}`, but if the JSON is malformed
        # for some reason we still want a useful message back.
        from acteon_client.bus import _SseFrame, _envelope_to_stream_item

        frame = _SseFrame("bus.stream.error", None, "broker disconnected")
        item = _envelope_to_stream_item(frame)
        self.assertTrue(item.is_error)
        self.assertEqual(item.error, "broker disconnected")

    def test_stream_unknown_event_raises(self):
        from acteon_client.bus import _SseFrame, _envelope_to_stream_item

        frame = _SseFrame("bogus", None, "{}")
        with self.assertRaises(ValueError):
            _envelope_to_stream_item(frame)


class TestReconnectBackoff(unittest.TestCase):
    """Behaviour contract for the best-effort reconnect helper."""

    def test_backoff_caps_at_max(self):
        from acteon_client.bus import _reconnect_backoff_ms
        from acteon_client.bus_models import ReconnectConfig

        cfg = ReconnectConfig(initial_backoff_ms=100, max_backoff_ms=5_000)
        self.assertEqual(_reconnect_backoff_ms(0, cfg), 100)
        self.assertEqual(_reconnect_backoff_ms(1, cfg), 200)
        self.assertEqual(_reconnect_backoff_ms(2, cfg), 400)
        # Past the cap.
        self.assertEqual(_reconnect_backoff_ms(20, cfg), 5_000)
        # Bounded shift handles wild attempt counters cleanly.
        self.assertEqual(_reconnect_backoff_ms(64, cfg), 5_000)

    def test_reconnected_item_helpers(self):
        from acteon_client.bus_models import (
            BusConsumeItem,
            BusConsumedMessage,
            ReconnectedInfo,
        )

        keep_alive = BusConsumeItem()
        self.assertTrue(keep_alive.is_keep_alive)
        self.assertFalse(keep_alive.is_reconnected)

        message = BusConsumeItem(message=BusConsumedMessage(topic="t"))
        self.assertTrue(message.is_message)
        self.assertFalse(message.is_keep_alive)

        reconnected = BusConsumeItem(
            reconnected=ReconnectedInfo(backoff_ms=500, attempt=1)
        )
        self.assertTrue(reconnected.is_reconnected)
        self.assertFalse(reconnected.is_keep_alive)
        self.assertFalse(reconnected.is_message)
        self.assertFalse(reconnected.is_error)


if __name__ == "__main__":
    unittest.main()
