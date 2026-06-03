from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import pytest
from fastmcp import Client

from suunto_mcp.config import settings
from suunto_mcp.server import mcp


def _live_token_file() -> Path:
    configured = os.environ.get("SUUNTO_LIVE_TOKEN_FILE")
    if configured:
        return Path(configured).expanduser()
    return Path(".suunto-mcp/oauth-test-token.json")


pytestmark = pytest.mark.skipif(
    not settings.LIVE_TESTS,
    reason="Set SUUNTO_LIVE_TESTS=true to run live Suunto API tests.",
)


def _structured(result: Any) -> Any:
    data = getattr(result, "structured_content", None)
    if data is not None:
        return data
    data = getattr(result, "data", None)
    if data is not None:
        return data
    return result


async def test_live_list_workouts_read_only(monkeypatch: pytest.MonkeyPatch) -> None:
    token_file = _live_token_file()
    if not token_file.exists():
        pytest.skip(f"No live token file found at {token_file}.")

    monkeypatch.setattr("suunto_mcp.token_store.settings.TOKEN_STORE", "file")
    monkeypatch.setattr(
        "suunto_mcp.token_store.settings.TOKEN_FILE", ".suunto-mcp/live-test-tokens.json"
    )
    monkeypatch.setattr("suunto_mcp.client.settings.TOKEN_STORE", "file")
    monkeypatch.setattr(
        "suunto_mcp.client.settings.TOKEN_FILE", ".suunto-mcp/live-test-tokens.json"
    )

    async with Client(mcp) as client:
        await client.call_tool("suunto_import_token_file", {"path": str(token_file)})
        response = _structured(await client.call_tool("suunto_list_workouts", {"limit": 1}))

    assert isinstance(response, dict)
    assert response.get("error") is None
    assert "payload" in response
