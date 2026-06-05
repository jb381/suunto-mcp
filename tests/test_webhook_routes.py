from __future__ import annotations

import hashlib
import hmac

from starlette.testclient import TestClient

from suunto_mcp.server import mcp
from suunto_mcp.webhooks import list_webhook_events


def test_json_webhook_route_stores_event(monkeypatch) -> None:
    from collections import deque

    monkeypatch.setattr("suunto_mcp.webhooks.settings.WEBHOOK_STORE", "memory")
    monkeypatch.setattr("suunto_mcp.webhooks._memory_events", deque(maxlen=10000))
    monkeypatch.setattr("suunto_mcp.server.settings.WEBHOOK_SECRET", "")

    app = mcp.http_app()
    with TestClient(app) as client:
        response = client.post(
            "/webhooks/suunto", json={"type": "ROUTE_CREATED", "username": "runner"}
        )

    assert response.status_code == 200
    assert response.json()["ok"] is True
    assert response.json()["event_id"]
    assert list_webhook_events(limit=1)[0]["kind"] == "suunto-route"


def test_typed_route_webhook_route_uses_route_kind_without_type(monkeypatch) -> None:
    from collections import deque

    monkeypatch.setattr("suunto_mcp.webhooks.settings.WEBHOOK_STORE", "memory")
    monkeypatch.setattr("suunto_mcp.webhooks._memory_events", deque(maxlen=10000))
    monkeypatch.setattr("suunto_mcp.server.settings.WEBHOOK_SECRET", "")

    app = mcp.http_app()
    with TestClient(app) as client:
        response = client.post("/webhooks/suunto/route", json={"username": "runner"})

    assert response.status_code == 200
    assert response.json()["ok"] is True
    assert list_webhook_events(limit=1)[0]["kind"] == "suunto-route"


def test_typed_sleep_webhook_route_uses_sleep_kind_without_type(monkeypatch) -> None:
    from collections import deque

    monkeypatch.setattr("suunto_mcp.webhooks.settings.WEBHOOK_STORE", "memory")
    monkeypatch.setattr("suunto_mcp.webhooks._memory_events", deque(maxlen=10000))
    monkeypatch.setattr("suunto_mcp.server.settings.WEBHOOK_SECRET", "")

    app = mcp.http_app()
    with TestClient(app) as client:
        response = client.post("/webhooks/suunto/247/sleep", json={"username": "runner"})

    assert response.status_code == 200
    assert response.json()["ok"] is True
    assert list_webhook_events(limit=1)[0]["kind"] == "suunto-247-sleep"


def test_json_webhook_route_rejects_invalid_signature(monkeypatch) -> None:
    from collections import deque

    monkeypatch.setattr("suunto_mcp.webhooks.settings.WEBHOOK_STORE", "memory")
    monkeypatch.setattr("suunto_mcp.webhooks._memory_events", deque(maxlen=10000))
    monkeypatch.setattr("suunto_mcp.server.settings.WEBHOOK_SECRET", "secret")

    app = mcp.http_app()
    with TestClient(app) as client:
        response = client.post(
            "/webhooks/suunto",
            json={"username": "runner"},
            headers={"X-HMAC-SHA256-Signature": "bad"},
        )

    assert response.status_code == 401


def test_json_webhook_route_accepts_valid_signature(monkeypatch) -> None:
    from collections import deque

    monkeypatch.setattr("suunto_mcp.webhooks.settings.WEBHOOK_STORE", "memory")
    monkeypatch.setattr("suunto_mcp.webhooks._memory_events", deque(maxlen=10000))
    monkeypatch.setattr("suunto_mcp.server.settings.WEBHOOK_SECRET", "secret")
    body = b'{"username":"runner"}'
    signature = hmac.new(b"secret", body, hashlib.sha256).hexdigest()

    app = mcp.http_app()
    with TestClient(app) as client:
        response = client.post(
            "/webhooks/suunto",
            content=body,
            headers={
                "Content-Type": "application/json",
                "X-HMAC-SHA256-Signature": signature,
            },
        )

    assert response.status_code == 200
    assert response.json()["ok"] is True


def test_json_webhook_route_rejects_malformed_json(monkeypatch) -> None:
    from collections import deque

    monkeypatch.setattr("suunto_mcp.webhooks.settings.WEBHOOK_STORE", "memory")
    monkeypatch.setattr("suunto_mcp.webhooks._memory_events", deque(maxlen=10000))
    monkeypatch.setattr("suunto_mcp.server.settings.WEBHOOK_SECRET", "")

    app = mcp.http_app()
    with TestClient(app) as client:
        response = client.post(
            "/webhooks/suunto",
            content=b"{not-json",
            headers={"Content-Type": "application/json"},
        )

    assert response.status_code == 400
    assert response.json() == {"ok": False, "error": "invalid_json"}


def test_legacy_workout_webhook_route_stores_form_event(monkeypatch) -> None:
    from collections import deque

    monkeypatch.setattr("suunto_mcp.webhooks.settings.WEBHOOK_STORE", "memory")
    monkeypatch.setattr("suunto_mcp.webhooks._memory_events", deque(maxlen=10000))
    monkeypatch.setattr("suunto_mcp.server.settings.WEBHOOK_SECRET", "")

    app = mcp.http_app()
    with TestClient(app) as client:
        response = client.post(
            "/webhooks/suunto/legacy-workout",
            data={"username": "runner", "workoutid": "w1"},
        )

    assert response.status_code == 200
    assert response.json()["ok"] is True
