from __future__ import annotations

import hashlib
import secrets

from fastmcp.server.auth.auth import AccessToken, TokenVerifier


class StaticBearerTokenVerifier(TokenVerifier):
    """Verify one configured MCP bearer token using constant-time comparison."""

    def __init__(self, token: str, *, base_url: str | None = None) -> None:
        super().__init__(base_url=base_url)
        if not token:
            raise ValueError("MCP bearer token must not be empty.")
        self._token = token
        self._token_fingerprint = hashlib.sha256(token.encode("utf-8")).hexdigest()[:16]

    async def verify_token(self, token: str) -> AccessToken | None:
        if not secrets.compare_digest(token, self._token):
            return None
        return AccessToken(
            token=token,
            client_id="suunto-mcp-static-token",
            scopes=["mcp"],
            claims={"token_fingerprint": self._token_fingerprint},
        )
