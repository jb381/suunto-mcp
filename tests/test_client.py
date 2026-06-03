from __future__ import annotations

import base64
import json
from collections.abc import Mapping

import respx
from httpx import Response

from suunto_mcp.client import SuuntoClient
from suunto_mcp.config import Settings
from suunto_mcp.models import TokenRecord
from suunto_mcp.quota import quota_status
from suunto_mcp.token_store import InMemoryTokenStore


def _jwt(payload: dict[str, object]) -> str:
    header = {"alg": "none", "typ": "JWT"}

    def encode(data: Mapping[str, object]) -> str:
        raw = json.dumps(data, separators=(",", ":")).encode("utf-8")
        return base64.urlsafe_b64encode(raw).decode("ascii").rstrip("=")

    return f"{encode(header)}.{encode(payload)}."


@respx.mock
async def test_suunto_client_adds_subscription_and_bearer_headers() -> None:
    settings = Settings(
        API_BASE="https://cloudapi.suunto.test",
        SUBSCRIPTION_KEY="subscription-key",
        TOKEN_STORE="memory",
        API_CALLS_PER_MINUTE=0,
        API_WEEKLY_CALL_LIMIT=0,
    )
    store = InMemoryTokenStore()
    store.save(TokenRecord(account_id="runner42", access_token="access-token"))

    route = respx.get("https://cloudapi.suunto.test/v3/workouts").mock(
        return_value=Response(200, json={"payload": []})
    )

    async with SuuntoClient(
        account_id="runner42",
        settings_obj=settings,
        token_store=store,
    ) as client:
        payload = await client.get_json("/v3/workouts")

    assert payload == {"payload": []}
    request = route.calls.last.request
    assert request.headers["Authorization"] == "Bearer access-token"
    assert request.headers["Ocp-Apim-Subscription-Key"] == "subscription-key"


@respx.mock
async def test_suunto_client_can_call_deprecated_v2_workout_paths() -> None:
    settings = Settings(
        API_BASE="https://cloudapi.suunto.test",
        SUBSCRIPTION_KEY="subscription-key",
        TOKEN_STORE="memory",
        API_CALLS_PER_MINUTE=0,
        API_WEEKLY_CALL_LIMIT=0,
    )
    store = InMemoryTokenStore()
    store.save(TokenRecord(account_id="runner42", access_token="access-token"))
    list_route = respx.get("https://cloudapi.suunto.test/v2/workouts").mock(
        return_value=Response(200, json={"payload": []})
    )
    fit_route = respx.get("https://cloudapi.suunto.test/v2/workouts/abc123/fit").mock(
        return_value=Response(200, content=b"fit")
    )

    async with SuuntoClient(
        account_id="runner42",
        settings_obj=settings,
        token_store=store,
    ) as client:
        assert await client.get_json("/v2/workouts") == {"payload": []}
        data, _content_type = await client.get_bytes("/v2/workouts/abc123/fit")

    assert data == b"fit"
    assert list_route.called
    assert fit_route.called


@respx.mock
async def test_suunto_client_refreshes_and_retries_after_unauthorized(tmp_path) -> None:
    settings = Settings(
        API_BASE="https://cloudapi.suunto.test",
        OAUTH_BASE="https://oauth.suunto.test",
        CLIENT_ID="client-id",
        CLIENT_SECRET="client-secret",
        SUBSCRIPTION_KEY="subscription-key",
        TOKEN_STORE="memory",
        API_CALLS_PER_MINUTE=0,
        API_WEEKLY_CALL_LIMIT=10,
        API_QUOTA_FILE=str(tmp_path / "quota.json"),
    )
    store = InMemoryTokenStore()
    store.save(
        TokenRecord(
            account_id="runner42",
            access_token="stale-access-token",
            refresh_token="refresh-token",
        )
    )
    workouts_route = respx.get("https://cloudapi.suunto.test/v3/workouts").mock(
        side_effect=[
            Response(401, json={"message": "expired"}),
            Response(200, json={"payload": [{"id": "workout-1"}]}),
        ]
    )
    refresh_route = respx.post("https://oauth.suunto.test/oauth/token").mock(
        return_value=Response(
            200,
            json={
                "access_token": _jwt({"user": "runner42"}),
                "refresh_token": "new-refresh-token",
            },
        )
    )

    async with SuuntoClient(
        account_id="runner42",
        settings_obj=settings,
        token_store=store,
    ) as client:
        payload = await client.get_json("/v3/workouts")

    assert payload == {"payload": [{"id": "workout-1"}]}
    assert workouts_route.call_count == 2
    assert refresh_route.called
    assert workouts_route.calls[0].request.headers["Authorization"] == "Bearer stale-access-token"
    assert workouts_route.calls[1].request.headers["Authorization"].startswith("Bearer ey")
    refreshed = store.resolve("runner42")
    assert refreshed.refresh_token == "new-refresh-token"
    assert quota_status(settings)["used"] == 2
