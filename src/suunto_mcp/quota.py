from __future__ import annotations

import asyncio
import json
import os
import stat
import time
from pathlib import Path
from typing import Any, cast

from suunto_mcp.config import Settings, settings

SECONDS_PER_WEEK = 7 * 24 * 60 * 60


class QuotaExceededError(RuntimeError):
    pass


class SimpleRateLimiter:
    def __init__(self) -> None:
        self._lock = asyncio.Lock()
        self._last_call_by_key: dict[str, float] = {}

    async def wait(self, *, key: str, calls_per_minute: float) -> None:
        if calls_per_minute <= 0:
            return
        min_interval = 60.0 / calls_per_minute
        async with self._lock:
            now = time.monotonic()
            last_call = self._last_call_by_key.get(key, 0.0)
            delay = min_interval - (now - last_call)
            if delay > 0:
                await asyncio.sleep(delay)
            self._last_call_by_key[key] = time.monotonic()


class WeeklyQuotaGuard:
    def __init__(self) -> None:
        self._lock = asyncio.Lock()

    async def record_call(
        self,
        *,
        settings_obj: Settings = settings,
        account_id: str | None = None,
        method: str,
        path: str,
    ) -> dict[str, Any] | None:
        limit = settings_obj.API_WEEKLY_CALL_LIMIT
        if limit <= 0:
            return None
        async with self._lock:
            return self._record_call_sync(
                settings_obj=settings_obj,
                account_id=account_id,
                method=method,
                path=path,
            )

    def _record_call_sync(
        self,
        *,
        settings_obj: Settings,
        account_id: str | None,
        method: str,
        path: str,
    ) -> dict[str, Any]:
        quota_path = quota_file(settings_obj)
        now = time.time()
        window_start = now - SECONDS_PER_WEEK
        data = read_quota_file(quota_path)
        calls = recent_calls(data, window_start)
        if len(calls) >= settings_obj.API_WEEKLY_CALL_LIMIT:
            oldest = min(call_timestamp(call) for call in calls)
            resets_at = oldest + SECONDS_PER_WEEK
            raise QuotaExceededError(
                "Suunto API weekly call limit would be exceeded "
                f"({len(calls)}/{settings_obj.API_WEEKLY_CALL_LIMIT}). "
                f"Next call is available after {int(resets_at)}."
            )
        call_record: dict[str, Any] = {
            "timestamp": now,
            "method": method,
            "path": path,
            "account_id": account_id,
        }
        calls.append(call_record)
        data: dict[str, Any] = {"window_seconds": SECONDS_PER_WEEK, "calls": calls}
        write_quota_file(quota_path, data)
        return quota_status_from_calls(calls, settings_obj.API_WEEKLY_CALL_LIMIT)


def quota_file(settings_obj: Settings = settings) -> Path:
    if settings_obj.API_QUOTA_FILE:
        return Path(settings_obj.API_QUOTA_FILE).expanduser()
    return Path(settings_obj.LOCAL_DATA_DIR).expanduser().parent / "api-quota.json"


def read_quota_file(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {"window_seconds": SECONDS_PER_WEEK, "calls": []}
    data_obj: object = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data_obj, dict):
        raise ValueError(f"Quota file {path} is not a JSON object.")
    data = cast(dict[str, Any], data_obj)
    calls = data.get("calls")
    if not isinstance(calls, list):
        raise ValueError(f"Quota file {path} does not contain a calls list.")
    return data


def call_timestamp(call: dict[str, Any]) -> float:
    value = call.get("timestamp")
    if isinstance(value, int | float):
        return float(value)
    if isinstance(value, str):
        try:
            return float(value)
        except ValueError:
            return 0.0
    return 0.0


def recent_calls(data: dict[str, Any], window_start: float) -> list[dict[str, Any]]:
    calls_obj = data.get("calls", [])
    if not isinstance(calls_obj, list):
        return []
    items = cast(list[Any], calls_obj)
    calls: list[dict[str, Any]] = []
    for item in items:
        if isinstance(item, dict):
            call = cast(dict[str, Any], item)
            if call_timestamp(call) >= window_start:
                calls.append(call)
    return calls


def write_quota_file(path: Path, data: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, sort_keys=True), encoding="utf-8")
    os.chmod(tmp, stat.S_IRUSR | stat.S_IWUSR)
    os.replace(str(tmp), str(path))
    os.chmod(path, stat.S_IRUSR | stat.S_IWUSR)


def quota_status(settings_obj: Settings = settings) -> dict[str, Any]:
    limit = settings_obj.API_WEEKLY_CALL_LIMIT
    path = quota_file(settings_obj)
    if limit <= 0:
        return {
            "enabled": False,
            "limit": limit,
            "used": None,
            "remaining": None,
            "path": str(path),
        }
    data = read_quota_file(path)
    now = time.time()
    window_start = now - SECONDS_PER_WEEK
    calls = recent_calls(data, window_start)
    return {**quota_status_from_calls(calls, limit), "path": str(path)}


def quota_status_from_calls(calls: list[dict[str, Any]], limit: int) -> dict[str, Any]:
    used = len(calls)
    reset_at = None
    if calls:
        reset_at = int(min(call_timestamp(call) for call in calls) + SECONDS_PER_WEEK)
    return {
        "enabled": True,
        "limit": limit,
        "used": used,
        "remaining": max(limit - used, 0),
        "reset_at": reset_at,
    }


rate_limiter = SimpleRateLimiter()
weekly_quota_guard = WeeklyQuotaGuard()
