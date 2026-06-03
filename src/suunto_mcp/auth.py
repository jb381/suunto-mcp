from __future__ import annotations

import base64
import json
import time
from typing import Any, cast
from urllib.parse import urlencode

import httpx

from suunto_mcp.config import Settings, settings
from suunto_mcp.models import TokenRecord, TokenSet, utc_now


def _base64url_decode(value: str) -> bytes:
    padding = "=" * (-len(value) % 4)
    return base64.urlsafe_b64decode((value + padding).encode("ascii"))


def decode_jwt_payload_unverified(token: str) -> dict[str, Any]:
    """Decode JWT payload without verification for local metadata extraction only."""
    parts = token.split(".")
    if len(parts) < 2:
        return {}
    try:
        payload = _base64url_decode(parts[1])
        parsed: Any = json.loads(payload.decode("utf-8"))
    except (ValueError, json.JSONDecodeError):
        return {}
    return cast(dict[str, Any], parsed) if isinstance(parsed, dict) else {}


def infer_account_id(access_token: str, explicit_account_id: str | None = None) -> str:
    if explicit_account_id:
        return explicit_account_id
    payload = decode_jwt_payload_unverified(access_token)
    for key in ("user", "username", "sub"):
        value = payload.get(key)
        if isinstance(value, str) and value:
            return value
    raise ValueError("Could not infer account_id from token; provide account_id explicitly.")


def token_set_from_response(data: dict[str, Any]) -> TokenSet:
    access_token = data.get("access_token")
    if not isinstance(access_token, str) or not access_token:
        raise ValueError("OAuth response did not contain access_token.")

    expires_in = data.get("expires_in")
    expires_at: float | None = None
    if isinstance(expires_in, int | float):
        expires_at = time.time() + float(expires_in)
    else:
        payload = decode_jwt_payload_unverified(access_token)
        exp = payload.get("exp")
        if isinstance(exp, int | float):
            expires_at = float(exp)

    refresh_token = data.get("refresh_token")
    scope = data.get("scope")
    token_type = data.get("token_type") or "bearer"
    return TokenSet(
        access_token=access_token,
        refresh_token=refresh_token if isinstance(refresh_token, str) else None,
        expires_in=int(expires_in) if isinstance(expires_in, int | float) else None,
        expires_at=expires_at,
        scope=scope if isinstance(scope, str) else None,
        token_type=token_type if isinstance(token_type, str) else "bearer",
        raw=data,
    )


def record_from_token_set(
    token_set: TokenSet,
    *,
    account_id: str | None = None,
    label: str | None = None,
    existing: TokenRecord | None = None,
) -> TokenRecord:
    payload = decode_jwt_payload_unverified(token_set.access_token)
    resolved_account_id = infer_account_id(token_set.access_token, account_id)
    username = payload.get("user") or payload.get("username")
    refresh_token = token_set.refresh_token or (existing.refresh_token if existing else None)
    created_at = existing.created_at if existing else utc_now()
    return TokenRecord(
        account_id=resolved_account_id,
        username=username if isinstance(username, str) else None,
        label=label if label is not None else (existing.label if existing else None),
        access_token=token_set.access_token,
        refresh_token=refresh_token,
        expires_at=token_set.expires_at,
        scope=token_set.scope,
        token_type=token_set.token_type,
        created_at=created_at,
        updated_at=utc_now(),
    )


def build_authorization_url(
    *,
    client_id: str | None = None,
    redirect_uri: str | None = None,
    response_type: str = "code",
    state: str | None = None,
    settings_obj: Settings = settings,
) -> str:
    cid = client_id or settings_obj.CLIENT_ID
    uri = redirect_uri or settings_obj.REDIRECT_URI
    if not cid:
        raise ValueError("SUUNTO_CLIENT_ID is required.")
    if not uri:
        raise ValueError("SUUNTO_REDIRECT_URI is required.")
    query = {
        "response_type": response_type,
        "client_id": cid,
        "redirect_uri": uri,
    }
    if state:
        query["state"] = state
    return settings_obj.OAUTH_BASE.rstrip("/") + "/oauth/authorize?" + urlencode(query)


async def exchange_authorization_code(
    code: str,
    *,
    settings_obj: Settings = settings,
    client: httpx.AsyncClient | None = None,
) -> TokenSet:
    if (
        not settings_obj.CLIENT_ID
        or not settings_obj.CLIENT_SECRET
        or not settings_obj.REDIRECT_URI
    ):
        raise ValueError(
            "SUUNTO_CLIENT_ID, SUUNTO_CLIENT_SECRET, and SUUNTO_REDIRECT_URI are required."
        )
    data = {
        "grant_type": "authorization_code",
        "redirect_uri": settings_obj.REDIRECT_URI,
        "code": code,
    }
    close_client = client is None
    http_client = client or httpx.AsyncClient(timeout=30)
    try:
        response = await http_client.post(
            settings_obj.OAUTH_BASE.rstrip("/") + "/oauth/token",
            auth=(settings_obj.CLIENT_ID, settings_obj.CLIENT_SECRET),
            data=data,
        )
        response.raise_for_status()
        return token_set_from_response(response.json())
    finally:
        if close_client:
            await http_client.aclose()


async def refresh_access_token(
    refresh_token: str,
    *,
    settings_obj: Settings = settings,
    client: httpx.AsyncClient | None = None,
) -> TokenSet:
    if not settings_obj.CLIENT_ID or not settings_obj.CLIENT_SECRET:
        raise ValueError("SUUNTO_CLIENT_ID and SUUNTO_CLIENT_SECRET are required.")
    data = {
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    }
    close_client = client is None
    http_client = client or httpx.AsyncClient(timeout=30)
    try:
        response = await http_client.post(
            settings_obj.OAUTH_BASE.rstrip("/") + "/oauth/token",
            auth=(settings_obj.CLIENT_ID, settings_obj.CLIENT_SECRET),
            data=data,
        )
        response.raise_for_status()
        return token_set_from_response(response.json())
    finally:
        if close_client:
            await http_client.aclose()
