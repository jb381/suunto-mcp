from __future__ import annotations

from fastmcp import Client

from suunto_mcp.server import mcp


async def test_server_registers_expected_tools_and_resources() -> None:
    async with Client(mcp) as client:
        tools = {tool.name for tool in await client.list_tools()}
        resources = {str(resource.uri) for resource in await client.list_resources()}

    assert len(tools) == 60
    assert "suunto_list_workouts" in tools
    assert "suunto_add_workout_description_info" in tools
    assert "suunto_get_activity_samples" in tools
    assert "suunto_estimate_247_summary_calls" in tools
    assert "suunto_get_legacy_daily_activity" in tools
    assert "suunto_get_legacy_daily_activity_statistics" in tools
    assert "suunto_list_workouts_deprecated_v2" in tools
    assert "suunto_get_workout_deprecated_v2" in tools
    assert "suunto_get_workout_fit_deprecated_v2" in tools
    assert "suunto_import_route_gpx" in tools
    assert "suunto_parse_health_export" in tools
    assert "suunto_import_health_export" in tools
    assert "suunto_parse_activity_csv" in tools
    assert "suunto_import_activity_csv" in tools
    assert "suunto_upload_workout_fit" in tools
    assert "suunto_wait_for_upload_status" in tools
    assert "suunto_validate_guide_json" in tools
    assert "suunto_api_quota_status" in tools
    assert "suunto_delete_webhook_event" in tools
    assert "suunto_suggest_webhook_followups" in tools
    assert "suunto://config" in resources
    assert "suunto://api-catalog" in resources
    assert "suunto://quota" in resources
    assert "suunto://coverage" in resources
    assert len(resources) == 4
