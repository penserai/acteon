import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

from acteon_client.models import Action, ActionOutcome


def test_action_matches_the_rust_wire_fixture():
    fixture = json.loads(
        (Path(__file__).parent / "../../contract-fixtures/action-wire.json").read_text()
    )
    action = Action(
        id=fixture["id"],
        namespace=fixture["namespace"],
        tenant=fixture["tenant"],
        provider=fixture["provider"],
        action_type=fixture["action_type"],
        payload=fixture["payload"],
        metadata=fixture["metadata"],
        created_at=datetime(2026, 1, 1, 2, tzinfo=timezone(timedelta(hours=2))),
    )
    assert action.to_dict() == fixture


def test_deduplication_uses_the_rust_string_variant():
    assert ActionOutcome.from_dict("Deduplicated").is_deduplicated()
