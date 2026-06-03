# pyright: reportUnusedFunction=false
from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any, Literal, cast

import httpx
from fastmcp import FastMCP

from suunto_mcp.client import SuuntoClient
from suunto_mcp.tools import require_write_tools_enabled

Privacy = Literal["DEFAULT", "PRIVATE", "FOLLOWERS", "PUBLIC"]
DEFAULT_UPLOAD_TERMINAL_STATUSES = ["COMPLETED", "COMPLETE", "DONE", "FAILED", "ERROR"]


def _redacted_signed_url(value: str) -> str:
    if "?" not in value:
        return value
    return value.split("?", 1)[0] + "?<redacted-signature>"


def _redact_upload_response(data: Any) -> Any:
    if isinstance(data, dict):
        redacted = dict(cast(dict[str, Any], data))
        for key in ("url", "curlString"):
            value = redacted.get(key)
            if isinstance(value, str):
                redacted[key] = _redacted_signed_url(value)
        return redacted
    return data


def _upload_status_value(status: Any) -> str | None:
    if not isinstance(status, dict):
        return None
    status_dict = cast(dict[str, Any], status)
    for key in ("status", "state", "uploadStatus", "processingStatus"):
        value = status_dict.get(key)
        if isinstance(value, str) and value:
            return value
    return None


def _is_terminal_upload_status(status: Any, terminal_statuses: list[str]) -> bool:
    value = _upload_status_value(status)
    if value is None:
        return False
    normalized = {item.upper() for item in terminal_statuses}
    return value.upper() in normalized


async def _wait_for_upload_status(
    client: SuuntoClient,
    upload_id: str,
    *,
    terminal_statuses: list[str] | None = None,
    poll_interval_seconds: float = 5.0,
    max_attempts: int = 12,
) -> dict[str, Any]:
    if max_attempts < 1:
        raise ValueError("max_attempts must be at least 1.")
    terminal = terminal_statuses or DEFAULT_UPLOAD_TERMINAL_STATUSES
    attempts: list[Any] = []
    for attempt in range(1, max_attempts + 1):
        status = await client.get_json(f"/v2/upload/{upload_id}")
        attempts.append(status)
        if _is_terminal_upload_status(status, terminal):
            return {
                "terminal": True,
                "attempt_count": attempt,
                "terminal_statuses": terminal,
                "final_status": status,
                "attempts": attempts,
            }
        if attempt < max_attempts and poll_interval_seconds > 0:
            await asyncio.sleep(poll_interval_seconds)
    return {
        "terminal": False,
        "attempt_count": len(attempts),
        "terminal_statuses": terminal,
        "final_status": attempts[-1] if attempts else None,
        "attempts": attempts,
    }


def register(mcp: FastMCP) -> None:
    @mcp.tool(
        name="suunto_init_workout_upload",
        description=(
            "Initialize a Suunto FIT workout upload. Requires SUUNTO_ENABLE_WRITE_TOOLS=true."
        ),
    )
    async def init_workout_upload(
        account_id: str | None = None,
        description: str | None = None,
        comment: str | None = None,
        notify_user: bool | None = None,
        privacy: Privacy | None = None,
    ) -> Any:
        require_write_tools_enabled()
        body = {
            key: value
            for key, value in {
                "description": description,
                "comment": comment,
                "notifyUser": notify_user,
                "privacy": privacy,
            }.items()
            if value is not None
        }
        async with SuuntoClient(account_id=account_id) as client:
            response = await client.post_json("/v2/upload", body=body)
        return _redact_upload_response(response)

    @mcp.tool(
        name="suunto_upload_workout_fit_to_signed_url",
        description=(
            "Upload a FIT file to the signed object-storage URL returned by "
            "suunto_init_workout_upload."
        ),
    )
    async def upload_workout_fit_to_signed_url(
        signed_url: str,
        fit_path: str,
        headers: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        require_write_tools_enabled()
        data = Path(fit_path).expanduser().read_bytes()
        async with httpx.AsyncClient(timeout=60) as client:
            response = await client.put(signed_url, content=data, headers=headers or {})
        return {
            "status_code": response.status_code,
            "ok": 200 <= response.status_code < 300,
            "content_md5": response.headers.get("content-md5"),
            "etag": response.headers.get("etag"),
            "signed_url": _redacted_signed_url(signed_url),
            "response_preview": response.text[:1000],
        }

    @mcp.tool(name="suunto_get_upload_status", description="Fetch Suunto workout upload status.")
    async def get_upload_status(upload_id: str, account_id: str | None = None) -> Any:
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json(f"/v2/upload/{upload_id}")

    @mcp.tool(
        name="suunto_wait_for_upload_status",
        description="Poll Suunto workout upload status until a terminal state or attempt limit.",
    )
    async def wait_for_upload_status(
        upload_id: str,
        account_id: str | None = None,
        terminal_statuses: list[str] | None = None,
        poll_interval_seconds: float = 5.0,
        max_attempts: int = 12,
    ) -> dict[str, Any]:
        async with SuuntoClient(account_id=account_id) as client:
            return await _wait_for_upload_status(
                client,
                upload_id,
                terminal_statuses=terminal_statuses,
                poll_interval_seconds=poll_interval_seconds,
                max_attempts=max_attempts,
            )

    @mcp.tool(
        name="suunto_upload_workout_fit",
        description=(
            "Initialize and upload a FIT file to Suunto App. "
            "Requires SUUNTO_ENABLE_WRITE_TOOLS=true."
        ),
    )
    async def upload_workout_fit(
        fit_path: str,
        account_id: str | None = None,
        description: str | None = None,
        comment: str | None = None,
        notify_user: bool | None = None,
        privacy: Privacy | None = None,
        fetch_status: bool = True,
        wait_for_status: bool = False,
        poll_interval_seconds: float = 5.0,
        max_status_attempts: int = 12,
        terminal_statuses: list[str] | None = None,
    ) -> dict[str, Any]:
        require_write_tools_enabled()
        body = {
            key: value
            for key, value in {
                "description": description,
                "comment": comment,
                "notifyUser": notify_user,
                "privacy": privacy,
            }.items()
            if value is not None
        }
        async with SuuntoClient(account_id=account_id) as client:
            init_response = await client.post_json("/v2/upload", body=body)
            if not isinstance(init_response, dict):
                raise ValueError("Upload initialization response was not an object.")
            init_data = cast(dict[str, Any], init_response)
            signed_url: object = init_data.get("url")
            method: object = init_data.get("method", "PUT")
            upload_headers_obj: object = init_data.get("headers") or {}
            upload_id: object = init_data.get("id")
            if not isinstance(signed_url, str) or method != "PUT":
                raise ValueError("Upload initialization did not return a PUT signed URL.")
            upload_headers = (
                cast(dict[str, str], upload_headers_obj)
                if isinstance(upload_headers_obj, dict)
                else {}
            )
            data = Path(fit_path).expanduser().read_bytes()
            async with httpx.AsyncClient(timeout=60) as upload_client:
                upload_response = await upload_client.put(
                    signed_url,
                    content=data,
                    headers=upload_headers,
                )
            status = None
            if fetch_status and isinstance(upload_id, str):
                if wait_for_status:
                    status = await _wait_for_upload_status(
                        client,
                        upload_id,
                        terminal_statuses=terminal_statuses,
                        poll_interval_seconds=poll_interval_seconds,
                        max_attempts=max_status_attempts,
                    )
                else:
                    status = await client.get_json(f"/v2/upload/{upload_id}")
        return {
            "initialized": _redact_upload_response(init_response),
            "uploaded": {
                "status_code": upload_response.status_code,
                "ok": 200 <= upload_response.status_code < 300,
                "etag": upload_response.headers.get("etag"),
            },
            "status": status,
        }
