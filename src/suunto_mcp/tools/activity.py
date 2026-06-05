# pyright: reportUnusedFunction=false
from __future__ import annotations

from datetime import datetime, timedelta
from typing import Any, cast

from fastmcp import FastMCP

from suunto_mcp.client import SuuntoClient
from suunto_mcp.config import Settings, settings
from suunto_mcp.quota import QuotaExceededError, quota_status
from suunto_mcp.tools import merge_params

MAX_247_WINDOW_DAYS = 28


def _parse_datetime(value: str) -> datetime:
    normalized = value.replace("Z", "+00:00")
    return datetime.fromisoformat(normalized)


def _fmt_datetime(value: datetime) -> str:
    return value.isoformat()


def _to_ms(value: str) -> int:
    dt = _parse_datetime(value)
    return int(dt.timestamp() * 1000)


def _chunks(
    from_value: str, to_value: str, max_days: int = MAX_247_WINDOW_DAYS
) -> list[tuple[str, str]]:
    start = _parse_datetime(from_value)
    end = _parse_datetime(to_value)
    if end < start:
        raise ValueError("to_value must be after from_value.")
    chunks: list[tuple[str, str]] = []
    cursor = start
    while cursor < end:
        chunk_end = min(cursor + timedelta(days=max_days), end)
        chunks.append((_fmt_datetime(cursor), _fmt_datetime(chunk_end)))
        cursor = chunk_end
    return chunks or [(from_value, to_value)]


def _planned_247_summary_calls(
    from_value: str,
    to_value: str,
    *,
    include_activity: bool = True,
    include_sleep: bool = True,
    include_recovery: bool = True,
    include_daily_statistics: bool = True,
) -> list[dict[str, str]]:
    calls: list[dict[str, str]] = []
    for chunk_from, chunk_to in _chunks(from_value, to_value):
        if include_activity:
            calls.append(
                {
                    "endpoint": "activity",
                    "path": "/247samples/activity",
                    "from": _to_ms(chunk_from),
                    "to": _to_ms(chunk_to),
                }
            )
        if include_sleep:
            calls.append(
                {
                    "endpoint": "sleep",
                    "path": "/247samples/sleep",
                    "from": _to_ms(chunk_from),
                    "to": _to_ms(chunk_to),
                }
            )
        if include_recovery:
            calls.append(
                {
                    "endpoint": "recovery",
                    "path": "/247samples/recovery",
                    "from": _to_ms(chunk_from),
                    "to": _to_ms(chunk_to),
                }
            )
    if include_daily_statistics:
        calls.append(
            {
                "endpoint": "daily_activity_statistics",
                "path": "/247samples/daily-activity-statistics",
                "startdate": from_value[:10],
                "enddate": to_value[:10],
            }
        )
    return calls


def _estimate_247_summary(
    from_value: str,
    to_value: str,
    *,
    include_activity: bool = True,
    include_sleep: bool = True,
    include_recovery: bool = True,
    include_daily_statistics: bool = True,
    settings_obj: Settings = settings,
) -> dict[str, Any]:
    chunks = _chunks(from_value, to_value)
    calls = _planned_247_summary_calls(
        from_value,
        to_value,
        include_activity=include_activity,
        include_sleep=include_sleep,
        include_recovery=include_recovery,
        include_daily_statistics=include_daily_statistics,
    )
    quota = quota_status(settings_obj)
    remaining = quota.get("remaining")
    would_exceed_quota = isinstance(remaining, int) and len(calls) > remaining
    return {
        "call_count": len(calls),
        "chunk_count": len(chunks),
        "max_window_days": MAX_247_WINDOW_DAYS,
        "calls": calls,
        "quota": quota,
        "would_exceed_quota": would_exceed_quota,
    }


def _raise_if_247_summary_would_exceed_quota(estimate: dict[str, Any]) -> None:
    if not estimate.get("would_exceed_quota"):
        return
    quota_obj = estimate.get("quota")
    quota = cast(dict[str, Any], quota_obj) if isinstance(quota_obj, dict) else {}
    raise QuotaExceededError(
        "Suunto 24/7 summary would require "
        f"{estimate.get('call_count')} API calls, but only "
        f"{quota.get('remaining')} of {quota.get('limit')} weekly calls remain. "
        "Narrow the range, disable some include_* options, or wait for quota reset."
    )


async def _get_247(
    endpoint: str,
    *,
    from_value: str,
    to_value: str,
    account_id: str | None,
    query_params: dict[str, Any] | None = None,
) -> Any:
    params = merge_params({"from": _to_ms(from_value), "to": _to_ms(to_value)}, query_params)
    async with SuuntoClient(account_id=account_id) as client:
        return await client.get_json(endpoint, params=params)


def register(mcp: FastMCP) -> None:
    @mcp.tool(
        name="suunto_get_activity_samples",
        description="Fetch 24/7 activity samples for a Suunto account.",
    )
    async def get_activity_samples(
        from_value: str,
        to_value: str,
        account_id: str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        return await _get_247(
            "/247samples/activity",
            from_value=from_value,
            to_value=to_value,
            account_id=account_id,
            query_params=query_params,
        )

    @mcp.tool(
        name="suunto_get_sleep_data",
        description="Fetch 24/7 sleep samples where available for the account/API subscription.",
    )
    async def get_sleep_data(
        from_value: str,
        to_value: str,
        account_id: str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        return await _get_247(
            "/247samples/sleep",
            from_value=from_value,
            to_value=to_value,
            account_id=account_id,
            query_params=query_params,
        )

    @mcp.tool(
        name="suunto_get_recovery_data",
        description="Fetch 24/7 recovery samples where available for the account/API subscription.",
    )
    async def get_recovery_data(
        from_value: str,
        to_value: str,
        account_id: str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        return await _get_247(
            "/247samples/recovery",
            from_value=from_value,
            to_value=to_value,
            account_id=account_id,
            query_params=query_params,
        )

    @mcp.tool(
        name="suunto_get_daily_activity_statistics",
        description="Fetch daily activity statistics for a date range.",
    )
    async def get_daily_activity_statistics(
        startdate: str,
        enddate: str,
        account_id: str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        params = merge_params({"startdate": startdate, "enddate": enddate}, query_params)
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json("/247samples/daily-activity-statistics", params=params)

    @mcp.tool(
        name="suunto_estimate_247_summary_calls",
        description="Estimate Suunto API calls for a 24/7 summary without fetching data.",
    )
    def estimate_247_summary_calls(
        from_value: str,
        to_value: str,
        include_activity: bool = True,
        include_sleep: bool = True,
        include_recovery: bool = True,
        include_daily_statistics: bool = True,
    ) -> dict[str, Any]:
        return _estimate_247_summary(
            from_value,
            to_value,
            include_activity=include_activity,
            include_sleep=include_sleep,
            include_recovery=include_recovery,
            include_daily_statistics=include_daily_statistics,
        )

    @mcp.tool(
        name="suunto_get_247_summary",
        description="Fetch activity, sleep, recovery, and daily statistics using 28-day chunks.",
    )
    async def get_247_summary(
        from_value: str,
        to_value: str,
        account_id: str | None = None,
        include_activity: bool = True,
        include_sleep: bool = True,
        include_recovery: bool = True,
        include_daily_statistics: bool = True,
    ) -> dict[str, Any]:
        estimate = _estimate_247_summary(
            from_value,
            to_value,
            include_activity=include_activity,
            include_sleep=include_sleep,
            include_recovery=include_recovery,
            include_daily_statistics=include_daily_statistics,
        )
        _raise_if_247_summary_would_exceed_quota(estimate)
        calls = estimate["calls"]
        results: dict[str, list[Any]] = {
            "activity": [],
            "sleep": [],
            "recovery": [],
            "daily_activity_statistics": [],
        }
        async with SuuntoClient(account_id=account_id) as client:
            for call in calls:
                endpoint = call["endpoint"]
                if endpoint == "activity":
                    results["activity"].append(
                        await client.get_json(
                            "/247samples/activity",
                            params={"from": call["from"], "to": call["to"]},
                        )
                    )
                elif endpoint == "sleep":
                    results["sleep"].append(
                        await client.get_json(
                            "/247samples/sleep",
                            params={"from": call["from"], "to": call["to"]},
                        )
                    )
                elif endpoint == "recovery":
                    results["recovery"].append(
                        await client.get_json(
                            "/247samples/recovery",
                            params={"from": call["from"], "to": call["to"]},
                        )
                    )
                elif endpoint == "daily_activity_statistics":
                    results["daily_activity_statistics"].append(
                        await client.get_json(
                            "/247samples/daily-activity-statistics",
                            params={"startdate": call["startdate"], "enddate": call["enddate"]},
                        )
                    )
                else:
                    raise ValueError(f"Unsupported 24/7 summary endpoint: {endpoint}")
        return {
            "call_count": len(calls),
            "calls": calls,
            "quota": estimate["quota"],
            "results": results,
        }

    @mcp.tool(
        name="suunto_get_legacy_daily_activity",
        description=(
            "Fetch the legacy daily activity API surface when enabled for the subscription."
        ),
    )
    async def get_legacy_daily_activity(
        account_id: str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json("/247", params=query_params)

    @mcp.tool(
        name="suunto_get_legacy_daily_activity_statistics",
        description="Fetch legacy daily activity statistics when enabled for the subscription.",
    )
    async def get_legacy_daily_activity_statistics(
        startdate: str,
        enddate: str,
        account_id: str | None = None,
        query_params: dict[str, Any] | None = None,
    ) -> Any:
        params = merge_params({"startdate": startdate, "enddate": enddate}, query_params)
        async with SuuntoClient(account_id=account_id) as client:
            return await client.get_json("/247/daily-activity-statistics", params=params)
