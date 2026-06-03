from __future__ import annotations

import pytest

from suunto_mcp.config import Settings
from suunto_mcp.quota import QuotaExceededError
from suunto_mcp.tools.activity import (
    _estimate_247_summary,
    _raise_if_247_summary_would_exceed_quota,
)


def test_estimate_247_summary_counts_chunks_and_daily_call(tmp_path) -> None:
    settings = Settings(
        API_WEEKLY_CALL_LIMIT=200,
        API_QUOTA_FILE=str(tmp_path / "quota.json"),
    )

    estimate = _estimate_247_summary(
        "2026-01-01T00:00:00+00:00",
        "2026-03-01T00:00:00+00:00",
        settings_obj=settings,
    )

    assert estimate["chunk_count"] == 3
    assert estimate["call_count"] == 10
    assert estimate["calls"][0]["endpoint"] == "activity"
    assert estimate["calls"][-1]["endpoint"] == "daily_activity_statistics"
    assert estimate["would_exceed_quota"] is False


def test_estimate_247_summary_flags_quota_overrun_before_fetching(tmp_path) -> None:
    settings = Settings(
        API_WEEKLY_CALL_LIMIT=1,
        API_QUOTA_FILE=str(tmp_path / "quota.json"),
    )

    estimate = _estimate_247_summary(
        "2026-01-01T00:00:00+00:00",
        "2026-01-02T00:00:00+00:00",
        include_activity=True,
        include_sleep=False,
        include_recovery=False,
        include_daily_statistics=True,
        settings_obj=settings,
    )

    assert estimate["call_count"] == 2
    assert estimate["would_exceed_quota"] is True
    with pytest.raises(QuotaExceededError, match="would require 2 API calls"):
        _raise_if_247_summary_would_exceed_quota(estimate)
