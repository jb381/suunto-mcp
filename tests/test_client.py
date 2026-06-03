from __future__ import annotations

import respx
from httpx import Response

from suunto_mcp.client import SuuntoClient
from suunto_mcp.config import Settings
from suunto_mcp.models import TokenRecord
from suunto_mcp.token_store import InMemoryTokenStore


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
