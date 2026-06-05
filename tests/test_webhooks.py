from __future__ import annotations

from collections import deque

from suunto_mcp.config import Settings
from suunto_mcp.webhooks import (
    _MEMORY_STORE_MAXLEN,
    classify_webhook_kind,
    delete_webhook_event,
    get_webhook_event,
    list_webhook_events,
    store_webhook_event,
    suggest_webhook_followups,
)


def test_classify_webhook_kind_maps_documented_suunto_types() -> None:
    assert classify_webhook_kind({"type": "WORKOUT_CREATED"}) == "suunto-workout"
    assert classify_webhook_kind({"type": "ROUTE_CREATED"}) == "suunto-route"
    assert classify_webhook_kind({"type": "SUUNTO_247_ACTIVITY_CREATED"}) == "suunto-247-activity"
    assert classify_webhook_kind({"type": "SUUNTO_247_SLEEP_CREATED"}) == "suunto-247-sleep"
    assert classify_webhook_kind({"type": "SUUNTO_247_RECOVERY_CREATED"}) == "suunto-247-recovery"


def test_classify_webhook_kind_uses_fallback_for_unknown_type() -> None:
    assert classify_webhook_kind({"type": "UNKNOWN"}, fallback="suunto-custom") == "suunto-custom"
    assert classify_webhook_kind({}, fallback="suunto-custom") == "suunto-custom"


def test_sqlite_webhook_store_round_trips_events(tmp_path) -> None:
    settings = Settings(
        WEBHOOK_STORE="sqlite",
        WEBHOOK_STORE_PATH=str(tmp_path / "webhooks.sqlite3"),
    )

    stored = store_webhook_event(
        {
            "kind": "suunto-json",
            "payload": {"username": "runner42", "route": {"id": "route-1"}},
        },
        settings,
    )

    assert stored["id"]
    assert get_webhook_event(stored["id"], settings) == stored
    assert list_webhook_events(settings_obj=settings)[0] == stored


def test_sqlite_webhook_store_respects_limit(tmp_path) -> None:
    settings = Settings(
        WEBHOOK_STORE="sqlite",
        WEBHOOK_STORE_PATH=str(tmp_path / "webhooks.sqlite3"),
    )

    first = store_webhook_event({"id": "first", "payload": {"username": "a"}}, settings)
    second = store_webhook_event({"id": "second", "payload": {"username": "b"}}, settings)

    assert list_webhook_events(limit=1, settings_obj=settings) == [second]
    assert get_webhook_event("first", settings) == first


def test_sqlite_webhook_store_deletes_events(tmp_path) -> None:
    settings = Settings(
        WEBHOOK_STORE="sqlite",
        WEBHOOK_STORE_PATH=str(tmp_path / "webhooks.sqlite3"),
    )
    stored = store_webhook_event({"id": "event-1", "payload": {"username": "a"}}, settings)

    assert get_webhook_event(stored["id"], settings) is not None
    assert delete_webhook_event(stored["id"], settings) is True
    assert get_webhook_event(stored["id"], settings) is None
    assert delete_webhook_event(stored["id"], settings) is False


def test_jsonl_webhook_store_deletes_events(tmp_path) -> None:
    settings = Settings(
        WEBHOOK_STORE="jsonl",
        WEBHOOK_STORE_PATH=str(tmp_path / "webhooks.jsonl"),
    )
    stored = store_webhook_event({"id": "event-1", "payload": {"username": "a"}}, settings)

    assert get_webhook_event(stored["id"], settings) is not None
    assert delete_webhook_event(stored["id"], settings) is True
    assert list_webhook_events(settings_obj=settings) == []


def test_memory_store_respects_maxlen(monkeypatch) -> None:
    store = deque(maxlen=_MEMORY_STORE_MAXLEN)
    monkeypatch.setattr("suunto_mcp.webhooks._memory_events", store)
    settings = Settings(WEBHOOK_STORE="memory")

    for i in range(_MEMORY_STORE_MAXLEN + 5):
        store_webhook_event({"id": str(i)}, settings)

    assert len(store) == _MEMORY_STORE_MAXLEN
    oldest = list(store)[0]
    assert oldest["id"] == "5"


def test_suggest_webhook_followups_for_workout_route_and_247() -> None:
    workout = suggest_webhook_followups(
        {
            "payload": {
                "type": "WORKOUT_CREATED",
                "username": "runner",
                "workout": {"workoutKey": "workout-1"},
            }
        }
    )
    route = suggest_webhook_followups(
        {
            "payload": {
                "type": "ROUTE_CREATED",
                "username": "runner",
                "route": {"id": "route-1"},
            }
        }
    )
    activity = suggest_webhook_followups(
        {
            "payload": {
                "type": "SUUNTO_247_ACTIVITY_CREATED",
                "username": "runner",
                "samples": [
                    {"timestamp": "2026-06-01T08:00:00+02:00"},
                    {"timestamp": "2026-06-01T08:10:00+02:00"},
                ],
            }
        }
    )

    assert [action["tool"] for action in workout["actions"]] == [
        "suunto_get_workout",
        "suunto_get_workout_fit",
    ]
    assert workout["actions"][0]["arguments"]["workout_id_or_key"] == "workout-1"
    assert [action["tool"] for action in route["actions"]] == [
        "suunto_get_route",
        "suunto_export_route_gpx",
    ]
    assert route["actions"][0]["arguments"]["route_id"] == "route-1"
    assert activity["actions"] == [
        {
            "tool": "suunto_get_activity_samples",
            "arguments": {
                "account_id": "runner",
                "from_value": "2026-06-01T08:00:00+02:00",
                "to_value": "2026-06-01T08:10:00+02:00",
            },
        }
    ]
