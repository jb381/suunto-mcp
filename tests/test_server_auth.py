from __future__ import annotations

import pytest

from suunto_mcp import _configure_http_auth
from suunto_mcp.server_auth import StaticBearerTokenVerifier


async def test_static_bearer_token_verifier_accepts_only_configured_token() -> None:
    verifier = StaticBearerTokenVerifier("secret")

    assert await verifier.verify_token("wrong") is None
    access = await verifier.verify_token("secret")

    assert access is not None
    assert access.client_id == "suunto-mcp-static-token"
    assert access.scopes == ["mcp"]
    assert access.claims["token_fingerprint"]


def test_non_local_http_requires_mcp_token(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("suunto_mcp.settings.MCP_API_TOKEN", "")

    with pytest.raises(SystemExit, match="SUUNTO_MCP_API_TOKEN"):
        _configure_http_auth("0.0.0.0", 8000)


def test_local_http_can_run_without_mcp_token(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("suunto_mcp.settings.MCP_API_TOKEN", "")

    assert _configure_http_auth("127.0.0.1", 8000) is None


def test_http_auth_is_configured_when_token_exists(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("suunto_mcp.settings.MCP_API_TOKEN", "secret")

    assert isinstance(_configure_http_auth("0.0.0.0", 8000), StaticBearerTokenVerifier)
