from __future__ import annotations

import hashlib
import hmac
import json
import sqlite3
import uuid
from pathlib import Path
from typing import Any, cast

from suunto_mcp.config import Settings, settings
from suunto_mcp.models import utc_now

WEBHOOK_TYPE_KINDS = {
    "WORKOUT_CREATED": "suunto-workout",
    "ROUTE_CREATED": "suunto-route",
    "SUUNTO_247_ACTIVITY_CREATED": "suunto-247-activity",
    "SUUNTO_247_SLEEP_CREATED": "suunto-247-sleep",
    "SUUNTO_247_RECOVERY_CREATED": "suunto-247-recovery",
}


def classify_webhook_kind(payload: dict[str, Any], *, fallback: str = "suunto-json") -> str:
    webhook_type = payload.get("type")
    if isinstance(webhook_type, str):
        return WEBHOOK_TYPE_KINDS.get(webhook_type.strip().upper(), fallback)
    return fallback


def verify_webhook_signature(
    body: bytes,
    signature: str,
    *,
    secret: str | None = None,
    settings_obj: Settings = settings,
) -> bool:
    actual_secret = secret or settings_obj.WEBHOOK_SECRET
    if not actual_secret:
        raise ValueError("Webhook secret is required for signature verification.")
    expected = hmac.new(actual_secret.encode("utf-8"), body, hashlib.sha256).hexdigest()
    normalized = signature.removeprefix("sha256=").strip()
    return hmac.compare_digest(expected, normalized)


class JsonlWebhookStore:
    def __init__(self, path: str | Path) -> None:
        self.path = Path(path).expanduser()

    def append(self, event: dict[str, Any]) -> dict[str, Any]:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        event_id = event.get("id") or str(uuid.uuid4())
        stored = {
            "id": event_id,
            "received_at": utc_now().isoformat(),
            **event,
        }
        with self.path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(stored, sort_keys=True) + "\n")
        return stored

    def list(self, limit: int = 100) -> list[dict[str, Any]]:
        if not self.path.exists():
            return []
        lines = self.path.read_text(encoding="utf-8").splitlines()
        events = [json.loads(line) for line in lines[-limit:] if line.strip()]
        return list(reversed(events))

    def get(self, event_id: str) -> dict[str, Any] | None:
        if not self.path.exists():
            return None
        for line in self.path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            event = json.loads(line)
            if event.get("id") == event_id:
                return event
        return None

    def delete(self, event_id: str) -> bool:
        if not self.path.exists():
            return False
        kept: list[str] = []
        removed = False
        for line in self.path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            event = json.loads(line)
            if event.get("id") == event_id:
                removed = True
                continue
            kept.append(json.dumps(event, sort_keys=True))
        if removed:
            self.path.write_text("\n".join(kept) + ("\n" if kept else ""), encoding="utf-8")
        return removed


class SqliteWebhookStore:
    def __init__(self, path: str | Path) -> None:
        self.path = Path(path).expanduser()

    def _connect(self) -> sqlite3.Connection:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        connection = sqlite3.connect(self.path)
        connection.row_factory = sqlite3.Row
        connection.execute(
            """
            CREATE TABLE IF NOT EXISTS webhook_events (
                id TEXT PRIMARY KEY,
                received_at TEXT NOT NULL,
                kind TEXT,
                username TEXT,
                payload_json TEXT NOT NULL
            )
            """
        )
        connection.execute(
            "CREATE INDEX IF NOT EXISTS idx_webhook_events_received_at "
            "ON webhook_events(received_at)"
        )
        connection.execute(
            "CREATE INDEX IF NOT EXISTS idx_webhook_events_username ON webhook_events(username)"
        )
        return connection

    @staticmethod
    def _username(event: dict[str, Any]) -> str | None:
        payload = event.get("payload")
        if isinstance(payload, dict):
            payload_dict = cast(dict[str, Any], payload)
            username = payload_dict.get("username")
            if isinstance(username, str):
                return username
        username = event.get("username")
        return username if isinstance(username, str) else None

    @staticmethod
    def _event_from_row(row: sqlite3.Row) -> dict[str, Any]:
        payload_obj: object = json.loads(row["payload_json"])
        if not isinstance(payload_obj, dict):
            raise ValueError("Stored webhook payload is not a JSON object.")
        return cast(dict[str, Any], payload_obj)

    def append(self, event: dict[str, Any]) -> dict[str, Any]:
        event_id = event.get("id") or str(uuid.uuid4())
        stored = {
            "id": event_id,
            "received_at": utc_now().isoformat(),
            **event,
        }
        with self._connect() as connection:
            connection.execute(
                """
                INSERT OR REPLACE INTO webhook_events
                    (id, received_at, kind, username, payload_json)
                VALUES (?, ?, ?, ?, ?)
                """,
                (
                    stored["id"],
                    stored["received_at"],
                    stored.get("kind"),
                    self._username(stored),
                    json.dumps(stored, sort_keys=True),
                ),
            )
        return stored

    def list(self, limit: int = 100) -> list[dict[str, Any]]:
        with self._connect() as connection:
            rows = connection.execute(
                """
                SELECT payload_json
                FROM webhook_events
                ORDER BY received_at DESC
                LIMIT ?
                """,
                (limit,),
            ).fetchall()
        return [self._event_from_row(row) for row in rows]

    def get(self, event_id: str) -> dict[str, Any] | None:
        with self._connect() as connection:
            row = connection.execute(
                "SELECT payload_json FROM webhook_events WHERE id = ?",
                (event_id,),
            ).fetchone()
        return self._event_from_row(row) if row is not None else None

    def delete(self, event_id: str) -> bool:
        with self._connect() as connection:
            cursor = connection.execute("DELETE FROM webhook_events WHERE id = ?", (event_id,))
        return cursor.rowcount > 0


_memory_events: list[dict[str, Any]] = []


def default_webhook_path(settings_obj: Settings = settings) -> Path:
    if settings_obj.WEBHOOK_STORE_PATH:
        return Path(settings_obj.WEBHOOK_STORE_PATH).expanduser()
    if settings_obj.WEBHOOK_STORE == "sqlite":
        return Path(settings_obj.LOCAL_DATA_DIR).expanduser().parent / "webhooks.sqlite3"
    return Path(settings_obj.LOCAL_DATA_DIR).expanduser().parent / "webhooks.jsonl"


def store_webhook_event(event: dict[str, Any], settings_obj: Settings = settings) -> dict[str, Any]:
    if settings_obj.WEBHOOK_STORE == "memory":
        stored = {
            "id": event.get("id") or str(uuid.uuid4()),
            "received_at": utc_now().isoformat(),
            **event,
        }
        _memory_events.append(stored)
        return stored
    if settings_obj.WEBHOOK_STORE == "jsonl":
        return JsonlWebhookStore(default_webhook_path(settings_obj)).append(event)
    if settings_obj.WEBHOOK_STORE == "sqlite":
        return SqliteWebhookStore(default_webhook_path(settings_obj)).append(event)
    raise ValueError(f"Unsupported webhook store: {settings_obj.WEBHOOK_STORE}")


def list_webhook_events(
    limit: int = 100, settings_obj: Settings = settings
) -> list[dict[str, Any]]:
    if settings_obj.WEBHOOK_STORE == "memory":
        return list(reversed(_memory_events[-limit:]))
    if settings_obj.WEBHOOK_STORE == "jsonl":
        return JsonlWebhookStore(default_webhook_path(settings_obj)).list(limit)
    if settings_obj.WEBHOOK_STORE == "sqlite":
        return SqliteWebhookStore(default_webhook_path(settings_obj)).list(limit)
    raise ValueError(f"Unsupported webhook store: {settings_obj.WEBHOOK_STORE}")


def get_webhook_event(event_id: str, settings_obj: Settings = settings) -> dict[str, Any] | None:
    if settings_obj.WEBHOOK_STORE == "memory":
        return next((event for event in _memory_events if event.get("id") == event_id), None)
    if settings_obj.WEBHOOK_STORE == "jsonl":
        return JsonlWebhookStore(default_webhook_path(settings_obj)).get(event_id)
    if settings_obj.WEBHOOK_STORE == "sqlite":
        return SqliteWebhookStore(default_webhook_path(settings_obj)).get(event_id)
    raise ValueError(f"Unsupported webhook store: {settings_obj.WEBHOOK_STORE}")


def delete_webhook_event(event_id: str, settings_obj: Settings = settings) -> bool:
    if settings_obj.WEBHOOK_STORE == "memory":
        for index, event in enumerate(_memory_events):
            if event.get("id") == event_id:
                del _memory_events[index]
                return True
        return False
    if settings_obj.WEBHOOK_STORE == "jsonl":
        return JsonlWebhookStore(default_webhook_path(settings_obj)).delete(event_id)
    if settings_obj.WEBHOOK_STORE == "sqlite":
        return SqliteWebhookStore(default_webhook_path(settings_obj)).delete(event_id)
    raise ValueError(f"Unsupported webhook store: {settings_obj.WEBHOOK_STORE}")


def _payload_from_event(event: dict[str, Any]) -> dict[str, Any]:
    payload = event.get("payload")
    if isinstance(payload, dict):
        return cast(dict[str, Any], payload)
    return event


def _sample_range(samples: object) -> dict[str, str]:
    if not isinstance(samples, list):
        return {}
    timestamps: list[str] = []
    for sample in cast(list[Any], samples):
        if isinstance(sample, dict):
            sample_dict = cast(dict[str, Any], sample)
            timestamp = sample_dict.get("timestamp")
            if isinstance(timestamp, str):
                timestamps.append(timestamp)
    if not timestamps:
        return {}
    return {"from_value": min(timestamps), "to_value": max(timestamps)}


def suggest_webhook_followups(event: dict[str, Any]) -> dict[str, Any]:
    payload = _payload_from_event(event)
    webhook_type = payload.get("type")
    kind = classify_webhook_kind(payload, fallback=str(event.get("kind") or "suunto-json"))
    username = payload.get("username")
    base_arguments: dict[str, Any] = {}
    if isinstance(username, str) and username:
        base_arguments["account_id"] = username

    actions: list[dict[str, Any]] = []
    workout_obj = payload.get("workout")
    workout_key = None
    if isinstance(workout_obj, dict):
        workout_dict = cast(dict[str, Any], workout_obj)
        workout_key_obj = workout_dict.get("workoutKey")
        if isinstance(workout_key_obj, str):
            workout_key = workout_key_obj
    legacy_workout_id = payload.get("workoutid")
    if workout_key is None and isinstance(legacy_workout_id, str):
        workout_key = legacy_workout_id
    if kind == "suunto-workout" and workout_key:
        args = {**base_arguments, "workout_id_or_key": workout_key}
        actions.extend(
            [
                {"tool": "suunto_get_workout", "arguments": args},
                {"tool": "suunto_get_workout_fit", "arguments": args},
            ]
        )

    route_obj = payload.get("route")
    route_id = None
    if isinstance(route_obj, dict):
        route_dict = cast(dict[str, Any], route_obj)
        route_id_obj = route_dict.get("id")
        if isinstance(route_id_obj, str):
            route_id = route_id_obj
    if kind == "suunto-route" and route_id:
        args = {**base_arguments, "route_id": route_id}
        actions.extend(
            [
                {"tool": "suunto_get_route", "arguments": args},
                {"tool": "suunto_export_route_gpx", "arguments": args},
            ]
        )

    ranges = _sample_range(payload.get("samples"))
    if ranges:
        args = {**base_arguments, **ranges}
        if kind == "suunto-247-activity":
            actions.append({"tool": "suunto_get_activity_samples", "arguments": args})
        elif kind == "suunto-247-sleep":
            actions.append({"tool": "suunto_get_sleep_data", "arguments": args})
        elif kind == "suunto-247-recovery":
            actions.append({"tool": "suunto_get_recovery_data", "arguments": args})

    return {
        "type": webhook_type,
        "kind": kind,
        "username": username if isinstance(username, str) else None,
        "actions": actions,
    }
