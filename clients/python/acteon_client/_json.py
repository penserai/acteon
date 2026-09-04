"""Validated JSON shapes at the HTTP boundary."""

from typing import Any


def json_object(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or any(not isinstance(key, str) for key in value):
        raise ValueError("Expected a JSON object in the Acteon response")
    return value


def json_objects(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        raise ValueError("Expected a JSON array in the Acteon response")
    return [json_object(item) for item in value]


def json_string(value: Any) -> str:
    if not isinstance(value, str):
        raise ValueError("Expected a JSON string in the Acteon response")
    return value
