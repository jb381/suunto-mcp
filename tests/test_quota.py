from __future__ import annotations

import pytest

from suunto_mcp.config import Settings
from suunto_mcp.quota import QuotaExceededError, WeeklyQuotaGuard, quota_status


async def test_weekly_quota_guard_records_and_blocks_when_limit_reached(tmp_path) -> None:
    settings = Settings(
        API_WEEKLY_CALL_LIMIT=1,
        API_QUOTA_FILE=str(tmp_path / "quota.json"),
    )
    guard = WeeklyQuotaGuard()

    first = await guard.record_call(
        settings_obj=settings,
        account_id="runner42",
        method="GET",
        path="/v3/workouts",
    )

    assert first is not None
    assert first["used"] == 1
    assert first["remaining"] == 0
    with pytest.raises(QuotaExceededError):
        await guard.record_call(
            settings_obj=settings,
            account_id="runner42",
            method="GET",
            path="/v3/workouts",
        )


def test_quota_status_can_be_disabled(tmp_path) -> None:
    settings = Settings(
        API_WEEKLY_CALL_LIMIT=0,
        API_QUOTA_FILE=str(tmp_path / "quota.json"),
    )

    status = quota_status(settings)

    assert status["enabled"] is False
    assert status["remaining"] is None
