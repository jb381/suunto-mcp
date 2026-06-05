# pyright: reportUnusedFunction=false
from __future__ import annotations

import json
import time
from pathlib import Path
from typing import Any, cast

import httpx
from fastmcp import FastMCP

from suunto_mcp.auth import (
    build_authorization_url,
    exchange_authorization_code,
    record_from_token_set,
    refresh_access_token,
    token_set_from_response,
)
from suunto_mcp.config import settings
from suunto_mcp.token_store import get_token_store


def register(mcp: FastMCP) -> None:
    @mcp.tool(
        name="suunto_get_authorization_url",
        description="Build the Suunto OAuth browser authorization URL.",
    )
    def get_authorization_url(state: str | None = None) -> dict[str, Any]:
        return {
            "authorization_url": build_authorization_url(state=state),
            "redirect_uri": settings.REDIRECT_URI,
            "next_step": (
                "Open authorization_url, authorize with a Suunto App account, "
                "then exchange the returned code."
            ),
        }

    @mcp.tool(
        name="suunto_exchange_authorization_code",
        description="Exchange a Suunto OAuth authorization code for tokens and store them locally.",
    )
    async def exchange_code(
        code: str,
        account_id: str | None = None,
        label: str | None = None,
    ) -> dict[str, Any]:
        token_set = await exchange_authorization_code(code)
        store = get_token_store()
        existing = store.get(account_id) if account_id else None
        record = record_from_token_set(
            token_set, account_id=account_id, label=label, existing=existing
        )
        store.save(record)
        return {"stored": True, "account": record.safe_dict()}

    @mcp.tool(
        name="suunto_import_token_set",
        description=(
            "Store an existing Suunto access token and optional refresh token "
            "without exposing token values."
        ),
    )
    def import_token_set(
        access_token: str,
        refresh_token: str | None = None,
        account_id: str | None = None,
        label: str | None = None,
        expires_in: int | None = None,
        expires_at: float | None = None,
        scope: str | None = None,
        token_type: str = "bearer",
    ) -> dict[str, Any]:
        token_set = token_set_from_response(
            {
                "access_token": access_token,
                "refresh_token": refresh_token,
                "expires_in": expires_in,
                "scope": scope,
                "token_type": token_type,
            }
        )
        if expires_at is not None:
            token_set.expires_at = expires_at
        store = get_token_store()
        existing = store.get(account_id) if account_id else None
        record = record_from_token_set(
            token_set, account_id=account_id, label=label, existing=existing
        )
        store.save(record)
        return {"stored": True, "account": record.safe_dict()}

    @mcp.tool(
        name="suunto_import_token_file",
        description="Import a token JSON response file created by a trusted OAuth test flow.",
    )
    def import_token_file(
        path: str,
        account_id: str | None = None,
        label: str | None = None,
    ) -> dict[str, Any]:
        token_path = Path(path).expanduser()
        data_obj: object = json.loads(token_path.read_text(encoding="utf-8"))
        if not isinstance(data_obj, dict):
            raise ValueError("Token file must contain a JSON object.")
        data = cast(dict[str, Any], data_obj)
        token_set = token_set_from_response(data)
        store = get_token_store()
        existing = store.get(account_id) if account_id else None
        record = record_from_token_set(
            token_set, account_id=account_id, label=label, existing=existing
        )
        store.save(record)
        return {"stored": True, "account": record.safe_dict()}

    @mcp.tool(
        name="suunto_refresh_token",
        description="Refresh one stored Suunto account, or all accounts that have refresh tokens.",
    )
    async def refresh_token(account_id: str | None = None) -> dict[str, Any]:
        store = get_token_store()
        records = [store.resolve(account_id)] if account_id else store.list_records()
        refreshed: list[dict[str, Any]] = []
        skipped: list[str] = []
        for record in records:
            if not record.refresh_token:
                skipped.append(record.account_id)
                continue
            token_set = await refresh_access_token(record.refresh_token)
            updated = record_from_token_set(
                token_set, account_id=record.account_id, existing=record
            )
            store.save(updated)
            refreshed.append(updated.safe_dict())
        return {"refreshed": refreshed, "skipped_without_refresh_token": skipped}

    @mcp.tool(
        name="suunto_list_accounts", description="List stored Suunto accounts without secrets."
    )
    def list_accounts() -> dict[str, Any]:
        return {"accounts": [record.safe_dict() for record in get_token_store().list_records()]}

    @mcp.tool(
        name="suunto_auth_status", description="Report whether stored account tokens are usable."
    )
    def auth_status(account_id: str | None = None) -> dict[str, Any]:
        store = get_token_store()
        records = [store.resolve(account_id)] if account_id else store.list_records()
        now = time.time()
        return {
            "accounts": [
                {
                    **record.safe_dict(),
                    "is_expired": record.expires_at is not None and record.expires_at <= now,
                    "seconds_until_expiry": (
                        int(record.expires_at - now) if record.expires_at is not None else None
                    ),
                }
                for record in records
            ]
        }

    @mcp.tool(
        name="suunto_deauthorize",
        description=(
            "Call Suunto OAuth deauthorize for an account and optionally remove local tokens."
        ),
    )
    async def deauthorize(
        account_id: str | None = None, remove_local: bool = True
    ) -> dict[str, Any]:
        store = get_token_store()
        record = store.resolve(account_id)
        async with httpx.AsyncClient(timeout=30) as client:
            response = await client.get(
                settings.OAUTH_BASE.rstrip("/") + "/oauth/deauthorize",
                headers={"Authorization": f"Bearer {record.access_token}"},
            )
        if response.status_code >= 400:
            return {
                "deauthorized": False,
                "status_code": response.status_code,
                "note": "Suunto API returned an error during deauthorization.",
                "local_removed": False,
            }
        removed = store.delete(record.account_id) if remove_local else False
        return {"deauthorized": True, "status_code": response.status_code, "local_removed": removed}
