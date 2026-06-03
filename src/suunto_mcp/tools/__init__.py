# pyright: reportUnusedFunction=false
from __future__ import annotations

from typing import Any

from suunto_mcp.config import settings
from suunto_mcp.quota import quota_status


def require_write_tools_enabled() -> None:
    if not settings.ENABLE_WRITE_TOOLS:
        raise RuntimeError(
            "This is a write/push operation. Set SUUNTO_ENABLE_WRITE_TOOLS=true to enable it."
        )


def clean_params(params: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in params.items() if value is not None}


def merge_params(base: dict[str, Any], extra: dict[str, Any] | None) -> dict[str, Any]:
    merged = clean_params(base)
    if extra:
        merged.update({key: value for key, value in extra.items() if value is not None})
    return merged


def register_safety_tools(mcp: Any) -> None:
    @mcp.tool(name="suunto_api_quota_status", description="Inspect local Suunto API quota usage.")
    def api_quota_status() -> dict[str, Any]:
        return quota_status(settings)
